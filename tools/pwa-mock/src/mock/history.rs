//! Change history in the shape of docs/v5/history.md (`kanade.change.v1`):
//! one record per mutation with row before/after values, rollbacks as new
//! records with `refs`. The hash is a stand-in (FNV), not the real SHA-256
//! canonical encoding; the API shape is what the PWA is built against.

use super::catalog::boss_ref;
use super::clock::{TZ_OFFSET_SECS, clock, iso_date, minutes, now_secs, parse_instant};
use super::dto::Participant;
use super::seed::{self, Rec};
use super::{MoveError, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Actor {
    pub kind: String,
    pub id: String,
}

impl Actor {
    pub fn new(kind: &str, id: &str) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }

    /// The mock's signed-in admin: the break-glass token, as the server
    /// attributes it (`admin:token`).
    pub fn admin() -> Self {
        Self::new("admin", "token")
    }

    /// A Discord-signed-in admin (`admin:discord:<user id>`).
    pub fn discord_admin(user: &str) -> Self {
        Self::new("admin", &format!("discord:{user}"))
    }
}

#[derive(Clone, Serialize)]
pub struct RowChange {
    pub key: Value,
    pub before: Value,
    pub after: Value,
}

#[derive(Clone, Serialize)]
pub struct Ref {
    pub seq: u64,
    pub hash: String,
}

#[derive(Clone, Serialize)]
pub struct Record {
    pub format: &'static str,
    pub seq: u64,
    pub id: String,
    pub revision: u64,
    pub at: String,
    pub actor: Actor,
    pub surface: &'static str,
    pub request_id: Option<String>,
    pub weeks: Vec<String>,
    pub rows: Vec<RowChange>,
    pub notices: Vec<&'static str>,
    pub refs: Vec<Ref>,
    pub prev_hash: String,
    pub hash: String,
}

/// Every schedule row by canonical key text, with the key object alongside.
pub type Rows = BTreeMap<String, (Value, Value)>;

#[derive(Serialize)]
pub struct Conflict {
    pub seq: u64,
    pub key: Value,
    pub expected: Value,
    pub found: Value,
}

#[derive(Serialize)]
pub struct Skipped {
    pub key: Value,
    pub reason: &'static str,
}

#[derive(Serialize)]
pub struct Plan {
    /// `preview`, `applied`, `unchanged` or `conflicts` (strict mode refused).
    pub outcome: &'static str,
    pub reverts: Vec<u64>,
    pub rows: Vec<RowChange>,
    pub conflicts: Vec<Conflict>,
    pub skipped: Vec<Skipped>,
    pub record: Option<Record>,
}

#[derive(Deserialize)]
pub struct Mode {
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub preview: bool,
    pub request_id: Option<String>,
}

#[derive(Serialize)]
pub struct Page {
    pub records: Vec<Record>,
    pub head: Ref,
    pub next_before: Option<u64>,
    pub total: usize,
}

#[derive(Serialize)]
pub struct Blame {
    pub field: String,
    pub value: Value,
    pub seq: u64,
    pub at: String,
    pub actor: Actor,
    pub surface: &'static str,
}

fn fnv(text: &str, seed: u64) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325 ^ seed, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn stand_in_hash(body: &str) -> String {
    (0..4).map(|i| format!("{:016x}", fnv(body, i))).collect()
}

pub fn iso(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    format!(
        "{}T{:02}:{:02}:{:02}+00:00",
        iso_date(days),
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn key_text(key: &Value) -> String {
    key.to_string()
}

fn static_status(s: &str) -> &'static str {
    [
        "planned",
        "confirmed",
        "at_risk",
        "otot",
        "done",
        "cancelled",
    ]
    .into_iter()
    .find(|v| *v == s)
    .unwrap_or("planned")
}

/// A timing's owner as a member id (the mock stores names).
fn owner_id(name: &str) -> &'static str {
    seed::members()
        .into_iter()
        .find(|m| m.name == name)
        .map_or("token", |m| m.id)
}

fn owner(id: &str) -> &'static str {
    seed::member_name(id).map_or("admin token", |m| m.1)
}

/// UTC instant text for an absolute guild-local minute.
fn instant(local_minute: i64) -> String {
    iso(local_minute * 60 - TZ_OFFSET_SECS)
}

/// Absolute guild-local minute of an instant the mock wrote.
fn local_minute(text: &str) -> Option<i64> {
    let utc = parse_instant(&format!("{}Z", text.strip_suffix("+00:00")?))?;
    Some((utc + TZ_OFFSET_SECS).div_euclid(60))
}

/// The domain's reminder kinds for the mock's card labels.
fn reminder_kind(label: &str) -> &'static str {
    match label {
        "T-1h" => "countdown_60",
        "T-15m" => "countdown_15",
        _ => "day_of",
    }
}

/// The run a key belongs to (reminder ids are `{run}:{kind}` here).
pub fn run_of(key: &Value) -> Option<&str> {
    match key["table"].as_str()? {
        "runs" => key["id"].as_str(),
        "rsvps" => key["run_id"].as_str(),
        "reminders" => key["id"].as_str()?.rsplit_once(':').map(|(run, _)| run),
        _ => None,
    }
}

impl Store {
    /// A boss week as records name it: the RFC 3339 instant it starts.
    fn week_of(next: bool) -> String {
        instant(Self::start(next) * 1440)
    }

    /// A week given as the instant or as the guild-local start date (`Week.starts`).
    pub fn week_key(text: &str) -> String {
        if text.len() == 10
            && let Some(utc) = parse_instant(&format!("{text}T00:00:00Z"))
        {
            return iso(utc - TZ_OFFSET_SECS);
        }
        text.to_owned()
    }

    /// Every schedule row as history.md keys and encodes them (domain rows).
    pub fn rows(&self) -> Rows {
        let mut rows = Rows::new();
        for r in &self.runs {
            let week_start = Self::week_of(r.next_week);
            let key = json!({ "table": "runs", "id": r.id });
            let value = json!({
                "id": r.id,
                "fixed_run_id": r.fixed_id,
                "channel_id": r.channel,
                "week_start": week_start,
                "datetime": instant(Self::start_minute(r)),
                "bosses": r.bosses.iter().map(|b| b.token.clone()).collect::<Vec<_>>(),
                "participants": r.participants.iter().map(|p| p.id).collect::<Vec<_>>(),
                "status": r.status,
                "source": if r.fixed_id.is_some() && !self.amended(r) { "fixed" } else { "amend" },
            });
            rows.insert(key_text(&key), (key, value));
            for p in r.participants.iter().filter(|p| p.answer != "waiting") {
                let key = json!({ "table": "rsvps", "run_id": r.id, "user_id": p.id });
                let value = json!({
                    "run_id": r.id,
                    "user_id": p.id,
                    "state": p.answer,
                    "source": "chat",
                    "at": week_start,
                });
                rows.insert(key_text(&key), (key, value));
            }
            // Unsent cards only: sending is the delivery tick's record, not the mock's.
            let day = (Self::start(r.next_week) + i64::from(r.day)) * 1440;
            for card in Self::cards(r) {
                let kind = reminder_kind(card.label);
                let id = format!("{}:{kind}", r.id);
                let key = json!({ "table": "reminders", "id": id });
                let value = json!({
                    "id": id,
                    "run_id": r.id,
                    "kind": kind,
                    "fire_at": instant(day + minutes(&card.at)),
                    "sent_at": null,
                    "message_id": null,
                });
                rows.insert(key_text(&key), (key, value));
            }
        }
        for f in self.fixed.iter().filter(|f| !f.retired) {
            let key = json!({ "table": "fixed_runs", "id": f.id });
            let value = json!({
                "id": f.id,
                "owner_id": owner_id(f.owner),
                "channel_id": f.channel,
                "bosses": f.bosses.iter().map(|b| b.token.clone()).collect::<Vec<_>>(),
                "weekday": f.weekday,
                "time": format!("{}:00", f.time),
                "participants": f.participants,
                "note": f.note,
            });
            rows.insert(key_text(&key), (key, value));
        }
        rows
    }

    fn diff(before: &Rows, after: &Rows) -> Vec<RowChange> {
        let mut keys: Vec<&String> = before.keys().chain(after.keys()).collect();
        keys.sort();
        keys.dedup();
        keys.into_iter()
            .filter_map(|k| {
                let b = before.get(k).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                let a = after.get(k).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                let key = before
                    .get(k)
                    .or_else(|| after.get(k))
                    .map(|(key, _)| key.clone())?;
                (b != a).then_some(RowChange {
                    key,
                    before: b,
                    after: a,
                })
            })
            .collect()
    }

    fn weeks_of(&self, rows: &[RowChange]) -> Vec<String> {
        let mut weeks: Vec<String> = rows
            .iter()
            .filter_map(|row| match row.key["table"].as_str()? {
                "runs" => row.after["week_start"]
                    .as_str()
                    .or(row.before["week_start"].as_str())
                    .map(str::to_owned),
                "rsvps" | "reminders" => {
                    let id = run_of(&row.key)?;
                    let run = self.runs.iter().find(|r| r.id == id)?;
                    Some(Self::week_of(run.next_week))
                }
                _ => None,
            })
            .collect();
        weeks.sort();
        weeks.dedup();
        weeks
    }

    fn push_record(
        &mut self,
        before: &Rows,
        actor: Actor,
        surface: &'static str,
        at: i64,
        refs: Vec<Ref>,
        request_id: Option<String>,
    ) -> Option<Record> {
        let rows = Self::diff(before, &self.rows());
        if rows.is_empty() {
            return None;
        }
        let prev = self
            .history
            .last()
            .map(|r| r.hash.clone())
            .unwrap_or_else(|| "0".repeat(64));
        let seq = self.history.len() as u64;
        let notices = if surface == "rollback" {
            vec!["notice.rollback.reverted"]
        } else {
            Vec::new()
        };
        let mut record = Record {
            format: "kanade.change.v1",
            seq,
            id: format!("00000000-0000-4000-8000-{seq:012}"),
            revision: self.version,
            at: iso(at),
            actor,
            surface,
            request_id,
            weeks: self.weeks_of(&rows),
            rows,
            notices,
            refs,
            prev_hash: prev,
            hash: String::new(),
        };
        record.hash = stand_in_hash(&serde_json::to_string(&record).unwrap_or_default());
        self.history.push(record.clone());
        Some(record)
    }

    /// Runs `change` and appends a record of whatever rows it changed.
    pub fn tracked<T>(
        &mut self,
        actor: Actor,
        surface: &'static str,
        change: impl FnOnce(&mut Self) -> Result<T, MoveError>,
    ) -> Result<T, MoveError> {
        let before = self.rows();
        let value = change(self)?;
        self.push_record(&before, actor, surface, now_secs(), Vec::new(), None);
        Ok(value)
    }

    fn apply_row(&mut self, key: &Value, value: &Value) {
        let text = |v: &Value| v.as_str().map(str::to_owned);
        match key["table"].as_str() {
            Some("runs") => {
                let id = key["id"].as_str().unwrap_or_default();
                let Some(run) = self.runs.iter_mut().find(|r| r.id == id) else {
                    return;
                };
                if value.is_null() {
                    // Runs are never deleted: undoing a creation cancels.
                    run.status = "cancelled";
                    return;
                }
                run.status = static_status(value["status"].as_str().unwrap_or("planned"));
                if let Some(at) = value["datetime"].as_str().and_then(local_minute) {
                    let week = value["week_start"].as_str().unwrap_or_default();
                    run.next_week = week == Self::week_of(true);
                    run.day = (at.div_euclid(1440) - Self::start(run.next_week)).clamp(0, 6) as u8;
                    // An own-time run's instant is its day; its time stays as the mock holds it.
                    if run.status != "otot" {
                        run.time = Some(clock(at.rem_euclid(1440)));
                    }
                }
                run.bosses = value["bosses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| boss_ref(t.as_str()?))
                    .collect();
                run.channel = seed::channel(value["channel_id"].as_str().unwrap_or_default())
                    .map_or(run.channel, |c| c.0);
                run.fixed_id = text(&value["fixed_run_id"]);
                let before = std::mem::take(&mut run.participants);
                run.participants = value["participants"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| {
                        let id = id.as_str()?;
                        before.iter().find(|p| p.id == id).cloned().or_else(|| {
                            seed::member_name(id).map(|(id, name)| Participant {
                                id,
                                name,
                                answer: "waiting",
                            })
                        })
                    })
                    .collect();
            }
            Some("rsvps") => {
                let run_id = key["run_id"].as_str().unwrap_or_default();
                let user = key["user_id"].as_str().unwrap_or_default();
                let answer = match value["state"].as_str() {
                    Some("yes") => "yes",
                    Some("no") => "no",
                    Some("maybe") => "maybe",
                    _ => "waiting",
                };
                if let Some(p) = self
                    .runs
                    .iter_mut()
                    .find(|r| r.id == run_id)
                    .and_then(|r| r.participants.iter_mut().find(|p| p.id == user))
                {
                    p.answer = answer;
                }
            }
            Some("fixed_runs") => {
                let id = key["id"].as_str().unwrap_or_default();
                let Some(f) = self.fixed.iter_mut().find(|f| f.id == id) else {
                    return;
                };
                if value.is_null() {
                    f.retired = true;
                    return;
                }
                f.weekday = value["weekday"].as_u64().unwrap_or(0) as u8;
                f.time = text(&value["time"])
                    .map(|t| t.get(..5).unwrap_or_default().to_owned())
                    .unwrap_or_default();
                f.bosses = value["bosses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| boss_ref(t.as_str()?))
                    .collect();
                f.participants = value["participants"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| seed::member_name(id.as_str()?).map(|m| m.0))
                    .collect();
                f.channel = seed::channel(value["channel_id"].as_str().unwrap_or_default())
                    .map_or(f.channel, |c| c.0);
                f.note = text(&value["note"]);
                f.owner = owner(value["owner_id"].as_str().unwrap_or_default());
                f.retired = false;
            }
            // Reminders follow their run's slot.
            _ => {}
        }
    }

    /// Plan (and unless previewing, apply) the inverse of `records`, newest
    /// first, optionally limited to rows `scope` accepts.
    fn rollback(
        &mut self,
        mut seqs: Vec<u64>,
        scope: impl Fn(&Value, &Self) -> Option<&'static str>,
        mode: &Mode,
    ) -> Plan {
        seqs.sort_unstable_by(|a, b| b.cmp(a));
        seqs.dedup();
        let current = self.rows();
        let mut working = current.clone();
        let mut conflicts = Vec::new();
        let mut skipped: Vec<Skipped> = Vec::new();
        for seq in &seqs {
            let Some(record) = self.history.get(*seq as usize) else {
                continue;
            };
            for row in &record.rows {
                if let Some(reason) = scope(&row.key, self) {
                    if !skipped.iter().any(|s| s.key == row.key) {
                        skipped.push(Skipped {
                            key: row.key.clone(),
                            reason,
                        });
                    }
                    continue;
                }
                let k = key_text(&row.key);
                let found = working
                    .get(&k)
                    .map(|(_, v)| v.clone())
                    .unwrap_or(Value::Null);
                if found != row.after {
                    conflicts.push(Conflict {
                        seq: *seq,
                        key: row.key.clone(),
                        expected: row.after.clone(),
                        found,
                    });
                }
                if row.before.is_null() {
                    working.remove(&k);
                } else {
                    working.insert(k, (row.key.clone(), row.before.clone()));
                }
            }
        }
        let rows = Self::diff(&current, &working);
        let outcome = if rows.is_empty() {
            "unchanged"
        } else if !conflicts.is_empty() && !mode.force {
            "conflicts"
        } else if mode.preview {
            "preview"
        } else {
            "applied"
        };
        if outcome == "conflicts" {
            // As the server: a strict refusal plans no rows and names every
            // selected record (newest first), not only the conflicting ones.
            return Plan {
                outcome,
                reverts: seqs,
                rows: Vec::new(),
                conflicts,
                skipped: Vec::new(),
                record: None,
            };
        }
        let mut record = None;
        if outcome == "applied" {
            for row in &rows {
                self.apply_row(&row.key, &row.after);
            }
            self.version += 1;
            let refs = seqs
                .iter()
                .filter_map(|s| self.history.get(*s as usize))
                .map(|r| Ref {
                    seq: r.seq,
                    hash: r.hash.clone(),
                })
                .collect();
            record = self.push_record(
                &current,
                Actor::admin(),
                "rollback",
                now_secs(),
                refs,
                mode.request_id.clone(),
            );
        }
        Plan {
            outcome,
            reverts: seqs,
            rows,
            conflicts,
            skipped,
            record,
        }
    }

    fn check_seqs(&self, seqs: &[u64]) -> Result<(), MoveError> {
        if seqs.is_empty() {
            return Err(MoveError::invalid("Choose at least one change."));
        }
        if let Some(bad) = seqs
            .iter()
            .find(|s| **s == 0 || **s as usize >= self.history.len())
        {
            return Err(MoveError::Invalid(format!("No change #{bad}.")));
        }
        Ok(())
    }

    pub fn revert_changes(&mut self, seqs: Vec<u64>, mode: &Mode) -> Result<Plan, MoveError> {
        self.check_seqs(&seqs)?;
        Ok(self.rollback(seqs, |_, _| None, mode))
    }

    /// Every later change touching `week`, limited to that week's runs and RSVPs.
    pub fn restore_week_to(
        &mut self,
        week: &str,
        revision: u64,
        mode: &Mode,
    ) -> Result<Plan, MoveError> {
        let week = Self::week_key(week);
        let seqs: Vec<u64> = self
            .history
            .iter()
            .filter(|r| r.seq > 0 && r.revision > revision && r.weeks.contains(&week))
            .map(|r| r.seq)
            .collect();
        let in_week = move |key: &Value, store: &Self| -> Option<&'static str> {
            let Some(run_id) = run_of(key) else {
                return Some("outside week");
            };
            let run = store.runs.iter().find(|r| r.id == run_id)?;
            (Self::week_of(run.next_week) != week).then_some("outside week")
        };
        Ok(self.rollback(seqs, in_week, mode))
    }

    pub fn revert_by_actor(
        &mut self,
        actor: &Actor,
        since: &str,
        mode: &Mode,
    ) -> Result<Plan, MoveError> {
        let seqs = self
            .history
            .iter()
            .filter(|r| r.seq > 0 && &r.actor == actor && r.at.as_str() >= since)
            .map(|r| r.seq)
            .collect();
        Ok(self.rollback(seqs, |_, _| None, mode))
    }

    fn head(&self) -> Ref {
        let last = self.history.last();
        Ref {
            seq: last.map_or(0, |r| r.seq),
            hash: last.map_or_else(|| "0".repeat(64), |r| r.hash.clone()),
        }
    }

    /// Newest first, `limit` per page, optionally for one week or one actor.
    pub fn history_page(
        &self,
        week: Option<&str>,
        actor: Option<&Actor>,
        before: Option<u64>,
        limit: usize,
    ) -> Page {
        let week = week.map(Self::week_key);
        let matching: Vec<&Record> = self
            .history
            .iter()
            .rev()
            .filter(|r| r.seq > 0)
            .filter(|r| week.as_ref().is_none_or(|w| r.weeks.contains(w)))
            .filter(|r| actor.is_none_or(|a| &r.actor == a))
            .collect();
        let total = matching.len();
        let page: Vec<Record> = matching
            .into_iter()
            .filter(|r| before.is_none_or(|b| r.seq < b))
            .take(limit + 1)
            .cloned()
            .collect();
        let next_before = (page.len() > limit).then(|| page[limit - 1].seq);
        Page {
            records: page.into_iter().take(limit).collect(),
            head: self.head(),
            next_before,
            total,
        }
    }

    pub fn record(&self, seq: u64) -> Option<Record> {
        self.history.get(seq as usize).cloned()
    }

    /// Whether a record after `version` changed this row field (the server's
    /// per-field precondition behind `409 stale`).
    pub fn changed_after(&self, key: &Value, field: &str, version: u64) -> bool {
        self.history.iter().rev().any(|r| {
            r.revision > version
                && r.rows
                    .iter()
                    // Creating the row sets every field.
                    .any(|row| {
                        row.key == *key
                            && (row.before.is_null() || row.before[field] != row.after[field])
                    })
        })
    }

    /// Who last changed each field of a run, under the domain's blame names
    /// (`slot`, `bosses`, `participants`, `channel`, `status`, `rsvp:<id>`).
    pub fn blame(&self, run_id: &str) -> Vec<Blame> {
        let mut out: BTreeMap<String, Blame> = BTreeMap::new();
        for record in self.history.iter().filter(|r| r.seq > 0) {
            for row in &record.rows {
                let mut note = |field: String, value: Value| {
                    out.insert(
                        field.clone(),
                        Blame {
                            field,
                            value,
                            seq: record.seq,
                            at: record.at.clone(),
                            actor: record.actor.clone(),
                            surface: record.surface,
                        },
                    );
                };
                match row.key["table"].as_str() {
                    Some("runs") if row.key["id"] == run_id => {
                        let pick = |row: &Value, keys: &[&str]| -> Value {
                            keys.iter()
                                .map(|k| {
                                    ((*k).to_owned(), row.get(*k).cloned().unwrap_or(Value::Null))
                                })
                                .collect::<serde_json::Map<_, _>>()
                                .into()
                        };
                        let slot = ["datetime", "week_start", "source", "fixed_run_id"];
                        let fields: [(&str, Value, Value); 5] = [
                            ("slot", pick(&row.before, &slot), pick(&row.after, &slot)),
                            (
                                "bosses",
                                row.before["bosses"].clone(),
                                row.after["bosses"].clone(),
                            ),
                            (
                                "participants",
                                row.before["participants"].clone(),
                                row.after["participants"].clone(),
                            ),
                            (
                                "channel",
                                row.before["channel_id"].clone(),
                                row.after["channel_id"].clone(),
                            ),
                            (
                                "status",
                                row.before["status"].clone(),
                                row.after["status"].clone(),
                            ),
                        ];
                        for (field, before, after) in fields {
                            if row.before.is_null() || before != after {
                                note(field.to_owned(), after);
                            }
                        }
                    }
                    Some("rsvps") if row.key["run_id"] == run_id => {
                        let user = row.key["user_id"].as_str().unwrap_or_default();
                        note(format!("rsvp:{user}"), row.after.clone());
                    }
                    _ => {}
                }
            }
        }
        out.into_values().collect()
    }

    pub fn checkpoints(&self) -> Value {
        let at = |seq: u64| {
            self.history
                .get(seq as usize)
                .map(|r| (r.revision, r.hash.clone(), r.at.clone()))
        };
        let backup = |seq: u64| {
            at(seq).map(|(revision, hash, when)| {
                json!({
                    "file": format!("kanade-{}.sqlite", &when[..10]),
                    "format": "kanade.backup.v1",
                    "created_at": when,
                    "history_head": { "seq": seq, "hash": hash },
                    "revision": revision,
                    "schema_version": 1,
                    "anchored": true,
                })
            })
        };
        let backups: Vec<Value> = [backup(3), backup(1)].into_iter().flatten().collect();
        json!({
            "verified": { "ok": true, "checked": self.history.len(), "head": self.head() },
            "backups": backups,
        })
    }

    /// A believable past for the seed week: an earlier state, then the changes
    /// that led to the seed, attributed as the live service would.
    pub fn seed_history(&mut self) {
        let start = Self::start(false) * 86_400 - 8 * 3600;
        let hour = |h: i64| start + h * 3600;
        let set = |s: &mut Self, id: &str, f: &dyn Fn(&mut Rec)| {
            if let Some(r) = s.runs.iter_mut().find(|r| r.id == id) {
                f(r);
            }
        };
        // The state before the week's first change.
        set(self, "r-kalos", &|r| {
            r.time = Some("21:30".into());
            r.status = "planned";
            if let Some(p) = r.participants.iter_mut().find(|p| p.id == "1005") {
                p.answer = "waiting";
            }
        });
        set(self, "r-carling", &|r| {
            r.participants.retain(|p| p.id != "1013");
            if let Some(p) = r.participants.iter_mut().find(|p| p.id == "1011") {
                p.answer = "waiting";
            }
        });
        set(self, "r-seren", &|r| r.status = "planned");
        set(self, "r-baldrix", &|r| r.status = "confirmed");
        let genesis = Record {
            format: "kanade.change.v1",
            seq: 0,
            id: "genesis".into(),
            revision: self.version,
            at: "1970-01-01T00:00:00+00:00".into(),
            actor: Actor::new("system", "store"),
            surface: "import",
            request_id: None,
            weeks: Vec::new(),
            rows: Vec::new(),
            notices: Vec::new(),
            refs: Vec::new(),
            prev_hash: "0".repeat(64),
            hash: stand_in_hash("genesis"),
        };
        self.history = vec![genesis];

        let step =
            |s: &mut Self, at: i64, actor: Actor, surface: &'static str, f: &dyn Fn(&mut Self)| {
                let before = s.rows();
                f(s);
                s.version += 1;
                s.push_record(&before, actor, surface, at, Vec::new(), None);
            };
        step(
            self,
            hour(22),
            Actor::new("system", "delivery"),
            "delivery_tick",
            &|s| set(s, "r-baldrix", &|r| r.status = "done"),
        );
        step(
            self,
            hour(36),
            Actor::new("member", "1005"),
            "discord",
            &|s| {
                set(s, "r-kalos", &|r| {
                    r.status = "at_risk";
                    if let Some(p) = r.participants.iter_mut().find(|p| p.id == "1005") {
                        p.answer = "no";
                    }
                })
            },
        );
        step(
            self,
            hour(41),
            // Asahi (staff) approved it after signing in with Discord.
            Actor::discord_admin("1001"),
            "extraction_approval",
            &|s| set(s, "r-kalos", &|r| r.time = Some("22:00".into())),
        );
        step(self, hour(58), Actor::admin(), "admin_portal", &|s| {
            set(s, "r-carling", &|r| {
                r.participants.push(Participant {
                    id: "1013",
                    name: "Ren",
                    answer: "waiting",
                })
            })
        });
        step(
            self,
            hour(61),
            Actor::new("member", "1011"),
            "discord",
            &|s| {
                set(s, "r-carling", &|r| {
                    if let Some(p) = r.participants.iter_mut().find(|p| p.id == "1011") {
                        p.answer = "maybe";
                    }
                })
            },
        );
        step(
            self,
            hour(62),
            Actor::new("member", "1013"),
            "discord",
            &|s| {
                set(s, "r-carling", &|r| {
                    if let Some(p) = r.participants.iter_mut().find(|p| p.id == "1013") {
                        p.answer = "maybe";
                    }
                })
            },
        );
        step(
            self,
            hour(80),
            Actor::new("member", "1010"),
            "discord",
            &|s| set(s, "r-seren", &|r| r.status = "cancelled"),
        );
        // An admin moved FA by mistake and rolled it back.
        step(self, hour(90), Actor::admin(), "admin_portal", &|s| {
            set(s, "r-fa", &|r| r.day = 5)
        });
        let before = self.rows();
        set(self, "r-fa", &|r| r.day = 4);
        self.version += 1;
        let refs = vec![Ref {
            seq: 8,
            hash: self.history[8].hash.clone(),
        }];
        self.push_record(
            &before,
            Actor::admin(),
            "rollback",
            hour(90) + 300,
            refs,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use super::{Actor, Mode};
    use crate::mock::dto::MoveRequest;

    fn mode(force: bool, preview: bool) -> Mode {
        Mode {
            force,
            preview,
            request_id: None,
        }
    }

    #[test]
    fn seeded_history_is_a_chain_ending_in_a_rollback() {
        let s = store();
        let page = s.history_page(None, None, None, 50);
        assert_eq!(page.head.seq, 9);
        let latest = &page.records[0];
        assert_eq!(latest.surface, "rollback");
        assert_eq!(latest.refs[0].seq, 8);
        assert!(s.history.windows(2).all(|w| w[1].prev_hash == w[0].hash));
    }

    #[test]
    fn mutations_record_rows_and_revert_strictly_or_by_force() {
        let mut s = store();
        let v = s.version;
        s.tracked(Actor::admin(), "admin_portal", |s| {
            s.move_run(
                "r-limbo",
                MoveRequest {
                    day: 2,
                    time: Some("20:00".into()),
                    version: v,
                },
            )
        })
        .ok()
        .unwrap();
        let seq = s.history.last().unwrap().seq;
        // The run row, and its reminders re-placed with it (as the server records them).
        let rows = &s.history.last().unwrap().rows;
        let tables: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.key["table"].as_str())
            .collect();
        assert_eq!(tables, ["runs", "reminders", "reminders", "reminders"]);
        assert!(
            rows.iter()
                .all(|r| super::run_of(&r.key) == Some("r-limbo"))
        );

        let preview = s
            .revert_changes(vec![seq], &mode(false, true))
            .ok()
            .unwrap();
        assert_eq!(preview.outcome, "preview");
        assert_eq!(
            s.history.last().unwrap().seq,
            seq,
            "a preview writes nothing"
        );

        // Someone moves it again: the strict revert now conflicts.
        let v = s.version;
        s.tracked(Actor::admin(), "admin_portal", |s| {
            s.move_run(
                "r-limbo",
                MoveRequest {
                    day: 3,
                    time: Some("20:00".into()),
                    version: v,
                },
            )
        })
        .ok()
        .unwrap();
        let strict = s
            .revert_changes(vec![seq], &mode(false, false))
            .ok()
            .unwrap();
        assert_eq!(strict.outcome, "conflicts");
        assert_eq!(
            (strict.reverts.as_slice(), strict.rows.len()),
            (&[seq][..], 0)
        );
        // The run row and the reminders that followed it moved again.
        assert!(
            strict
                .conflicts
                .iter()
                .all(|c| super::run_of(&c.key) == Some("r-limbo"))
        );
        assert!(strict.conflicts.iter().any(|c| c.key["table"] == "runs"));
        let forced = s
            .revert_changes(vec![seq], &mode(true, false))
            .ok()
            .unwrap();
        assert_eq!(forced.outcome, "applied");
        assert_eq!(forced.record.as_ref().unwrap().refs[0].seq, seq);
        let limbo = s
            .week(false)
            .runs
            .into_iter()
            .find(|r| r.id == "r-limbo")
            .unwrap();
        assert_eq!((limbo.day, limbo.time.as_deref()), (1, Some("23:30")));
    }

    #[test]
    fn restore_week_and_revert_by_member() {
        let mut s = store();
        let week = s.history[2].weeks[0].clone();
        let plan = s
            .restore_week_to(&week, s.history[1].revision, &mode(false, true))
            .ok()
            .unwrap();
        assert_eq!(plan.outcome, "preview");
        assert!(plan.rows.iter().any(|r| r.key["id"] == "r-kalos"));

        let member = Actor::new("member", "1005");
        // The run row changed again after the member's change (time moved by an
        // admin), so strict refuses and names the conflict; force overrides it.
        let strict = s
            .revert_by_actor(&member, "1970-01-01", &mode(false, false))
            .ok()
            .unwrap();
        assert_eq!(strict.outcome, "conflicts");
        assert!(
            strict
                .conflicts
                .iter()
                .all(|c| super::run_of(&c.key) == Some("r-kalos"))
        );
        assert!(strict.rows.is_empty());
        // Every selected record, even those with no conflict of their own.
        let selected: Vec<u64> = s
            .history
            .iter()
            .rev()
            .filter(|r| r.seq > 0 && r.actor == member)
            .map(|r| r.seq)
            .collect();
        assert_eq!(strict.reverts, selected);
        let plan = s
            .revert_by_actor(&member, "1970-01-01", &mode(true, false))
            .ok()
            .unwrap();
        assert_eq!(plan.outcome, "applied");
        let kalos = s
            .week(false)
            .runs
            .into_iter()
            .find(|r| r.id == "r-kalos")
            .unwrap();
        assert_eq!(
            kalos
                .participants
                .iter()
                .find(|p| p.id == "1005")
                .unwrap()
                .answer,
            "waiting"
        );
    }

    #[test]
    fn blame_names_the_last_change_per_field() {
        let s = store();
        let blame = s.blame("r-kalos");
        let slot = blame.iter().find(|b| b.field == "slot").unwrap();
        assert_eq!(slot.surface, "extraction_approval");
        assert_eq!(slot.actor, Actor::discord_admin("1001"));
        assert!(slot.value["datetime"].as_str().unwrap().ends_with("+00:00"));
        let answer = blame.iter().find(|b| b.field == "rsvp:1005").unwrap();
        assert_eq!(answer.actor, Actor::new("member", "1005"));
        assert_eq!(answer.value["state"], "no");
    }
}
