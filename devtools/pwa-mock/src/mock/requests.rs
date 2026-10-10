//! Member requests from the portal (`member-writes-contract` Phase B): the
//! caller's own list (`GET /api/public/requests/mine`), submit
//! (`POST /api/public/requests`) and withdraw, with the server's limits
//! (3 undecided, 6 per rolling 24 h, withdrawn ones counted) and refusals.
//! The request id comes from the member and the `Idempotency-Key` (as the
//! ownership `ask_id`), so an exact retry finds the first request. Seeded to
//! match the boards: one waiting weekly change, then approved, rejected,
//! expired and withdrawn ones.

use serde::{Deserialize, Serialize};
use std::hash::{DefaultHasher, Hash, Hasher};

use super::catalog::{BossRef, boss_ref, parse_bosses};
use super::clock::{TZ_OFFSET_SECS, iso_secs, now_secs, valid_time};
use super::dto::{Boss, Named};
use super::member::MemberRun;
use super::member_runs::{idempotency_mismatch, invalid_body};
use super::portal::snowflake;
use super::seed;
use super::{MoveError, Store};

/// RequestLimits: undecided at once, and sent per rolling 24 h.
pub const MAX_OPEN: usize = 3;
pub const MAX_TODAY: usize = 6;
const DAY_SECS: i64 = 24 * 3600;

#[derive(Clone)]
pub struct Proposed {
    /// Weekday, 0 = Monday.
    day: Option<u8>,
    time: Option<String>,
    channel: Option<&'static str>,
    party: Option<Vec<&'static str>>,
}

#[derive(Clone)]
pub struct MemberRequestRec {
    id: String,
    member: &'static str,
    fingerprint: String,
    kind: &'static str,
    /// `waiting`, `approved`, `rejected` or `withdrawn`; a waiting one past
    /// `expires` reads as `expired`.
    state: &'static str,
    run_id: Option<String>,
    fixed_id: Option<String>,
    with: Option<&'static str>,
    proposed: Option<Proposed>,
    bosses: Vec<BossRef>,
    channel: Option<&'static str>,
    note: Option<String>,
    summary: String,
    sent: i64,
    decided: Option<i64>,
    decided_by: Option<String>,
    reason: Option<String>,
    expires: Option<i64>,
}

#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RequestBody {
    pub kind: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub fixed_id: Option<String>,
    #[serde(default)]
    pub with: Option<String>,
    #[serde(default)]
    pub day: Option<u8>,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub party: Option<Vec<String>>,
    #[serde(default)]
    pub bosses: Option<Vec<String>>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct ProposedDto {
    pub day: Option<u8>,
    pub time: Option<String>,
    pub channel: Option<String>,
    pub party: Option<Vec<Named>>,
}

/// `MemberRequest`.
#[derive(Serialize)]
pub struct MemberRequest {
    pub id: String,
    pub kind: &'static str,
    pub state: &'static str,
    pub summary: String,
    pub note: Option<String>,
    pub run: Option<MemberRun>,
    pub fixed_id: Option<String>,
    pub bosses: Vec<Boss>,
    pub channel: Option<String>,
    pub with: Option<Named>,
    pub proposed: Option<ProposedDto>,
    pub sent_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Serialize)]
pub struct MemberRequestOptions {
    pub channels: Vec<Named>,
    pub members: Vec<Named>,
}

/// `MemberRequests`.
#[derive(Serialize)]
pub struct MemberRequests {
    pub requests: Vec<MemberRequest>,
    pub open: usize,
    pub today: usize,
    pub max_open: usize,
    pub max_today: usize,
    pub options: MemberRequestOptions,
    pub generated_at: String,
}

/// Why a submit was refused: a coded refusal, or a limit (`429
/// request_limit` with `limit: open | today`).
pub enum Refused {
    Coded(MoveError),
    Limit(&'static str),
}

impl From<MoveError> for Refused {
    fn from(error: MoveError) -> Self {
        Self::Coded(error)
    }
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn coded(status: u16, code: &'static str, message: &str) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

fn not_found() -> MoveError {
    coded(404, "not_found", "No such run or weekly timing.")
}

fn field_not_allowed(field: &str) -> MoveError {
    MoveError::Coded(
        422,
        "field_not_allowed",
        format!("This kind of request does not take {field}."),
    )
}

fn no_effect() -> MoveError {
    coded(409, "no_effect", "That would change nothing.")
}

fn already_in_party() -> MoveError {
    coded(409, "already_in_party", "They are already in that party.")
}

fn digest(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The request id `member` submits with `key` (the server's is a SHA-256 prefix).
fn request_id(member: &str, key: &str) -> String {
    format!("req-{:016x}", digest(&format!("request:{member}:{key}")))
}

/// A seed id for a snowflake-shaped member id on the bossing roster.
fn roster_member(id: &str) -> Option<&'static str> {
    seed::members()
        .into_iter()
        .find(|m| m.bossing && snowflake(m.id) == id)
        .map(|m| m.id)
}

fn member_named(id: &str) -> Named {
    Named {
        id: snowflake(id),
        name: seed::member_name(id).map_or(id, |m| m.1).into(),
    }
}

fn tokens(bosses: &[BossRef]) -> String {
    bosses
        .iter()
        .map(|b| b.token.as_str())
        .collect::<Vec<_>>()
        .join(" + ")
}

impl MemberRequestRec {
    fn shown_state(&self, now: i64) -> &'static str {
        match (self.state, self.expires) {
            ("waiting", Some(at)) if now >= at => "expired",
            (state, _) => state,
        }
    }

    fn open(&self, now: i64) -> bool {
        self.shown_state(now) == "waiting"
    }
}

/// The boards' history, relative to the mock's now (Tue 12:00 by default).
pub fn seed() -> Vec<MemberRequestRec> {
    let now = now_secs();
    let (this_reset, last_reset) = {
        let start = Store::start(false);
        (
            (start + 7) * 86_400 - TZ_OFFSET_SECS,
            start * 86_400 - TZ_OFFSET_SECS,
        )
    };
    let bosses = |token: &str| token.split(' ').filter_map(boss_ref).collect::<Vec<_>>();
    let base =
        |id: &str, kind: &'static str, state: &'static str, hours_ago: i64| MemberRequestRec {
            id: id.into(),
            member: "1001",
            fingerprint: String::new(),
            kind,
            state,
            run_id: None,
            fixed_id: None,
            with: None,
            proposed: None,
            bosses: Vec::new(),
            channel: None,
            note: None,
            summary: String::new(),
            sent: now - hours_ago * 3600,
            decided: None,
            decided_by: None,
            reason: None,
            expires: Some(this_reset),
        };
    vec![
        MemberRequestRec {
            fixed_id: Some("f-kalos".into()),
            proposed: Some(Proposed {
                day: None,
                time: Some("21:00".into()),
                channel: None,
                party: None,
            }),
            bosses: bosses("XKalos"),
            channel: Some("kalos-four"),
            note: Some("21:00 suits Ren and Tsubame better on Fridays.".into()),
            summary: "member request: change_fixed XKalos Fri 21:00".into(),
            sent: now - 58 * 60,
            ..base("req-kalos-weekly", "change_fixed", "waiting", 0)
        },
        MemberRequestRec {
            run_id: Some("r-jupiter".into()),
            fixed_id: Some("f-jupiter".into()),
            bosses: bosses("HJupiter"),
            channel: Some("jupiter-trio"),
            summary: "member request: join HJupiter Mon 28 Sep 21:00".into(),
            decided: Some(now - 70 * 3600),
            decided_by: Some("Ren".into()),
            ..base("req-jupiter-join", "join", "approved", 80)
        },
        MemberRequestRec {
            run_id: Some("r-bellona".into()),
            bosses: bosses("NBellona"),
            channel: Some("bellona-otot"),
            summary: "member request: join NBellona Mon 28 Sep".into(),
            decided: Some(now - 90 * 3600),
            decided_by: Some("Ren".into()),
            reason: Some("party is full".into()),
            ..base("req-bellona-join", "join", "rejected", 100)
        },
        // Last week's Carling: the run is gone; the timing runs again this week.
        MemberRequestRec {
            run_id: Some("p-carling".into()),
            fixed_id: Some("f-carling".into()),
            with: Some("1009"),
            bosses: bosses("HCarling HStar"),
            channel: Some("hstar-party"),
            note: Some("Kaito offered to take my seat that night.".into()),
            summary: "member request: swap HCarling + HStar Tue 22 Sep 22:00".into(),
            expires: Some(last_reset),
            ..base("req-carling-swap", "swap", "waiting", 8 * 24)
        },
        MemberRequestRec {
            proposed: Some(Proposed {
                day: Some(0),
                time: Some("21:00".into()),
                channel: Some("jupiter-trio"),
                party: Some(vec!["1001", "1008"]),
            }),
            bosses: bosses("HJupiter"),
            channel: Some("jupiter-trio"),
            summary: "member request: new_fixed HJupiter Mon 21:00".into(),
            decided: Some(now - 110 * 3600),
            decided_by: Some("Asahi".into()),
            expires: None,
            ..base("req-jupiter-new", "new_fixed", "withdrawn", 120)
        },
    ]
}

impl Store {
    fn request_view(&self, r: &MemberRequestRec, now: i64) -> MemberRequest {
        let run = r
            .run_id
            .as_deref()
            .and_then(|id| self.runs.iter().find(|run| run.id == id))
            .map(|rec| super::member::member_run(self.dto(rec)));
        let channel = |id: &str| seed::channel(id).map_or(id, |c| c.1).to_owned();
        MemberRequest {
            id: r.id.clone(),
            kind: r.kind,
            state: r.shown_state(now),
            summary: r.summary.clone(),
            note: r.note.clone(),
            run,
            fixed_id: r.fixed_id.clone(),
            bosses: self.bosses(&r.bosses),
            channel: r.channel.map(channel),
            with: r.with.map(member_named),
            proposed: r.proposed.as_ref().map(|p| ProposedDto {
                day: p.day,
                time: p.time.clone(),
                channel: p.channel.map(channel),
                party: p
                    .party
                    .as_ref()
                    .map(|ids| ids.iter().map(|id| member_named(id)).collect()),
            }),
            sent_at: iso_secs(r.sent),
            decided_at: r.decided.map(iso_secs),
            decided_by: r.decided_by.clone(),
            reason: r.reason.clone(),
            expires_at: r.expires.map(iso_secs),
        }
    }

    fn counts(&self, me: &str, now: i64) -> (usize, usize) {
        let mine = || self.member_requests.iter().filter(move |r| r.member == me);
        (
            mine().filter(|r| r.open(now)).count(),
            mine().filter(|r| r.sent > now - DAY_SECS).count(),
        )
    }

    /// `GET /api/public/requests/mine`: newest first, with the limits and the
    /// form's choices (party channels, the bossing roster but the caller).
    pub fn member_requests(&self, me: &str) -> MemberRequests {
        let now = now_secs();
        let mut mine: Vec<&MemberRequestRec> = self
            .member_requests
            .iter()
            .filter(|r| r.member == me)
            .collect();
        mine.sort_by(|a, b| (b.sent, &b.id).cmp(&(a.sent, &a.id)));
        let (open, today) = self.counts(me, now);
        MemberRequests {
            requests: mine
                .into_iter()
                .map(|r| self.request_view(r, now))
                .collect(),
            open,
            today,
            max_open: MAX_OPEN,
            max_today: MAX_TODAY,
            options: MemberRequestOptions {
                channels: Self::channels(),
                members: seed::members()
                    .into_iter()
                    .filter(|m| m.bossing && m.id != me)
                    .map(|m| member_named(m.id))
                    .collect(),
            },
            generated_at: iso_secs(now),
        }
    }

    /// `POST /api/public/requests`: `true` with a new request; an exact
    /// retry of the same key finds it as it now is (`false`).
    pub fn member_request(
        &mut self,
        me: &'static str,
        key: &str,
        body: RequestBody,
    ) -> Result<(bool, MemberRequest), Refused> {
        let now = now_secs();
        let id = request_id(me, key);
        let fingerprint = serde_json::to_string(&body).unwrap_or_default();
        if let Some(found) = self.member_requests.iter().find(|r| r.id == id) {
            if found.member != me || found.fingerprint != fingerprint {
                return Err(idempotency_mismatch().into());
            }
            return Ok((false, self.request_view(found, now)));
        }
        let mut rec = self.validate(me, &body)?;
        let (open, today) = self.counts(me, now);
        if open >= MAX_OPEN {
            return Err(Refused::Limit("open"));
        }
        if today >= MAX_TODAY {
            return Err(Refused::Limit("today"));
        }
        rec.id = id;
        rec.fingerprint = fingerprint;
        rec.sent = now;
        let view = self.request_view(&rec, now);
        self.member_requests.push(rec);
        Ok((true, view))
    }

    /// The request a valid body makes (id and time are the caller's to set).
    fn validate(
        &self,
        me: &'static str,
        body: &RequestBody,
    ) -> Result<MemberRequestRec, MoveError> {
        // The note is the request's draft title: trimmed, at most 200 characters, no control characters.
        if body
            .note
            .as_ref()
            .is_some_and(|n| n.trim().chars().count() > 200 || n.chars().any(char::is_control))
        {
            return Err(invalid_body());
        }
        let kind: &'static str = match body.kind.as_str() {
            "join" => "join",
            "leave" => "leave",
            "swap" => "swap",
            "new_fixed" => "new_fixed",
            "change_fixed" => "change_fixed",
            _ => return Err(invalid_body()),
        };
        let refuse = |given: bool, field: &str| {
            if given {
                Err(field_not_allowed(field))
            } else {
                Ok(())
            }
        };
        let mut rec = MemberRequestRec {
            id: String::new(),
            member: me,
            fingerprint: String::new(),
            kind,
            state: "waiting",
            run_id: None,
            fixed_id: None,
            with: None,
            proposed: None,
            bosses: Vec::new(),
            channel: None,
            note: body
                .note
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(String::from),
            summary: String::new(),
            sent: 0,
            decided: None,
            decided_by: None,
            reason: None,
            expires: Some((Self::start(false) + 7) * 86_400 - TZ_OFFSET_SECS),
        };
        match kind {
            "join" | "leave" | "swap" => {
                refuse(body.bosses.is_some(), "bosses")?;
                refuse(body.day.is_some() || body.time.is_some(), "a day or time")?;
                refuse(
                    body.channel_id.is_some() || body.party.is_some(),
                    "a channel or party",
                )?;
                refuse(kind != "swap" && body.with.is_some(), "with")?;
                let with = match (kind, body.with.as_deref()) {
                    ("swap", None) => return Err(invalid_body()),
                    ("swap", Some(id)) => Some(roster_member(id).ok_or_else(not_found)?),
                    _ => None,
                };
                let party: Vec<&'static str> =
                    match (body.run_id.as_deref(), body.fixed_id.as_deref()) {
                        (Some(run), None) => {
                            let rec_run = self
                                .runs
                                .iter()
                                .find(|r| r.id == run)
                                .ok_or_else(not_found)?;
                            if matches!(rec_run.status, "done" | "cancelled") {
                                return Err(no_effect());
                            }
                            rec.run_id = Some(run.into());
                            rec.fixed_id = rec_run.fixed_id.clone();
                            rec.bosses = rec_run.bosses.clone();
                            rec.channel = Some(rec_run.channel);
                            rec.expires = Some(
                                (Self::start(rec_run.next_week) + 7) * 86_400 - TZ_OFFSET_SECS,
                            );
                            let minute = Self::start_minute(rec_run);
                            rec.summary =
                                format!(
                                    "member request: {kind} {} {}",
                                    tokens(&rec_run.bosses),
                                    Self::when(minute).trim_end_matches(
                                        if rec_run.time.is_none() { " 00:00" } else { "" }
                                    )
                                );
                            rec_run.participants.iter().map(|p| p.id).collect()
                        }
                        (None, Some(fixed)) => {
                            let timing = self
                                .fixed
                                .iter()
                                .find(|f| f.id == fixed && !f.retired)
                                .ok_or_else(not_found)?;
                            rec.fixed_id = Some(fixed.into());
                            rec.bosses = timing.bosses.clone();
                            rec.channel = Some(timing.channel);
                            rec.summary = format!(
                                "member request: {kind} {} {} {}",
                                tokens(&timing.bosses),
                                WEEKDAYS[usize::from(timing.weekday) % 7],
                                timing.time
                            );
                            timing.participants.clone()
                        }
                        _ => return Err(invalid_body()),
                    };
                let mine = party.contains(&me);
                match kind {
                    "join" if mine => return Err(already_in_party()),
                    "leave" | "swap" if !mine => return Err(no_effect()),
                    _ => {}
                }
                if let Some(with) = with {
                    if party.contains(&with) {
                        return Err(already_in_party());
                    }
                    rec.with = Some(with);
                }
            }
            "change_fixed" => {
                refuse(body.bosses.is_some(), "bosses")?;
                refuse(
                    body.run_id.is_some() || body.with.is_some(),
                    "a run or member",
                )?;
                let fixed = body.fixed_id.as_deref().ok_or_else(invalid_body)?;
                let timing = self
                    .fixed
                    .iter()
                    .find(|f| f.id == fixed && !f.retired && f.participants.contains(&me))
                    .ok_or_else(not_found)?;
                let proposed = self.proposal(body)?;
                if proposed.day.is_none()
                    && proposed.time.is_none()
                    && proposed.channel.is_none()
                    && proposed.party.is_none()
                {
                    return Err(invalid_body());
                }
                let same = proposed.day.is_none_or(|d| d == timing.weekday)
                    && proposed.time.as_deref().is_none_or(|t| t == timing.time)
                    && proposed.channel.is_none_or(|c| c == timing.channel)
                    && proposed
                        .party
                        .as_ref()
                        .is_none_or(|p| *p == timing.participants);
                if same {
                    return Err(no_effect());
                }
                rec.fixed_id = Some(fixed.into());
                rec.bosses = timing.bosses.clone();
                rec.channel = Some(timing.channel);
                rec.summary = format!(
                    "member request: change_fixed {} {} {}",
                    tokens(&timing.bosses),
                    WEEKDAYS[usize::from(proposed.day.unwrap_or(timing.weekday)) % 7],
                    proposed.time.as_deref().unwrap_or(&timing.time)
                );
                rec.proposed = Some(proposed);
            }
            _ => {
                refuse(
                    body.run_id.is_some() || body.fixed_id.is_some() || body.with.is_some(),
                    "a run, timing or member",
                )?;
                let (Some(_), Some(_), Some(_), Some(list)) =
                    (body.day, &body.time, &body.channel_id, &body.bosses)
                else {
                    return Err(invalid_body());
                };
                let bosses = parse_bosses(&list.join(" ")).map_err(|_| invalid_body())?;
                let mut proposed = self.proposal(body)?;
                let mut party = proposed.party.take().unwrap_or_default();
                if !party.contains(&me) {
                    party.insert(0, me);
                }
                proposed.party = Some(party);
                rec.summary = format!(
                    "member request: new_fixed {} {} {}",
                    tokens(&bosses),
                    WEEKDAYS[usize::from(proposed.day.unwrap_or(0)) % 7],
                    proposed.time.as_deref().unwrap_or_default()
                );
                rec.bosses = bosses;
                rec.channel = proposed.channel;
                rec.proposed = Some(proposed);
            }
        }
        Ok(rec)
    }

    /// The weekly fields of a body, checked: a weekday, a clock, a known
    /// channel and roster members.
    fn proposal(&self, body: &RequestBody) -> Result<Proposed, MoveError> {
        if body.day.is_some_and(|d| d > 6) || body.time.as_deref().is_some_and(|t| !valid_time(t)) {
            return Err(invalid_body());
        }
        let channel = match body.channel_id.as_deref() {
            Some(id) => Some(seed::channel(id).ok_or_else(invalid_body)?.0),
            None => None,
        };
        let party = match &body.party {
            Some(ids) => Some(
                ids.iter()
                    .map(|id| roster_member(id).ok_or_else(invalid_body))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            None => None,
        };
        Ok(Proposed {
            day: body.day,
            time: body.time.clone(),
            channel,
            party,
        })
    }

    /// `POST /api/public/requests/{id}/withdraw`: the caller's own waiting
    /// request; a retry of the same key answers as the first.
    pub fn member_withdraw_request(
        &mut self,
        me: &'static str,
        key: &str,
        id: &str,
    ) -> Result<serde_json::Value, MoveError> {
        self.keyed(me, key, format!("withdraw {id}"), |s| {
            let now = now_secs();
            let request = s
                .member_requests
                .iter_mut()
                .find(|r| r.id == id && r.member == me)
                .ok_or_else(|| coded(404, "not_found", "No such request."))?;
            if !request.open(now) {
                return Err(coded(
                    409,
                    "request_closed",
                    "That request is no longer waiting.",
                ));
            }
            request.state = "withdrawn";
            request.decided = Some(now);
            request.decided_by = seed::member_name(me).map(|m| m.1.to_owned());
            let request = request.clone();
            Ok(serde_json::to_value(s.request_view(&request, now)).unwrap_or_default())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::catalog::Catalog;

    fn store() -> Store {
        Store::new(Catalog::new(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../web/e2e/fixtures/boss"),
        ))
    }

    fn refused<T>(result: Result<T, Refused>) -> (u16, &'static str) {
        match result {
            Err(Refused::Coded(MoveError::Coded(status, code, _))) => (status, code),
            Err(Refused::Limit(limit)) => (429, limit),
            Err(Refused::Coded(other)) => panic!("not a coded refusal: {other}"),
            Ok(_) => panic!("not refused"),
        }
    }

    fn body(kind: &str) -> RequestBody {
        RequestBody {
            kind: kind.into(),
            ..RequestBody::default()
        }
    }

    fn leave(run: &str) -> RequestBody {
        RequestBody {
            run_id: Some(run.into()),
            ..body("leave")
        }
    }

    #[test]
    fn the_list_is_the_callers_newest_first_with_the_limits() {
        let s = store();
        let list = s.member_requests("1001");
        let states: Vec<&str> = list.requests.iter().map(|r| r.state).collect();
        assert_eq!(
            states,
            ["waiting", "approved", "rejected", "withdrawn", "expired"]
        );
        assert_eq!(
            (list.open, list.today, list.max_open, list.max_today),
            (1, 1, 3, 6)
        );
        let waiting = &list.requests[0];
        assert_eq!(
            waiting.proposed.as_ref().and_then(|p| p.time.as_deref()),
            Some("21:00")
        );
        assert_eq!(waiting.fixed_id.as_deref(), Some("f-kalos"));
        // The expired swap's run is gone; its timing is still there.
        let expired = list.requests.iter().find(|r| r.kind == "swap").unwrap();
        assert!(expired.run.is_none() && expired.fixed_id.is_some());
        assert_eq!(
            expired.with.as_ref().map(|w| w.name.as_str()),
            Some("Kaito")
        );
        assert!(
            list.options
                .members
                .iter()
                .all(|m| m.id != snowflake("1001"))
        );
        assert!(
            list.options
                .members
                .iter()
                .all(|m| m.id != snowflake("1014")),
            "not on the roster"
        );
        assert!(s.member_requests("1002").requests.is_empty());
    }

    #[test]
    fn a_submit_replays_by_key_and_counts_toward_the_limits() {
        let mut s = store();
        let (created, first) = s
            .member_request("1001", "r-1", leave("r-carling"))
            .ok()
            .unwrap();
        assert!(created && first.state == "waiting");
        assert!(
            first
                .summary
                .starts_with("member request: leave HCarling + HStar Tue 29 Sep 22:00")
        );
        let (created, again) = s
            .member_request("1001", "r-1", leave("r-carling"))
            .ok()
            .unwrap();
        assert!(!created && again.id == first.id);
        assert_eq!(
            refused(s.member_request("1001", "r-1", leave("n-carling"))),
            (422, "idempotency_mismatch")
        );
        assert!(s.member_request("1001", "r-2", leave("n-carling")).is_ok());
        // Three waiting now: the fourth is the open limit, the retry still answers.
        assert_eq!(
            refused(s.member_request("1001", "r-3", leave("n-kalos"))),
            (429, "open")
        );
        assert!(s.member_request("1001", "r-1", leave("r-carling")).is_ok());
        // Withdrawn ones free an open place but still count today.
        s.member_withdraw_request("1001", "w-1", &first.id)
            .ok()
            .unwrap();
        s.member_withdraw_request("1001", "w-2", &again.id)
            .err()
            .unwrap();
        let list = s.member_requests("1001");
        assert_eq!((list.open, list.today), (2, 3));
        // Each sent-then-withdrawn request keeps the open count down but counts today.
        for key in ["r-4", "r-5", "r-6"] {
            let (_, sent) = s
                .member_request("1001", key, leave("n-kalos"))
                .ok()
                .unwrap();
            s.member_withdraw_request("1001", &format!("w-{key}"), &sent.id)
                .ok()
                .unwrap();
        }
        let list = s.member_requests("1001");
        assert_eq!((list.open, list.today), (2, 6));
        assert_eq!(
            refused(s.member_request("1001", "r-7", leave("r-kalos"))),
            (429, "today")
        );
    }

    #[test]
    fn subjects_and_fields_are_checked_as_the_server_does() {
        let mut s = store();
        let join = |run: &str| RequestBody {
            run_id: Some(run.into()),
            ..body("join")
        };
        assert_eq!(
            refused(s.member_request("1001", "a", join("r-carling"))),
            (409, "already_in_party")
        );
        assert_eq!(
            refused(s.member_request("1001", "b", leave("r-limbo"))),
            (409, "no_effect")
        );
        assert_eq!(
            refused(s.member_request("1001", "c", join("nope"))),
            (404, "not_found")
        );
        let both = RequestBody {
            fixed_id: Some("f-limbo".into()),
            ..join("r-limbo")
        };
        assert_eq!(
            refused(s.member_request("1001", "d", both)),
            (422, "invalid_body")
        );
        let swap = |with: &str| RequestBody {
            run_id: Some("r-carling".into()),
            with: Some(snowflake(with)),
            ..body("swap")
        };
        assert_eq!(
            refused(s.member_request("1001", "e", swap("1002"))),
            (409, "already_in_party")
        );
        assert_eq!(
            refused(s.member_request("1001", "f", swap("1014"))),
            (404, "not_found")
        );
        let (_, swapped) = s.member_request("1001", "g", swap("1009")).ok().unwrap();
        assert_eq!(swapped.with.map(|w| w.name), Some("Kaito".into()));
        let change = |fields: RequestBody| RequestBody {
            fixed_id: Some("f-carling".into()),
            ..fields
        };
        let bosses = change(RequestBody {
            time: Some("21:00".into()),
            bosses: Some(vec!["HLimbo".into()]),
            ..body("change_fixed")
        });
        assert_eq!(
            refused(s.member_request("1001", "h", bosses)),
            (422, "field_not_allowed")
        );
        let same = change(RequestBody {
            time: Some("22:00".into()),
            ..body("change_fixed")
        });
        assert_eq!(
            refused(s.member_request("1001", "i", same)),
            (409, "no_effect")
        );
        let not_mine = RequestBody {
            fixed_id: Some("f-limbo".into()),
            time: Some("22:00".into()),
            ..body("change_fixed")
        };
        assert_eq!(
            refused(s.member_request("1001", "j", not_mine)),
            (404, "not_found")
        );
        let new = RequestBody {
            day: Some(6),
            time: Some("21:00".into()),
            channel_id: Some("limbo-trio".into()),
            bosses: Some(vec!["NLimbo".into()]),
            party: Some(vec![snowflake("1003")]),
            ..body("new_fixed")
        };
        let (created, made) = s.member_request("1001", "k", new).ok().unwrap();
        assert!(created);
        let party = made.proposed.and_then(|p| p.party).unwrap();
        assert_eq!(
            party.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["Asahi", "Mika"],
            "the requester is always in"
        );
        assert_eq!(
            refused(s.member_request("1001", "l", body("new_fixed"))),
            (422, "invalid_body")
        );
        let long = RequestBody {
            note: Some("x".repeat(201)),
            ..leave("r-carling")
        };
        assert_eq!(
            refused(s.member_request("1001", "m", long)),
            (422, "invalid_body")
        );
        let broken = RequestBody {
            note: Some("two\nlines".into()),
            ..leave("r-carling")
        };
        assert_eq!(
            refused(s.member_request("1001", "n", broken)),
            (422, "invalid_body")
        );
        let padded = RequestBody {
            note: Some(format!("  {}  ", "x".repeat(200))),
            ..leave("r-carling")
        };
        // Trimmed before counting: 200 characters with padding still fit (a store with room under the limits).
        let (_, sent) = store().member_request("1001", "o", padded).ok().unwrap();
        assert_eq!(sent.note.as_deref(), Some("x".repeat(200).as_str()));
    }

    #[test]
    fn withdraw_is_the_requesters_and_only_while_it_waits() {
        let mut s = store();
        let code = |r: Result<serde_json::Value, MoveError>| match r {
            Err(MoveError::Coded(status, code, _)) => (status, code),
            _ => panic!("not refused"),
        };
        assert_eq!(
            code(s.member_withdraw_request("1002", "w", "req-kalos-weekly")),
            (404, "not_found")
        );
        assert_eq!(
            code(s.member_withdraw_request("1001", "w-0", "req-carling-swap")),
            (409, "request_closed"),
            "expired"
        );
        let done = s
            .member_withdraw_request("1001", "w-1", "req-kalos-weekly")
            .ok()
            .unwrap();
        assert_eq!(
            (done["state"].as_str(), done["decided_by"].as_str()),
            (Some("withdrawn"), Some("Asahi"))
        );
        assert_eq!(
            s.member_withdraw_request("1001", "w-1", "req-kalos-weekly")
                .ok(),
            Some(done),
            "a retry answers the same"
        );
        assert_eq!(
            code(s.member_withdraw_request("1001", "w-2", "req-kalos-weekly")),
            (409, "request_closed")
        );
    }
}
