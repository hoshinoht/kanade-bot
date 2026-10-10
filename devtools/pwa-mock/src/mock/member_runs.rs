//! The member portal's own-run writes and the Discord deep-link view
//! (`member-writes-contract` Phase A: `PUT /runs/{id}/answer`,
//! `POST /runs/{id}/move`, `GET /runs/{id}`) for the one mock member, with
//! the server's refusals. Staleness follows the change history
//! (`history.rs`), as the server's `expectations`: a write is `409 stale`
//! only when a record newer than the caller's version touched the one field
//! it expects (the run's slot, or the caller's own RSVP), so other people's
//! edits never refuse it. A retried `Idempotency-Key` answers as the first
//! time; the same key with another body is `422 idempotency_mismatch`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::clock::{iso_date, iso_now, iso_z, valid_time};
use super::dto::Previous;
use super::history::{Actor, Record};
use super::member::{MemberRun, member_run};
use super::seed::{self, Rec};
use super::{MoveError, Store};

/// A member write already answered under an `Idempotency-Key`.
pub struct KeyedWrite {
    member: &'static str,
    key: String,
    fingerprint: String,
    answer: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerBody {
    pub answer: String,
    pub version: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MoveBody {
    pub day: u8,
    #[serde(default)]
    pub time: Option<String>,
    pub version: u64,
}

/// `MemberRunResult`.
#[derive(Serialize)]
pub struct MemberRunResult {
    pub run: MemberRun,
    pub version: u64,
}

/// `MemberMoveResult`.
#[derive(Serialize)]
pub struct MemberMoveResult {
    pub run: MemberRun,
    pub previous: Previous,
    pub version: u64,
}

#[derive(Serialize)]
pub struct Removed {
    pub by: String,
    pub at: String,
}

#[derive(Serialize)]
pub struct Timing {
    /// Weekday, 0 = Monday (as `MemberTiming.weekday`).
    pub day: u8,
    pub time: String,
}

/// `MemberRunLink`: what a Discord deep link opens.
#[derive(Serialize)]
pub struct MemberRunLink {
    pub run: MemberRun,
    pub week: &'static str,
    pub week_starts: String,
    pub week_ends_at: String,
    pub started: bool,
    pub removed: Option<Removed>,
    pub this_week: Option<MemberRun>,
    pub timing: Option<Timing>,
    pub generated_at: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Which {
    Current,
    Next,
    Past,
}

impl Which {
    fn word(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Next => "next",
            Self::Past => "past",
        }
    }
}

/// The field a write expects unchanged since the caller's version.
enum Field<'a> {
    Slot,
    Rsvp(&'a str),
}

/// Last week's runs the link view still finds: Asahi's done Kalos, so a
/// Discord link from that week opens on "That boss week is over".
pub fn past_runs() -> Vec<Rec> {
    let people = seed::people(&[
        ("1001", "yes"),
        ("1002", "yes"),
        ("1005", "yes"),
        ("1006", "yes"),
    ]);
    vec![Rec {
        id: "p-kalos".into(),
        short_id: "0c1d2e3f".into(),
        next_week: false,
        day: 1,
        time: Some("22:00".into()),
        status: "done",
        bosses: super::catalog::boss_ref("XKalos").into_iter().collect(),
        participants: people,
        channel: "kalos-four",
        fixed_id: Some("f-kalos".into()),
    }]
}

fn coded(status: u16, code: &'static str, message: &str) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

pub fn invalid_body() -> MoveError {
    coded(422, "invalid_body", "The request body is not valid.")
}

pub fn idempotency_mismatch() -> MoveError {
    coded(
        422,
        "idempotency_mismatch",
        "That Idempotency-Key was already used for a different request.",
    )
}

fn unknown_run() -> MoveError {
    coded(404, "not_found", "No such run.")
}

fn not_in_run() -> MoveError {
    coded(403, "not_in_run", "You're not in this run.")
}

fn run_closed() -> MoveError {
    coded(409, "run_closed", "That run is done or cancelled.")
}

fn stale() -> MoveError {
    coded(409, "stale", "The run changed since it was loaded.")
}

/// Who made a change, as the member is told: a member's name, "an admin"
/// for a session with no member behind it, else the bot.
fn actor_name(actor: &Actor) -> String {
    let member = |id: &str| seed::member_name(id).map(|m| m.1.to_owned());
    match actor.kind.as_str() {
        "admin" => actor
            .id
            .strip_prefix("discord:")
            .and_then(member)
            .unwrap_or_else(|| "an admin".into()),
        "member" => member(&actor.id).unwrap_or_else(|| "a member".into()),
        _ => "Kanade".into(),
    }
}

fn on(value: &Value, member: &str) -> bool {
    value["participants"]
        .as_array()
        .is_some_and(|ids| ids.iter().any(|id| id == member))
}

/// The record that took `member` off run `id`, newest first.
fn removal<'a>(history: &'a [Record], id: &str, member: &str) -> Option<&'a Record> {
    history.iter().rev().find(|r| {
        r.rows.iter().any(|row| {
            row.key["table"] == "runs"
                && row.key["id"] == id
                && on(&row.before, member)
                && !on(&row.after, member)
        })
    })
}

impl Store {
    /// A run the member routes can name: this or next boss week's, or a
    /// past one the link view still keeps; with its week and first day.
    fn member_find(&self, id: &str) -> Option<(&Rec, Which, i64)> {
        if let Some(rec) = self.runs.iter().find(|r| r.id == id) {
            let which = match (rec.next_week, self.week_over) {
                (true, _) => Which::Next,
                (false, false) => Which::Current,
                (false, true) => Which::Past,
            };
            return Some((rec, which, Self::start(rec.next_week)));
        }
        self.past_runs
            .iter()
            .find(|r| r.id == id)
            .map(|rec| (rec, Which::Past, Self::start(false) - 7))
    }

    /// The local minute a run starts (own-time runs: their day's midnight).
    fn starts_at(rec: &Rec, first_day: i64) -> i64 {
        (first_day + i64::from(rec.day)) * 1440
            + rec.time.as_deref().map_or(0, super::clock::minutes)
    }

    /// A record newer than `version` changed `field` of run `id`.
    fn changed_since(&self, version: u64, id: &str, field: Field) -> bool {
        version > self.version
            || self
                .history
                .iter()
                .filter(|r| r.revision > version)
                .flat_map(|r| &r.rows)
                .any(|row| match field {
                    Field::Slot => {
                        row.key["table"] == "runs"
                            && row.key["id"] == id
                            && row.before["datetime"] != row.after["datetime"]
                    }
                    Field::Rsvp(member) => {
                        row.key["table"] == "rsvps"
                            && row.key["run_id"] == id
                            && row.key["user_id"] == member
                    }
                })
    }

    /// Runs `write` once per `(member, key)`: a retry with the same
    /// fingerprint answers the first answer again.
    pub fn keyed(
        &mut self,
        me: &'static str,
        key: &str,
        fingerprint: String,
        write: impl FnOnce(&mut Self) -> Result<Value, MoveError>,
    ) -> Result<Value, MoveError> {
        if let Some(seen) = self
            .member_writes
            .iter()
            .find(|w| w.member == me && w.key == key)
        {
            return if seen.fingerprint == fingerprint {
                Ok(seen.answer.clone())
            } else {
                Err(idempotency_mismatch())
            };
        }
        let answer = write(self)?;
        self.member_writes.push(KeyedWrite {
            member: me,
            key: key.to_owned(),
            fingerprint,
            answer: answer.clone(),
        });
        Ok(answer)
    }

    fn member_view(&self, rec: &Rec) -> MemberRun {
        member_run(self.dto(rec))
    }

    /// `PUT /api/public/runs/{id}/answer`: the caller's own answer on a run
    /// of this or next boss week, as the admin `rsvp` (a no puts a live run
    /// at risk). No clear from the portal.
    pub fn member_answer(
        &mut self,
        me: &'static str,
        key: &str,
        id: &str,
        body: AnswerBody,
    ) -> Result<Value, MoveError> {
        let fingerprint = format!(
            "answer {id} {}",
            serde_json::to_string(&body).unwrap_or_default()
        );
        self.keyed(me, key, fingerprint, |s| {
            let answer: &'static str = match body.answer.as_str() {
                "yes" => "yes",
                "maybe" => "maybe",
                "no" => "no",
                _ => return Err(invalid_body()),
            };
            let (rec, which, _) = s.member_find(id).ok_or_else(unknown_run)?;
            if which == Which::Past {
                return Err(unknown_run());
            }
            if !rec.participants.iter().any(|p| p.id == me) {
                return Err(not_in_run());
            }
            if matches!(rec.status, "done" | "cancelled") {
                return Err(run_closed());
            }
            if s.changed_since(body.version, id, Field::Rsvp(me)) {
                return Err(stale());
            }
            s.tracked(Actor::new("member", me), "public_portal", |s| {
                let run = s
                    .runs
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or_else(unknown_run)?;
                if let Some(person) = run.participants.iter_mut().find(|p| p.id == me) {
                    person.answer = answer;
                }
                if answer == "no" && matches!(run.status, "planned" | "confirmed") {
                    run.status = "at_risk";
                }
                s.version += 1;
                Ok(())
            })?;
            let rec = s.runs.iter().find(|r| r.id == id).ok_or_else(unknown_run)?;
            Ok(serde_json::to_value(MemberRunResult {
                run: s.member_view(rec),
                version: s.version,
            })
            .unwrap_or_default())
        })
    }

    /// `POST /api/public/runs/{id}/move`: the caller's own run, this boss
    /// week only, before it starts, to a slot after now (as the admin move;
    /// the weekly timing stays). Own-time runs keep their clock on `null`.
    pub fn member_move(
        &mut self,
        me: &'static str,
        key: &str,
        id: &str,
        body: MoveBody,
    ) -> Result<Value, MoveError> {
        let fingerprint = format!(
            "move {id} {}",
            serde_json::to_string(&body).unwrap_or_default()
        );
        self.keyed(me, key, fingerprint, |s| {
            if body.day > 6 {
                return Err(invalid_body());
            }
            let (rec, which, first_day) = s.member_find(id).ok_or_else(unknown_run)?;
            if which == Which::Past {
                return Err(unknown_run());
            }
            if !rec.participants.iter().any(|p| p.id == me) {
                return Err(not_in_run());
            }
            if matches!(rec.status, "done" | "cancelled") {
                return Err(run_closed());
            }
            if which != Which::Current {
                return Err(coded(
                    409,
                    "week_over",
                    "Only this boss week's runs move from the portal.",
                ));
            }
            let now = Self::now_minute();
            if Self::starts_at(rec, first_day) <= now {
                return Err(coded(409, "run_started", "That run has already started."));
            }
            let own_time = rec.status == "otot";
            let time = match (&body.time, own_time) {
                (Some(t), _) if !valid_time(t) => {
                    return Err(coded(
                        422,
                        "invalid_time",
                        "Times are HH:MM, 00:00 to 23:59.",
                    ));
                }
                (None, false) => {
                    return Err(coded(422, "invalid_time", "A scheduled run needs a time."));
                }
                (time, _) => time.clone(),
            };
            let day_start = (first_day + i64::from(body.day)) * 1440;
            let target = match &time {
                Some(t) => day_start + super::clock::minutes(t),
                // An own-time run keeps its clock: the whole day counts.
                None => day_start + 1439,
            };
            if target <= now {
                return Err(coded(422, "in_the_past", "Pick a time that hasn't passed."));
            }
            if s.changed_since(body.version, id, Field::Slot) {
                return Err(stale());
            }
            let previous = Previous {
                day: rec.day,
                time: rec.time.clone(),
            };
            s.tracked(Actor::new("member", me), "public_portal", |s| {
                let run = s
                    .runs
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or_else(unknown_run)?;
                run.day = body.day;
                if time.is_some() {
                    run.time = time.clone();
                }
                s.version += 1;
                Ok(())
            })?;
            let rec = s.runs.iter().find(|r| r.id == id).ok_or_else(unknown_run)?;
            Ok(serde_json::to_value(MemberMoveResult {
                run: s.member_view(rec),
                previous,
                version: s.version,
            })
            .unwrap_or_default())
        })
    }

    /// `GET /api/public/runs/{id}`: any run of this or next boss week, or a
    /// past one the caller is or was on; whether it started, who took the
    /// caller off it, and (past) this week's run of the same timing.
    pub fn member_link(&self, me: &str, id: &str) -> Result<MemberRunLink, MoveError> {
        let (rec, which, first_day) = self.member_find(id).ok_or_else(unknown_run)?;
        let mine = rec.participants.iter().any(|p| p.id == me);
        let removed = (!mine)
            .then(|| removal(&self.history, id, me))
            .flatten()
            .map(|r| Removed {
                by: actor_name(&r.actor),
                at: r.at.clone(),
            });
        if which == Which::Past && !mine && removed.is_none() {
            return Err(unknown_run());
        }
        let this_week = (which == Which::Past)
            .then(|| {
                let fixed = rec.fixed_id.as_deref()?;
                self.runs.iter().find(|r| {
                    !r.next_week && !self.week_over && r.fixed_id.as_deref() == Some(fixed)
                })
            })
            .flatten()
            .map(|r| self.member_view(r));
        let timing = self.fixed_of(rec).filter(|f| !f.retired).map(|f| Timing {
            day: f.weekday,
            time: f.time.clone(),
        });
        Ok(MemberRunLink {
            run: self.member_view(rec),
            week: which.word(),
            week_starts: iso_date(first_day),
            week_ends_at: iso_z((first_day + 7) * 1440),
            started: Self::starts_at(rec, first_day) <= Self::now_minute(),
            removed,
            this_week,
            timing,
            generated_at: iso_now(),
        })
    }

    /// `POST /__mock/public/remove`: an admin (Ren) takes `me` off a run,
    /// as one recorded edit, so the link view can say who did it.
    pub fn remove_member(&mut self, me: &str, id: &str) -> Result<(), MoveError> {
        let index = self
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(unknown_run)?;
        if !self.runs[index].participants.iter().any(|p| p.id == me) {
            return Err(MoveError::invalid("The member is not on that run."));
        }
        self.tracked(Actor::discord_admin("1002"), "admin_portal", |s| {
            s.runs[index].participants.retain(|p| p.id != me);
            s.version += 1;
            Ok(())
        })
    }

    /// `POST /__mock/public/end-week`: this boss week reset while pages were
    /// open; its runs read as past and refuse member writes.
    pub fn end_week(&mut self) {
        self.week_over = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::catalog::Catalog;
    use crate::mock::dto::MoveRequest;

    fn store() -> Store {
        Store::new(Catalog::new(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../web/e2e/fixtures/boss"),
        ))
    }

    fn code<T>(result: Result<T, MoveError>) -> (u16, &'static str) {
        match result {
            Err(MoveError::Coded(status, code, _)) => (status, code),
            Err(other) => panic!("not a coded refusal: {other}"),
            Ok(_) => panic!("not refused"),
        }
    }

    fn answer(answer: &str, version: u64) -> AnswerBody {
        AnswerBody {
            answer: answer.into(),
            version,
        }
    }

    fn to(day: u8, time: Option<&str>, version: u64) -> MoveBody {
        MoveBody {
            day,
            time: time.map(Into::into),
            version,
        }
    }

    #[test]
    fn an_answer_is_the_callers_own_and_replays_by_key() {
        let mut s = store();
        let v = s.version;
        let done = s
            .member_answer("1001", "a-1", "r-carling", answer("no", v))
            .ok()
            .unwrap();
        assert_eq!(
            done["run"]["status"], "at_risk",
            "a no puts a live run at risk"
        );
        assert_eq!(done["version"], v + 1);
        let me = done["run"]["participants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "100000000000001001")
            .cloned()
            .unwrap();
        assert_eq!(me["answer"], "no");
        // The same key answers again without a second write; another body is refused.
        assert_eq!(
            s.member_answer("1001", "a-1", "r-carling", answer("no", v))
                .ok(),
            Some(done)
        );
        assert_eq!(s.version, v + 1);
        assert_eq!(
            code(s.member_answer("1001", "a-1", "r-carling", answer("yes", v))),
            (422, "idempotency_mismatch")
        );
        assert_eq!(
            code(s.member_answer("1001", "a-2", "r-carling", answer("clear", v + 1))),
            (422, "invalid_body")
        );
        assert_eq!(
            code(s.member_answer("1001", "a-3", "r-limbo", answer("yes", v + 1))),
            (403, "not_in_run")
        );
        assert_eq!(
            code(s.member_answer("1001", "a-4", "r-baldrix", answer("yes", v + 1))),
            (409, "run_closed")
        );
        assert_eq!(
            code(s.member_answer("1001", "a-5", "nope", answer("yes", v + 1))),
            (404, "not_found")
        );
        assert_eq!(
            code(s.member_answer("1001", "a-6", "p-kalos", answer("yes", v + 1))),
            (404, "not_found"),
            "last week's run takes no answers"
        );
        // Next week's runs take answers too.
        assert!(
            s.member_answer("1001", "a-7", "n-kalos", answer("maybe", v + 1))
                .is_ok()
        );
    }

    #[test]
    fn only_a_change_to_the_expected_field_is_stale() {
        let mut s = store();
        let v = s.version;
        // Someone else's answer and a move of the run: not the caller's RSVP.
        s.tracked(Actor::admin(), "admin_portal", |s| {
            s.move_run(
                "r-carling",
                MoveRequest {
                    day: 6,
                    time: Some("21:00".into()),
                    version: v,
                },
            )
        })
        .ok()
        .unwrap();
        assert!(
            s.member_answer("1001", "k-1", "r-carling", answer("maybe", v))
                .is_ok()
        );
        // The move did change the slot: a move against the old version is stale.
        assert_eq!(
            code(s.member_move("1001", "k-2", "r-carling", to(6, Some("23:00"), v))),
            (409, "stale")
        );
        let now = s.version;
        assert!(
            s.member_move("1001", "k-3", "r-carling", to(6, Some("23:00"), now))
                .is_ok()
        );
        // An admin answering for the caller makes the caller's answer stale.
        let before = s.version;
        s.tracked(Actor::admin(), "admin_portal", |s| {
            s.rsvp(
                "r-carling",
                crate::mock::dto::RsvpRequest {
                    member_id: "1001".into(),
                    answer: "yes".into(),
                    version: before,
                },
            )
        })
        .ok()
        .unwrap();
        assert_eq!(
            code(s.member_answer("1001", "k-4", "r-carling", answer("no", before))),
            (409, "stale")
        );
    }

    #[test]
    fn a_move_keeps_to_this_boss_week_and_the_future() {
        let mut s = store();
        let v = s.version;
        // Kalos (Fri) has started; next week's Kalos is not this week's.
        assert_eq!(
            code(s.member_move("1001", "m-1", "r-kalos", to(6, Some("21:00"), v))),
            (409, "run_started")
        );
        assert_eq!(
            code(s.member_move("1001", "m-2", "n-kalos", to(2, Some("21:00"), v))),
            (409, "week_over")
        );
        assert_eq!(
            code(s.member_move("1001", "m-3", "r-limbo", to(6, Some("21:00"), v))),
            (403, "not_in_run")
        );
        assert_eq!(
            code(s.member_move("1001", "m-4", "r-carling", to(4, Some("21:00"), v))),
            (422, "in_the_past")
        );
        assert_eq!(
            code(s.member_move("1001", "m-5", "r-carling", to(5, Some("11:00"), v))),
            (422, "in_the_past")
        );
        assert_eq!(
            code(s.member_move("1001", "m-6", "r-carling", to(6, Some("25:00"), v))),
            (422, "invalid_time")
        );
        assert_eq!(
            code(s.member_move("1001", "m-7", "r-carling", to(6, None, v))),
            (422, "invalid_time")
        );
        assert_eq!(
            code(s.member_move("1001", "m-8", "r-carling", to(7, Some("21:00"), v))),
            (422, "invalid_body")
        );
        let moved = s
            .member_move("1001", "m-9", "r-carling", to(6, Some("21:30"), v))
            .ok()
            .unwrap();
        assert_eq!(
            (moved["run"]["day"].clone(), moved["run"]["time"].clone()),
            (6.into(), "21:30".into())
        );
        assert_eq!(
            (
                moved["previous"]["day"].clone(),
                moved["previous"]["time"].clone()
            ),
            (5.into(), "22:00".into())
        );
        let record = s.history.last().unwrap();
        assert_eq!(
            (record.actor.kind.as_str(), record.surface),
            ("member", "public_portal")
        );
        s.end_week();
        assert_eq!(
            code(s.member_move("1001", "m-10", "r-carling", to(6, Some("22:00"), s.version))),
            (404, "not_found")
        );
    }

    #[test]
    fn the_link_view_says_where_the_run_stands() {
        let mut s = store();
        let carling = s.member_link("1001", "r-carling").ok().unwrap();
        assert_eq!((carling.week, carling.started), ("current", false));
        assert_eq!(carling.week_ends_at, "2026-09-30T16:00:00Z");
        assert!(carling.removed.is_none() && carling.this_week.is_none());
        assert_eq!(
            carling.timing.as_ref().map(|t| (t.day, t.time.as_str())),
            Some((1, "22:00"))
        );
        assert!(s.member_link("1001", "r-kalos").ok().unwrap().started);
        assert_eq!(s.member_link("1001", "n-kalos").ok().unwrap().week, "next");
        // Someone else's run this week is visible, view only.
        assert!(!s.member_link("1001", "r-limbo").ok().unwrap().run.mine);
        // Last week's Kalos points at this week's.
        let past = s.member_link("1001", "p-kalos").ok().unwrap();
        assert_eq!(past.week, "past");
        assert_eq!(past.this_week.map(|r| r.id), Some("r-kalos".into()));
        assert!(
            s.member_link("1007", "p-kalos").is_err(),
            "never on it: not found"
        );
        // Taken off by Ren: the link says who and when.
        s.remove_member("1001", "r-carling").ok().unwrap();
        let gone = s.member_link("1001", "r-carling").ok().unwrap();
        assert!(!gone.run.mine);
        assert_eq!(gone.removed.map(|r| r.by), Some("Ren".into()));
        assert_eq!(
            code(s.member_move("1001", "r-1", "r-carling", to(6, Some("21:00"), s.version))),
            (403, "not_in_run")
        );
    }
}
