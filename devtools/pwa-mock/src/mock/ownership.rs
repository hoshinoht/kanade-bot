//! Weekly-timing ownership requests (admin-api "Inbox (A6)", Ownership tab):
//! a party member asks to own a timing; any admin session accepts (pinning
//! the requester as owner) or declines it within 24 h, as the server does.

use serde::Serialize;

use super::clock::{iso_secs, now_secs};
use super::dto::{Boss, Named};
use super::seed::{self, Fixed};
use super::{MoveError, Store};

/// How long a request stays open.
const TTL_SECS: i64 = 24 * 3600;

#[derive(Clone)]
pub struct OwnerRequest {
    id: &'static str,
    short_id: &'static str,
    fixed_id: &'static str,
    requester: &'static str,
    /// Unix seconds it was asked.
    created: i64,
    /// `open`, `accepted`, `declined` or `superseded`.
    status: &'static str,
    /// `<kind>:<id>` of the admin who closed it.
    decided_by: Option<String>,
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
    let ask = |id, short_id, fixed_id, requester, hours: i64| OwnerRequest {
        id,
        short_id,
        fixed_id,
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

fn named(id: &str) -> Named {
    Named {
        id: id.into(),
        name: seed::member_name(id).map_or(id, |m| m.1).into(),
    }
}

fn coded(status: u16, code: &'static str, message: impl Into<String>) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

fn refused(message: &str) -> MoveError {
    coded(409, "conflicts", message)
}

impl OwnerRequest {
    fn live(&self, now: i64) -> bool {
        self.status == "open" && now < self.created + TTL_SECS
    }
}

impl Store {
    fn owner_dto(&self, r: &OwnerRequest, f: &Fixed) -> OwnerRequestDto {
        OwnerRequestDto {
            id: r.id.into(),
            short_id: r.short_id.into(),
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
        open.sort_by_key(|r| (r.created, r.id));
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
        let name = named(request.requester).name;
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
            let Some(requester) = fixed
                .participants
                .iter()
                .copied()
                .find(|p| *p == request.requester)
            else {
                return Err(refused(
                    "Ownership only moves between members of the party.",
                ));
            };
            if fixed.owner() == requester {
                return Err(refused("They already own this timing."));
            }
            self.tracked(actor, "admin_portal", |s| {
                s.fixed[fi].owner_id = requester;
                s.fixed[fi].owner_pinned = true;
                s.version += 1;
                Ok(())
            })?;
            for other in self.owner_requests.iter_mut().filter(|r| {
                r.fixed_id == request.fixed_id && r.id != request.id && r.status == "open"
            }) {
                other.status = "superseded";
                other.decided_by = Some(who.clone());
            }
        }
        let closed = &mut self.owner_requests[index];
        closed.status = wanted;
        closed.decided_by = Some(who);
        Ok(message)
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
}
