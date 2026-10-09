//! Who may move a weekly timing's ownership (user decision 2026-10-10). The
//! owner is [`FixedRun::owner`]: the pinned owner, else the first participant.
//! Only party members give or receive ownership; staff may act for anyone.
//! Every change pins the new owner through a recorded schedule edit.

use chrono::{DateTime, Utc};

mod request;

pub use request::{OWNER_REQUEST_TTL, OwnerRequest, OwnerRequestStatus, OwnerRequestStore};

use crate::domain::schedule::FixedRun;

/// Why an ownership action was refused, in the words members see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnershipRefusal {
    /// Only the owner (or staff) may hand off or decide.
    NotOwner,
    /// Ownership moves only between party members.
    NotOnParty,
    /// The receiver or requester already owns it.
    AlreadyOwner,
    /// The request was decided, withdrawn or superseded.
    Closed,
    Expired,
    /// Only the member who asked may withdraw.
    NotRequester,
}

impl std::fmt::Display for OwnershipRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotOwner => "Only the timing's owner or an admin can do that.",
            Self::NotOnParty => "Ownership only moves between members of the party.",
            Self::AlreadyOwner => "They already own this timing.",
            Self::Closed => "That request has already been decided.",
            Self::Expired => "That request has expired.",
            Self::NotRequester => "Only the member who asked can withdraw it.",
        })
    }
}

impl std::error::Error for OwnershipRefusal {}

fn on_party(fixed: &FixedRun, user: &str) -> bool {
    fixed.participants.iter().any(|id| id == user)
}

/// The owner (or staff) hands the timing to `to`, another party member.
///
/// # Errors
/// The [`OwnershipRefusal`] that applies.
pub fn hand_off(
    fixed: &FixedRun,
    giver: &str,
    staff: bool,
    to: &str,
) -> Result<(), OwnershipRefusal> {
    if !staff && fixed.owner() != giver {
        return Err(OwnershipRefusal::NotOwner);
    }
    if !on_party(fixed, to) {
        return Err(OwnershipRefusal::NotOnParty);
    }
    if fixed.owner() == to {
        return Err(OwnershipRefusal::AlreadyOwner);
    }
    Ok(())
}

/// A party member who does not own the timing asks to own it.
///
/// # Errors
/// The [`OwnershipRefusal`] that applies.
pub fn may_request(fixed: &FixedRun, requester: &str) -> Result<(), OwnershipRefusal> {
    if !on_party(fixed, requester) {
        return Err(OwnershipRefusal::NotOnParty);
    }
    if fixed.owner() == requester {
        return Err(OwnershipRefusal::AlreadyOwner);
    }
    Ok(())
}

/// The owner (or staff) accepts or declines `request` at `now`. Accepting
/// also needs the requester still on the party.
///
/// # Errors
/// The [`OwnershipRefusal`] that applies.
pub fn may_decide(
    fixed: &FixedRun,
    request: &OwnerRequest,
    decider: &str,
    staff: bool,
    accept: bool,
    now: DateTime<Utc>,
) -> Result<(), OwnershipRefusal> {
    if !request.live(now) {
        return Err(if request.status.as_str() == "open" {
            OwnershipRefusal::Expired
        } else {
            OwnershipRefusal::Closed
        });
    }
    if !staff && fixed.owner() != decider {
        return Err(OwnershipRefusal::NotOwner);
    }
    if accept {
        may_request(fixed, &request.requester)?;
    }
    Ok(())
}

/// Only the requester withdraws their own open request.
///
/// # Errors
/// The [`OwnershipRefusal`] that applies.
pub fn may_withdraw(
    request: &OwnerRequest,
    member: &str,
    now: DateTime<Utc>,
) -> Result<(), OwnershipRefusal> {
    if request.requester != member {
        return Err(OwnershipRefusal::NotRequester);
    }
    if !request.live(now) {
        return Err(OwnershipRefusal::Closed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, TimeDelta, TimeZone, Weekday};

    use super::*;
    use crate::domain::attendance::AttendanceDefault;

    fn timing(party: &[&str]) -> FixedRun {
        FixedRun {
            id: "f".into(),
            owner_id: "creator".into(),
            channel_id: Some("700".into()),
            bosses: Vec::new(),
            weekday: Weekday::Tue,
            time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
            participants: party.iter().map(|id| (*id).to_owned()).collect(),
            note: None,
            attendance_default: AttendanceDefault::OptIn,
            standing: Vec::new(),
            owner_pinned: false,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap()
    }

    #[test]
    fn only_the_owner_or_staff_hands_off_and_only_to_the_party() {
        let fixed = timing(&["a", "b"]);
        assert_eq!(hand_off(&fixed, "a", false, "b"), Ok(()));
        assert_eq!(
            hand_off(&fixed, "b", false, "b"),
            Err(OwnershipRefusal::NotOwner)
        );
        assert_eq!(
            hand_off(&fixed, "b", true, "b"),
            Ok(()),
            "staff act for the owner"
        );
        assert_eq!(
            hand_off(&fixed, "a", false, "c"),
            Err(OwnershipRefusal::NotOnParty)
        );
        assert_eq!(
            hand_off(&fixed, "a", false, "a"),
            Err(OwnershipRefusal::AlreadyOwner)
        );
    }

    #[test]
    fn requests_are_decided_by_the_owner_while_open_and_on_the_party() {
        let fixed = timing(&["a", "b"]);
        assert_eq!(may_request(&fixed, "b"), Ok(()));
        assert_eq!(
            may_request(&fixed, "a"),
            Err(OwnershipRefusal::AlreadyOwner)
        );
        assert_eq!(may_request(&fixed, "c"), Err(OwnershipRefusal::NotOnParty));

        let request = OwnerRequest::open("r".into(), "f".into(), "b".into(), None, now());
        let soon = now() + TimeDelta::hours(1);
        assert_eq!(may_decide(&fixed, &request, "a", false, true, soon), Ok(()));
        assert_eq!(
            may_decide(&fixed, &request, "b", false, true, soon),
            Err(OwnershipRefusal::NotOwner),
            "a requester cannot accept their own request"
        );
        assert_eq!(may_decide(&fixed, &request, "x", true, false, soon), Ok(()));
        let late = request.expires_at;
        assert_eq!(
            may_decide(&fixed, &request, "a", false, true, late),
            Err(OwnershipRefusal::Expired)
        );
        // The requester left the party: it can be declined, not accepted.
        let left = timing(&["a"]);
        assert_eq!(
            may_decide(&left, &request, "a", false, true, soon),
            Err(OwnershipRefusal::NotOnParty)
        );
        assert_eq!(may_decide(&left, &request, "a", false, false, soon), Ok(()));
        assert_eq!(may_withdraw(&request, "b", soon), Ok(()));
        assert_eq!(
            may_withdraw(&request, "a", soon),
            Err(OwnershipRefusal::NotRequester)
        );
    }
}
