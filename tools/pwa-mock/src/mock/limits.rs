//! Limits (v5): model backend groups behind the Kanata gateway, their
//! permits, queue, rate bucket, retry budget and breaker, and the gateway's
//! admission refusals by kind. Synthetic, shaped for the admin view. Times
//! are ISO instants, as the Rust server's `iso_instant`.

use super::clock::{iso_now, iso_z};
use super::config::Group;
use super::seed;
use super::{MoveError, Store};
use serde_json::{Value, json};

impl Store {
    pub fn limits(&self) -> Value {
        let minute = Self::now_minute();
        let ago = |m: i64| iso_z(minute - m);
        let allowances: Vec<Value> = self
            .members
            .iter()
            .filter(|m| m.seed.access != "none")
            .map(|m| {
                let used = if self.limit_resets.contains(&m.seed.id) { 0 } else { (m.seed.id.as_bytes()[3] % 4) as u32 };
                json!({
                    "member": { "id": m.seed.id, "name": m.seed.name },
                    "staff": m.seed.access == "staff",
                    "allowance": if m.seed.access == "staff" { Value::Null } else { json!({ "count": 4, "per_s": 300 }) },
                    "used": used,
                    "override": m.seed.id == "1010",
                })
            })
            .collect();
        // The groups Config's capacity table names (`live_groups`), each with
        // the seeded state of its position: full and queueing, half-open, open.
        let groups: Vec<Value> = super::config::live_groups(&self.config)
            .into_iter()
            .enumerate()
            .map(|(i, g)| {
                let mut group = json!({
                    "name": g.name, "backend": "Kanata", "models": g.models,
                    "permits": { "in_use": g.in_use, "total": g.total },
                });
                let state = match i {
                    0 => json!({
                        "queue": [
                            { "position": 1, "kind": "rescan", "who": "admin token", "waiting_s": 42 },
                            { "position": 2, "kind": "extraction", "who": "#hstar-party burst", "waiting_s": 8 },
                        ],
                        "rate": { "available": 9, "capacity": 12, "refill_per_min": 2 },
                        "retry": { "remaining": 5, "capacity": 5 },
                        "breaker": { "state": "closed", "failures": 0, "since": ago(600) },
                    }),
                    1 => json!({
                        "queue": [],
                        "rate": { "available": 1, "capacity": 20, "refill_per_min": 4 },
                        "retry": { "remaining": 2, "capacity": 5 },
                        "breaker": { "state": "half_open", "failures": 3, "since": ago(4) },
                    }),
                    _ => json!({
                        "queue": [],
                        "rate": { "available": 6, "capacity": 6, "refill_per_min": 1 },
                        "retry": { "remaining": 0, "capacity": 3 },
                        "breaker": { "state": "open", "failures": 5, "since": ago(12), "retry_at": iso_z(minute + 3) },
                    }),
                };
                group.as_object_mut().unwrap().extend(state.as_object().unwrap().clone());
                group
            })
            .collect();
        let named = |i: usize| {
            groups
                .get(i)
                .or(groups.first())
                .map_or(json!("gateway"), |g| g["name"].clone())
        };
        json!({
            "groups": groups,
            "admission": {
                "window": "last hour",
                "refusals": [
                    { "kind": "rate", "scope": "group", "target": named(1), "count": 3, "last_at": ago(6) },
                    { "kind": "concurrency", "scope": "group", "target": named(0), "count": 1, "last_at": ago(41) },
                    { "kind": "quota", "scope": "key", "target": "gateway key …7f2a", "count": 1, "last_at": ago(22) },
                    { "kind": "key_rate", "scope": "key", "target": "gateway key …7f2a", "count": 2, "last_at": ago(9) },
                ],
            },
            "allowances": allowances,
            "generated_at": iso_now(),
        })
    }

    /// Dev-only (`POST /__mock/limits {"groups": …}`): `three` declares cloud,
    /// local and legacy groups so Limits shows the seeded full, half-open and
    /// open states side by side; `default` returns to the one gateway group.
    pub fn seed_limit_groups(&mut self, which: &str) -> bool {
        let group = |model: &str, group: &str, permits: u32| Group {
            model: model.into(),
            group: group.into(),
            permits,
        };
        self.config.declared_groups = match which {
            "three" => Some(vec![
                group("kanata/chat-cloud", "cloud", 2),
                group("kanata/rewrite-cloud", "cloud", 2),
                group("kanata/chat", "local", 4),
                group("kanata/legacy", "legacy", 2),
            ]),
            "default" => None,
            _ => return false,
        };
        true
    }

    pub fn reset_window(&mut self, id: &str) -> Result<Value, MoveError> {
        let (id, name) = seed::member_name(id).ok_or(MoveError::NotFound)?;
        if !self.limit_resets.contains(&id) {
            self.limit_resets.push(id);
        }
        Ok(json!({ "message": format!("{name}'s window is reset.") }))
    }
}

#[cfg(test)]
mod tests {
    use crate::mock::tests::store;

    #[test]
    fn seeded_groups_show_three_states_and_default_restores_the_gateway() {
        let mut s = store();
        let one = s.limits();
        assert_eq!(one["groups"].as_array().unwrap().len(), 1);
        assert_eq!(one["groups"][0]["name"], "gateway");
        assert!(one["generated_at"].as_str().unwrap().ends_with('Z'));
        assert!(s.seed_limit_groups("three"));
        let three = s.limits();
        let row = |i: usize, key: &str| three["groups"][i][key].clone();
        assert_eq!(
            [row(0, "name"), row(1, "name"), row(2, "name")],
            ["cloud", "local", "legacy"]
        );
        assert_eq!(
            three["groups"][0]["permits"],
            serde_json::json!({ "in_use": 2, "total": 2 })
        );
        assert_eq!(
            three["groups"][1]["permits"],
            serde_json::json!({ "in_use": 2, "total": 4 })
        );
        assert_eq!(three["groups"][2]["breaker"]["state"], "open");
        assert!(
            three["groups"][2]["breaker"]["retry_at"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );
        assert!(!s.seed_limit_groups("four"));
        assert!(s.seed_limit_groups("default"));
        assert_eq!(s.limits()["groups"][0]["name"], "gateway");
    }
}
