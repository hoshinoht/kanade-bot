//! Limits (v5): model backend groups behind the Kanata gateway, their
//! permits, queue, rate bucket, retry budget and breaker, and the gateway's
//! admission refusals by kind. Synthetic, shaped for the admin view.

use super::seed;
use super::{MoveError, Store};
use serde_json::{Value, json};

impl Store {
    pub fn limits(&self) -> Value {
        let minute = Self::now_minute();
        let ago = |m: i64| Self::when(minute - m);
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
        json!({
            "groups": [
                {
                    "name": "extract", "backend": "Kanata", "models": ["kanata/extract"],
                    "permits": { "in_use": 1, "total": 1 },
                    "queue": [
                        { "position": 1, "kind": "rescan", "who": "admin token", "waiting_s": 42 },
                        { "position": 2, "kind": "extraction", "who": "#hstar-party burst", "waiting_s": 8 },
                    ],
                    "rate": { "available": 9, "capacity": 12, "refill_per_min": 2 },
                    "retry": { "remaining": 5, "capacity": 5 },
                    "breaker": { "state": "closed", "failures": 0, "since": ago(600) },
                },
                {
                    "name": "chat", "backend": "Kanata", "models": ["kanata/chat"],
                    "permits": { "in_use": 2, "total": 4 },
                    "queue": [],
                    "rate": { "available": 1, "capacity": 20, "refill_per_min": 4 },
                    "retry": { "remaining": 2, "capacity": 5 },
                    "breaker": { "state": "half_open", "failures": 3, "since": ago(4) },
                },
                {
                    "name": "rewrite", "backend": "Kanata", "models": ["kanata/rewrite"],
                    "permits": { "in_use": 0, "total": 1 },
                    "queue": [],
                    "rate": { "available": 6, "capacity": 6, "refill_per_min": 1 },
                    "retry": { "remaining": 0, "capacity": 3 },
                    "breaker": { "state": "open", "failures": 5, "since": ago(12), "retry_at": Self::when(minute + 3) },
                },
            ],
            "admission": {
                "window": "last hour",
                "refusals": [
                    { "kind": "rate", "scope": "group", "target": "chat", "count": 3, "last_at": ago(6) },
                    { "kind": "concurrency", "scope": "group", "target": "extract", "count": 1, "last_at": ago(41) },
                    { "kind": "quota", "scope": "key", "target": "gateway key …7f2a", "count": 1, "last_at": ago(22) },
                    { "kind": "key_rate", "scope": "key", "target": "gateway key …7f2a", "count": 2, "last_at": ago(9) },
                ],
            },
            "allowances": allowances,
        })
    }

    pub fn reset_window(&mut self, id: &str) -> Result<Value, MoveError> {
        let (id, name) = seed::member_name(id).ok_or(MoveError::NotFound)?;
        if !self.limit_resets.contains(&id) {
            self.limit_resets.push(id);
        }
        Ok(json!({ "message": format!("{name}'s window is reset.") }))
    }
}
