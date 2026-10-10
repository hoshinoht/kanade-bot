//! Weekly-timing ownership requests, one store for both origins, as the
//! server keeps them: the admin Inbox's Ownership tab (any admin session
//! accepts, pinning the requester as owner, or declines within 24 h) and the
//! member portal's timings (`src/api/public/ownership.rs`: hand off, ask,
//! accept, decline, withdraw), with the server's status and error codes.

use serde::Serialize;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::clock::{iso_secs, now_secs};
use super::dto::{Boss, Named};
use super::history::Actor;
use super::portal::snowflake;
use super::seed::{self, Fixed};
use super::{MoveError, Store};

/// How long a request stays open.
const TTL_SECS: i64 = 24 * 3600;

#[derive(Clone)]
pub struct OwnerRequest {
    id: String,
    short_id: String,
    fixed_id: String,
    requester: &'static str,
    /// Unix seconds it was asked.
    created: i64,
    /// `open`, `accepted`, `declined`, `withdrawn` or `superseded`.
    status: &'static str,
    /// `<kind>:<id>` of who closed it.
    decided_by: Option<String>,
}

/// A member hand-off already applied under an `Idempotency-Key`, so a retry
/// answers again instead of refusing (the new owner is no longer the giver).
pub struct HandOffKey {
    member: &'static str,
    key: String,
    fixed_id: String,
    to: &'static str,
}

#[derive(Serialize)]
pub struct OwnerRequestDto {
    pub id: String,
    pub short_id: String,
    pub fixed_id: String,
    pub fixed_short_id: String,
    pub bosses: Vec<Boss>,
    pub weekday: u8,
    pub weekday_name: &'static str,
    pub time: String,
    pub requester: Named,
    pub owner: Named,
    pub channel: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

/// `public.json` `MemberOwnerRequest`.
#[derive(Serialize)]
pub struct MemberOwnerRequest {
    pub id: String,
    pub requester: Named,
    pub created_at: String,
    pub expires_at: String,
    pub status: &'static str,
    pub mine: bool,
}

/// `public.json` `MemberTiming`: never the channel, note or short id.
#[derive(Serialize)]
pub struct MemberTiming {
    pub id: String,
    pub bosses: Vec<Boss>,
    pub weekday: u8,
    pub time: String,
    pub party: Vec<Named>,
    pub owner: Named,
    pub owner_pinned: bool,
    pub you_own: bool,
    pub requests: Vec<MemberOwnerRequest>,
}

/// `public.json` `MemberTimings`.
#[derive(Serialize)]
pub struct MemberTimings {
    pub timings: Vec<MemberTiming>,
    pub generated_at: String,
}

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Two open requests on seeded timings, asked `hours` before the mock's now:
/// Tsubame wants Kalos (pinned to Ren), Mika wants Carling (nearly expired).
pub fn seed() -> Vec<OwnerRequest> {
    let ask = |id: &str, short_id: &str, fixed_id: &str, requester, hours: i64| OwnerRequest {
        id: id.into(),
        short_id: short_id.into(),
        fixed_id: fixed_id.into(),
        requester,
        created: now_secs() - hours * 3600,
        status: "open",
        decided_by: None,
    };
    vec![
        ask("own-carling", "0a1b2c3d", "f-carling", "1003", 20),
        ask("own-kalos", "4e5f6a7b", "f-kalos", "1005", 5),
    ]
}

fn name(id: &str) -> String {
    seed::member_name(id).map_or(id, |m| m.1).into()
}

fn named(id: &str) -> Named {
    Named {
        id: id.into(),
        name: name(id),
    }
}

/// A member as the public origin names them: by their snowflake-shaped id.
fn member_named(id: &str) -> Named {
    Named {
        id: snowflake(id),
        name: name(id),
    }
}

fn coded(status: u16, code: &'static str, message: impl Into<String>) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

fn refused(message: &str) -> MoveError {
    coded(409, "conflicts", message)
}

// The member-facing refusals, as the server words them (`domain::ownership`).
fn not_owner() -> MoveError {
    coded(
        403,
        "not_owner",
        "Only the timing's owner or an admin can do that.",
    )
}

fn not_requester() -> MoveError {
    coded(
        403,
        "not_requester",
        "Only the member who asked can withdraw it.",
    )
}

fn not_on_party() -> MoveError {
    coded(
        409,
        "not_on_party",
        "Ownership only moves between members of the party.",
    )
}

fn already_owner() -> MoveError {
    coded(409, "already_owner", "They already own this timing.")
}

fn request_closed() -> MoveError {
    coded(
        409,
        "request_closed",
        "That request has already been decided.",
    )
}

fn request_expired() -> MoveError {
    coded(409, "request_expired", "That request has expired.")
}

fn unknown_timing() -> MoveError {
    coded(404, "not_found", "No such weekly timing.")
}

fn unknown_request() -> MoveError {
    coded(404, "not_found", "No such ownership request.")
}

fn idempotency_mismatch() -> MoveError {
    coded(
        422,
        "idempotency_mismatch",
        "That Idempotency-Key was already used for a different request.",
    )
}

fn digest(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The request id `member` asks with `key`, so a retried ask finds it (the
/// server's is a SHA-256 prefix).
fn ask_id(member: &str, key: &str) -> String {
    format!("public-{:016x}", digest(&format!("{member}:{key}")))
}

impl OwnerRequest {
    fn live(&self, now: i64) -> bool {
        self.status == "open" && now < self.created + TTL_SECS
    }

    /// An undecided request past its 24 h reads as the server's sweep leaves it.
    fn shown_status(&self, now: i64) -> &'static str {
        if self.status == "open" && !self.live(now) {
            "expired"
        } else {
            self.status
        }
    }

    fn member_view(&self, me: &str, now: i64) -> MemberOwnerRequest {
        MemberOwnerRequest {
            id: self.id.clone(),
            requester: member_named(self.requester),
            created_at: iso_secs(self.created),
            expires_at: iso_secs(self.created + TTL_SECS),
            status: self.shown_status(now),
            mine: self.requester == me,
        }
    }
}

fn may_request(fixed: &Fixed, requester: &str) -> Result<(), MoveError> {
    if !fixed.participants.contains(&requester) {
        return Err(not_on_party());
    }
    if fixed.owner() == requester {
        return Err(already_owner());
    }
    Ok(())
}

impl Store {
    fn owner_dto(&self, r: &OwnerRequest, f: &Fixed) -> OwnerRequestDto {
        OwnerRequestDto {
            id: r.id.clone(),
            short_id: r.short_id.clone(),
            fixed_id: f.id.clone(),
            fixed_short_id: f.short_id.clone(),
            bosses: self.bosses(&f.bosses),
            weekday: f.weekday,
            weekday_name: WEEKDAYS[usize::from(f.weekday) % 7],
            time: f.time.clone(),
            requester: named(r.requester),
            owner: named(f.owner()),
            channel: seed::channel(f.channel).map(|c| c.1.to_owned()),
            created_at: iso_secs(r.created),
            expires_at: iso_secs(r.created + TTL_SECS),
        }
    }

    /// `GET /api/admin/inbox/ownership`: open, unexpired, oldest first.
    pub fn owner_requests(&self) -> Vec<OwnerRequestDto> {
        let now = now_secs();
        let mut open: Vec<&OwnerRequest> =
            self.owner_requests.iter().filter(|r| r.live(now)).collect();
        open.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        open.into_iter()
            .filter_map(|r| {
                let f = self
                    .fixed
                    .iter()
                    .find(|f| f.id == r.fixed_id && !f.retired)?;
                Some(self.owner_dto(r, f))
            })
            .collect()
    }

    /// The summary's share of the inbox count.
    pub fn open_owner_requests(&self) -> usize {
        self.owner_requests().len()
    }

    /// Pin `owner` on timing `fi` as one recorded edit, then close the
    /// timing's other open requests (`keep` aside): its owner just changed.
    fn pin_owner(
        &mut self,
        fi: usize,
        owner: &'static str,
        actor: Actor,
        surface: &'static str,
        keep: Option<&str>,
    ) -> Result<(), MoveError> {
        let who = format!("{}:{}", actor.kind, actor.id);
        self.tracked(actor, surface, |s| {
            s.fixed[fi].owner_id = owner;
            s.fixed[fi].owner_pinned = true;
            s.version += 1;
            Ok(())
        })?;
        let fixed_id = self.fixed[fi].id.clone();
        for other in self
            .owner_requests
            .iter_mut()
            .filter(|r| r.fixed_id == fixed_id && Some(r.id.as_str()) != keep && r.status == "open")
        {
            other.status = "superseded";
            other.decided_by = Some(who.clone());
        }
        Ok(())
    }

    /// Accept (pin the requester as owner, closing the timing's other open
    /// requests) or decline. A retry of this admin's own decision answers as
    /// the first did; any other decision on a closed request is `409`.
    pub fn decide_owner_request(&mut self, id: &str, accept: bool) -> Result<String, MoveError> {
        let index = self
            .owner_requests
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| coded(404, "not_found", "Nothing in the inbox has that id."))?;
        let request = self.owner_requests[index].clone();
        let actor = self.session_actor();
        let who = format!("{}:{}", actor.kind, actor.id);
        let wanted = if accept { "accepted" } else { "declined" };
        let name = name(request.requester);
        let timing = self
            .fixed
            .iter()
            .find(|f| f.id == request.fixed_id)
            .map_or_else(String::new, |f| f.short_id.clone());
        let message = if accept {
            format!("{name} now owns weekly timing #{timing}.")
        } else {
            format!("Declined {name}'s request to own weekly timing #{timing}.")
        };
        if request.status == wanted && request.decided_by.as_deref() == Some(who.as_str()) {
            return Ok(message);
        }
        if request.status != "open" {
            return Err(refused("That request has already been decided."));
        }
        if !request.live(now_secs()) {
            return Err(refused("That request has expired."));
        }
        let fi = self
            .fixed
            .iter()
            .position(|f| f.id == request.fixed_id && !f.retired)
            .ok_or_else(|| refused("That weekly timing no longer exists."))?;
        if accept {
            let fixed = &self.fixed[fi];
            if !fixed.participants.contains(&request.requester) {
                return Err(refused(
                    "Ownership only moves between members of the party.",
                ));
            }
            if fixed.owner() == request.requester {
                return Err(refused("They already own this timing."));
            }
            self.pin_owner(
                fi,
                request.requester,
                actor,
                "admin_portal",
                Some(&request.id),
            )?;
        }
        let closed = &mut self.owner_requests[index];
        closed.status = wanted;
        closed.decided_by = Some(who);
        Ok(message)
    }

    fn live_timing(&self, fixed_id: &str) -> Result<usize, MoveError> {
        self.fixed
            .iter()
            .position(|f| f.id == fixed_id && !f.retired)
            .ok_or_else(unknown_timing)
    }

    fn request_index(&self, id: &str) -> Result<usize, MoveError> {
        self.owner_requests
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(unknown_request)
    }

    /// Timing `fi` as member `me` sees it: this timing's live requests, all
    /// of them for its owner and otherwise only the member's own.
    fn member_timing(&self, fi: usize, me: &str, now: i64) -> MemberTiming {
        let f = &self.fixed[fi];
        let you_own = f.owner() == me;
        MemberTiming {
            id: f.id.clone(),
            bosses: self.bosses(&f.bosses),
            weekday: f.weekday,
            time: f.time.clone(),
            party: f.participants.iter().map(|id| member_named(id)).collect(),
            owner: member_named(f.owner()),
            owner_pinned: f.owner_pinned,
            you_own,
            requests: self
                .owner_requests
                .iter()
                .filter(|r| r.fixed_id == f.id && r.live(now) && (you_own || r.requester == me))
                .map(|r| r.member_view(me, now))
                .collect(),
        }
    }

    /// `GET /api/public/timings`: the live weekly timings `me` (a seed id) is on.
    pub fn member_timings(&self, me: &str) -> MemberTimings {
        let now = now_secs();
        MemberTimings {
            timings: self
                .fixed
                .iter()
                .enumerate()
                .filter(|(_, f)| !f.retired && f.participants.contains(&me))
                .map(|(fi, _)| self.member_timing(fi, me, now))
                .collect(),
            generated_at: iso_secs(now),
        }
    }

    /// `POST /api/public/timings/{id}/owner {to}`: the owner hands the timing
    /// to another party member (`to` a snowflake) at once, closing its open
    /// requests. A retry with the same key answers the timing as it now is.
    pub fn member_hand_off(
        &mut self,
        me: &'static str,
        key: &str,
        fixed_id: &str,
        to: &str,
    ) -> Result<MemberTiming, MoveError> {
        let fi = self.live_timing(fixed_id)?;
        if let Some(seen) = self
            .owner_hand_offs
            .iter()
            .find(|h| h.member == me && h.key == key)
        {
            if seen.fixed_id != fixed_id || snowflake(seen.to) != to {
                return Err(idempotency_mismatch());
            }
            return Ok(self.member_timing(fi, me, now_secs()));
        }
        let fixed = &self.fixed[fi];
        if fixed.owner() != me {
            return Err(not_owner());
        }
        let receiver = fixed
            .participants
            .iter()
            .copied()
            .find(|p| snowflake(p) == to)
            .ok_or_else(not_on_party)?;
        if fixed.owner() == receiver {
            return Err(already_owner());
        }
        self.pin_owner(
            fi,
            receiver,
            Actor::new("member", me),
            "public_portal",
            None,
        )?;
        self.owner_hand_offs.push(HandOffKey {
            member: me,
            key: key.to_owned(),
            fixed_id: fixed_id.to_owned(),
            to: receiver,
        });
        Ok(self.member_timing(fi, me, now_secs()))
    }

    /// `POST /api/public/timings/{id}/owner-requests`: a party member asks the
    /// owner for the timing. `true` with a new request; a retry with the same
    /// key finds that request as it now is (`false`).
    pub fn member_ask(
        &mut self,
        me: &'static str,
        key: &str,
        fixed_id: &str,
    ) -> Result<(bool, MemberOwnerRequest), MoveError> {
        let now = now_secs();
        let id = ask_id(me, key);
        if let Some(found) = self.owner_requests.iter().find(|r| r.id == id) {
            if found.requester != me || found.fixed_id != fixed_id {
                return Err(idempotency_mismatch());
            }
            return Ok((false, found.member_view(me, now)));
        }
        let fi = self.live_timing(fixed_id)?;
        may_request(&self.fixed[fi], me)?;
        if self
            .owner_requests
            .iter()
            .any(|r| r.fixed_id == fixed_id && r.requester == me && r.live(now))
        {
            return Err(coded(
                409,
                "already_asked",
                "You already asked to own this timing.",
            ));
        }
        let request = OwnerRequest {
            short_id: format!("{:08x}", digest(&id) >> 32),
            id,
            fixed_id: fixed_id.to_owned(),
            requester: me,
            created: now,
            status: "open",
            decided_by: None,
        };
        let view = request.member_view(me, now);
        self.owner_requests.push(request);
        Ok((true, view))
    }

    /// `POST /api/public/owner-requests/{id}/accept|decline`: the timing's
    /// owner only. A request the caller has no part in (neither the owner nor
    /// the requester) is the same `404` as an unknown one; the requester gets
    /// `not_owner`. A closed or expired request answers its state to the
    /// owner alone. Unlike the admin Inbox, a repeat is refused as the server
    /// does (`request_closed`, or `404` once an accept moved the timing on).
    pub fn member_decide(
        &mut self,
        me: &'static str,
        id: &str,
        accept: bool,
    ) -> Result<MemberOwnerRequest, MoveError> {
        let now = now_secs();
        let index = self.request_index(id)?;
        let request = self.owner_requests[index].clone();
        let fi = self.live_timing(&request.fixed_id)?;
        let fixed = &self.fixed[fi];
        if fixed.owner() != me {
            return Err(if request.requester == me {
                not_owner()
            } else {
                unknown_request()
            });
        }
        if !request.live(now) {
            return Err(if request.status == "open" {
                request_expired()
            } else {
                request_closed()
            });
        }
        let who = format!("member:{me}");
        let status = if accept {
            may_request(fixed, request.requester)?;
            self.pin_owner(
                fi,
                request.requester,
                Actor::new("member", me),
                "public_portal",
                Some(&request.id),
            )?;
            "accepted"
        } else {
            "declined"
        };
        let closed = &mut self.owner_requests[index];
        closed.status = status;
        closed.decided_by = Some(who);
        Ok(closed.member_view(me, now))
    }

    /// `POST /api/public/owner-requests/{id}/withdraw`: the requester only.
    pub fn member_withdraw(
        &mut self,
        me: &'static str,
        id: &str,
    ) -> Result<MemberOwnerRequest, MoveError> {
        let now = now_secs();
        let index = self.request_index(id)?;
        let request = &self.owner_requests[index];
        if request.requester != me {
            let owns = self
                .fixed
                .iter()
                .any(|f| f.id == request.fixed_id && f.owner() == me);
            return Err(if owns {
                not_requester()
            } else {
                unknown_request()
            });
        }
        let request = &mut self.owner_requests[index];
        if !request.live(now) {
            return Err(request_closed());
        }
        request.status = "withdrawn";
        request.decided_by = Some(format!("member:{me}"));
        Ok(request.member_view(me, now))
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

    fn code<T>(result: Result<T, MoveError>) -> (u16, &'static str) {
        match result {
            Err(MoveError::Coded(status, code, _)) => (status, code),
            Err(other) => panic!("not a coded refusal: {other}"),
            Ok(_) => panic!("not refused"),
        }
    }

    fn timing<'a>(timings: &'a MemberTimings, id: &str) -> &'a MemberTiming {
        timings.timings.iter().find(|t| t.id == id).unwrap()
    }

    #[test]
    fn accepting_pins_the_requester_and_a_retry_answers_the_same() {
        let mut s = store();
        assert_eq!(s.open_owner_requests(), 2);
        let first = s.decide_owner_request("own-kalos", true).ok().unwrap();
        let kalos = s.fixed.iter().find(|f| f.id == "f-kalos").unwrap();
        assert_eq!((kalos.owner(), kalos.owner_pinned), ("1005", true));
        assert_eq!(s.decide_owner_request("own-kalos", true).ok(), Some(first));
        assert!(matches!(
            s.decide_owner_request("own-kalos", false),
            Err(MoveError::Coded(409, "conflicts", _))
        ));
        assert_eq!(s.open_owner_requests(), 1);
    }

    #[test]
    fn declining_leaves_the_owner() {
        let mut s = store();
        assert!(s.decide_owner_request("own-carling", false).is_ok());
        let carling = s.fixed.iter().find(|f| f.id == "f-carling").unwrap();
        assert_eq!((carling.owner(), carling.owner_pinned), ("1001", false));
        assert!(matches!(
            s.decide_owner_request("nope", false),
            Err(MoveError::Coded(404, "not_found", _))
        ));
    }

    #[test]
    fn members_see_their_timings_and_only_the_requests_theirs_to_see() {
        let s = store();
        let asahi = s.member_timings("1001");
        let ids: Vec<&str> = asahi.timings.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["f-baldrix", "f-kalos", "f-jupiter", "f-carling"]);
        let carling = timing(&asahi, "f-carling");
        assert!(carling.you_own && !carling.owner_pinned);
        assert_eq!(carling.owner.id, "100000000000001001");
        let mika = &carling.requests[0];
        assert_eq!(
            (mika.requester.id.as_str(), mika.mine),
            ("100000000000001003", false)
        );
        // Kalos belongs to Ren: Tsubame's request is not Asahi's to see.
        let kalos = timing(&asahi, "f-kalos");
        assert!(!kalos.you_own && kalos.requests.is_empty());
        let tsubame = s.member_timings("1005");
        assert!(timing(&tsubame, "f-kalos").requests[0].mine);
        assert!(s.member_timings("1014").timings.is_empty(), "on no party");
    }

    #[test]
    fn an_ask_replays_by_key_and_the_owner_accepts_it() {
        let mut s = store();
        let (created, asked) = s.member_ask("1001", "k-1", "f-jupiter").ok().unwrap();
        assert!(created && asked.mine && asked.status == "open");
        let (created, again) = s.member_ask("1001", "k-1", "f-jupiter").ok().unwrap();
        assert!(!created);
        assert_eq!(again.id, asked.id);
        assert_eq!(
            code(s.member_ask("1001", "k-2", "f-jupiter")),
            (409, "already_asked")
        );
        assert_eq!(
            code(s.member_ask("1001", "k-1", "f-kalos")),
            (422, "idempotency_mismatch")
        );
        assert_eq!(
            code(s.member_ask("1001", "k-3", "f-limbo")),
            (409, "not_on_party")
        );
        assert_eq!(
            code(s.member_ask("1001", "k-4", "f-baldrix")),
            (409, "already_owner")
        );
        assert_eq!(
            code(s.member_ask("1001", "k-5", "f-none")),
            (404, "not_found")
        );
        // Only Minato decides; Asahi's own request is not hers to accept.
        assert_eq!(
            code(s.member_decide("1001", &asked.id, true)),
            (403, "not_owner")
        );
        let accepted = s.member_decide("1012", &asked.id, true).ok().unwrap();
        assert_eq!((accepted.status, accepted.mine), ("accepted", false));
        let jupiter = s.member_timings("1001");
        let jupiter = timing(&jupiter, "f-jupiter");
        assert!(jupiter.you_own && jupiter.owner_pinned);
        // Minato no longer owns it: the request is no longer his to see.
        assert_eq!(
            code(s.member_decide("1012", &asked.id, true)),
            (404, "not_found")
        );
        assert_eq!(
            code(s.member_decide("1001", &asked.id, false)),
            (409, "request_closed")
        );
    }

    #[test]
    fn hand_off_moves_ownership_within_the_party_and_supersedes_requests() {
        let mut s = store();
        let ren = snowflake("1002");
        assert_eq!(
            code(s.member_hand_off("1002", "h-1", "f-carling", &ren)),
            (403, "not_owner")
        );
        assert_eq!(
            code(s.member_hand_off("1001", "h-2", "f-carling", &snowflake("1009"))),
            (409, "not_on_party")
        );
        assert_eq!(
            code(s.member_hand_off("1001", "h-3", "f-none", &ren)),
            (404, "not_found")
        );
        let version = s.version;
        let handed = s
            .member_hand_off("1001", "h-4", "f-carling", &ren)
            .ok()
            .unwrap();
        assert_eq!(handed.owner.id, ren);
        assert!(handed.owner_pinned && !handed.you_own && handed.requests.is_empty());
        assert_eq!(s.version, version + 1);
        assert_eq!(
            code(s.member_withdraw("1003", "own-carling")),
            (409, "request_closed")
        );
        assert!(s.member_hand_off("1001", "h-4", "f-carling", &ren).is_ok());
        assert_eq!(s.version, version + 1, "the retry changed nothing");
        assert_eq!(
            code(s.member_hand_off("1001", "h-4", "f-carling", &snowflake("1003"))),
            (422, "idempotency_mismatch")
        );
        assert_eq!(
            code(s.member_hand_off("1001", "h-5", "f-carling", &snowflake("1003"))),
            (403, "not_owner")
        );
        assert_eq!(
            s.open_owner_requests(),
            1,
            "only Kalos is left in the Inbox"
        );
    }

    #[test]
    fn decline_and_withdraw_belong_to_the_owner_and_the_requester() {
        let mut s = store();
        assert_eq!(
            code(s.member_decide("1002", "own-carling", false)),
            (404, "not_found")
        );
        assert_eq!(
            code(s.member_withdraw("1001", "own-carling")),
            (403, "not_requester")
        );
        let withdrawn = s.member_withdraw("1003", "own-carling").ok().unwrap();
        assert_eq!(withdrawn.status, "withdrawn");
        assert_eq!(
            code(s.member_withdraw("1003", "own-carling")),
            (409, "request_closed")
        );
        let version = s.version;
        let declined = s.member_decide("1002", "own-kalos", false).ok().unwrap();
        assert_eq!(declined.status, "declined");
        assert_eq!(s.version, version, "declining writes no edit");
        assert_eq!(code(s.member_withdraw("1001", "none")), (404, "not_found"));
        assert_eq!(s.open_owner_requests(), 0);
    }
}
