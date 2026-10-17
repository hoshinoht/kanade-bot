//! Pure rules: when a link (or a character's stored estimates) no longer
//! matches its roster character, how a Nexon error code backs off, when the
//! daily batch runs and the History row of a link change.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use chrono_tz::Asia::Singapore;

use super::model::{LINK_REGION, Link, LinkStatus, Snapshot};
use crate::domain::characters::{CHARACTERS_SECTION, Character, DEFAULT_WORLD, ign_key};
use crate::domain::history::{Actor, Surface};
use crate::domain::members::MemberProfile;
use crate::domain::settings::{RowDiff, SettingsChange};

/// The daily refresh batch's wall-clock time in Singapore time (the Open
/// API's reference zone).
pub const BATCH_TIME: NaiveTime = match NaiveTime::from_hms_opt(4, 0, 0) {
    Some(time) => time,
    None => panic!("04:00 is a time"),
};

/// What the orphan pass does with a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orphan {
    Keep,
    /// Delete it and its data: the identity it was made for is gone.
    Unlink(UnlinkReason),
    /// Back to `pending`: the member has not confirmed the current notice.
    Reconsent,
}

spelled! {
    /// Why a link was removed; logged as this code, never with names.
    UnlinkReason {
        /// An admin or the member asked.
        Requested => "requested",
        CharacterGone => "character_gone",
        /// The roster IGN, region or member changed.
        IdentityChanged => "identity_changed",
        MemberDeparted => "member_departed",
        /// Retired and its data has expired.
        RetiredExpired => "retired_expired",
        /// A pending link the member did not confirm within
        /// [`super::PENDING_TTL`].
        PendingExpired => "pending_expired",
        /// The member's confirmation found no such character in the Open API:
        /// nothing is kept, the request included.
        NotFound => "not_found",
    }
}

/// A pending request older than [`super::PENDING_TTL`] at `now`: the
/// sweeper removes it.
pub fn pending_expired(link: &Link, now: DateTime<Utc>) -> bool {
    link.status == LinkStatus::Pending && link.requested_at + super::PENDING_TTL <= now
}

/// The roster character still names what `link` was made for: the same
/// member, IGN (case-insensitively) and the link region.
pub fn same_identity(link: &Link, character: &Character) -> bool {
    character.member_id == link.member_id
        && ign_key(&character.ign) == link.ign_key
        && character.world == LINK_REGION
}

/// A member row as the gateway leaves it on departure: no bossing role, no
/// roles at all and no Administrator.
pub fn has_departed(member: &MemberProfile) -> bool {
    !member.member.has_role && member.roles.is_empty() && !member.is_guild_admin
}

/// The orphan pass over one link: its roster character (`None` when
/// deleted), its member row and its fresh snapshot, if any.
pub fn orphan(
    link: &Link,
    character: Option<&Character>,
    member: Option<&MemberProfile>,
    fresh: Option<&Snapshot>,
) -> Orphan {
    let Some(character) = character else {
        return Orphan::Unlink(UnlinkReason::CharacterGone);
    };
    if !same_identity(link, character) {
        return Orphan::Unlink(UnlinkReason::IdentityChanged);
    }
    if member.is_none_or(has_departed) {
        return Orphan::Unlink(UnlinkReason::MemberDeparted);
    }
    if !character.is_active() && fresh.is_none() {
        return Orphan::Unlink(UnlinkReason::RetiredExpired);
    }
    if link.needs_reconsent() {
        return Orphan::Reconsent;
    }
    Orphan::Keep
}

/// The sweep over one character's stored estimates: its own result and
/// each party total naming it, given by the IGN set each is held under (a
/// result's set is its IGN key). They go when the character is gone or
/// retired, is no longer on MapleSEA, its member left (as [`orphan`]
/// judges), or its roster IGN is missing from one of the sets (renamed).
/// A NEXON link plays no part: estimates come from the roster IGN.
pub fn estimates_orphaned(
    set_keys: &[String],
    character: Option<&Character>,
    member: Option<&MemberProfile>,
) -> bool {
    let Some(character) = character else {
        return true;
    };
    let key = character.ign_key();
    !character.is_active()
        || character.world != DEFAULT_WORLD
        || member.is_none_or(has_departed)
        || set_keys
            .iter()
            .any(|set| !set.split(',').any(|ign| ign == key))
}

/// How a failed Nexon call is treated (feasibility note §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// `OPENAPI00007` (429): retried 1 s, 2 s with jitter, at most 3
    /// tries; a second one in a batch ends the batch for the day.
    RateLimited,
    /// `OPENAPI00009`: this character again in 15 minutes.
    Preparing,
    /// `OPENAPI00001`/`00010`/`00011`, any 5xx or no answer: every call
    /// held 30 minutes, then hourly while it lasts.
    Maintenance,
    /// `OPENAPI00002`/`00005`: no call at all until a restart.
    KeyRefused,
    /// `OPENAPI00003`: the ocid is resolved again once, else the link is
    /// stale.
    InvalidIdentifier,
    /// `OPENAPI00004`/`00006` or anything unexpected: logged, not retried.
    Bug,
}

impl Failure {
    /// An `OPENAPI000NN` code (or `None`) and the HTTP status.
    pub fn classify(code: Option<&str>, status: Option<u16>) -> Self {
        match code {
            Some("OPENAPI00007") => Self::RateLimited,
            Some("OPENAPI00009") => Self::Preparing,
            Some("OPENAPI00001" | "OPENAPI00010" | "OPENAPI00011") => Self::Maintenance,
            Some("OPENAPI00002" | "OPENAPI00005") => Self::KeyRefused,
            Some("OPENAPI00003") => Self::InvalidIdentifier,
            Some(_) => Self::Bug,
            None => match status {
                Some(429) => Self::RateLimited,
                Some(500..=599) | None => Self::Maintenance,
                Some(_) => Self::Bug,
            },
        }
    }
}

/// Tries per request on `OPENAPI00007`.
pub const RATE_LIMIT_TRIES: u32 = 3;
/// `OPENAPI00009`: the character's next try.
pub const PREPARING_RETRY: TimeDelta = TimeDelta::minutes(15);
/// The first maintenance hold, then [`MAINTENANCE_AGAIN`] while it lasts.
pub const MAINTENANCE_FIRST: TimeDelta = TimeDelta::minutes(30);
pub const MAINTENANCE_AGAIN: TimeDelta = TimeDelta::hours(1);

/// The wait before try `try_number + 1` after a 429 (before jitter):
/// 1 s, 2 s, 4 s … capped at 60 s.
pub fn rate_limit_delay(try_number: u32) -> std::time::Duration {
    let seconds = 1u64 << try_number.saturating_sub(1).min(6);
    std::time::Duration::from_secs(seconds.min(60))
}

/// The next daily batch strictly after `now`: [`BATCH_TIME`] in Singapore.
pub fn next_batch(now: DateTime<Utc>) -> DateTime<Utc> {
    let local = now.with_timezone(&Singapore);
    let today = local.date_naive().and_time(BATCH_TIME);
    // Singapore has no DST: every local time exists exactly once.
    let at = |wall: chrono::NaiveDateTime| {
        wall.and_local_timezone(Singapore)
            .single()
            .map_or(now, |at| at.with_timezone(&Utc))
    };
    let first = at(today);
    if first > now {
        first
    } else {
        at(today + TimeDelta::days(1))
    }
}

/// The History row of a link moving from `before` to `after` (`None` is
/// unlinked): status words and the actor only, never a Nexon field.
pub fn link_change(
    character_id: &str,
    before: Option<LinkStatus>,
    after: Option<LinkStatus>,
    actor: &Actor,
    surface: Surface,
    at: DateTime<Utc>,
) -> Option<SettingsChange> {
    let (from, to) = (
        LinkStatus::audit_value(before),
        LinkStatus::audit_value(after),
    );
    (from != to).then(|| SettingsChange {
        id: 0,
        at,
        actor: actor.clone(),
        surface,
        section: CHARACTERS_SECTION.into(),
        revision: 0,
        values: BTreeMap::from([(
            format!("link.{character_id}"),
            RowDiff {
                from: from.into(),
                to: to.into(),
            },
        )]),
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::characters::CharacterRole;
    use crate::domain::chardata::NOTICE_VERSION;
    use crate::domain::members::{Member, PingLevel};

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
    }

    fn link() -> Link {
        Link {
            character_id: "c1".into(),
            member_id: "1001".into(),
            ign_key: "wisteria".into(),
            region: "msea".into(),
            status: LinkStatus::Active,
            requested_by: Actor::member("1001"),
            requested_at: at(1, 0),
            consent_version: Some(NOTICE_VERSION),
            consented_at: Some(at(1, 0)),
            linked_at: Some(at(1, 0)),
            consent_surface: None,
            version: 1,
        }
    }

    fn character() -> Character {
        Character {
            id: "c1".into(),
            member_id: "1001".into(),
            ign: "Wisteria".into(),
            world: "msea".into(),
            class: None,
            level: None,
            role: CharacterRole::Main,
            retired_at: None,
            created_at: at(1, 0),
            updated_at: at(1, 0),
            updated_by: Actor::admin("token"),
            version: 1,
        }
    }

    fn member(has_role: bool) -> MemberProfile {
        MemberProfile {
            member: Member {
                user_id: "1001".into(),
                display_name: Some("Alice".into()),
                nickname: None,
                has_role,
                is_bot: false,
                ping_level: PingLevel::default(),
            },
            aliases: Vec::new(),
            reply_style: None,
            roles: if has_role {
                vec!["10".into()]
            } else {
                Vec::new()
            },
            is_guild_admin: false,
            ..MemberProfile::default()
        }
    }

    #[test]
    fn orphan_rules() {
        let (link, row, here) = (link(), character(), member(true));
        assert_eq!(orphan(&link, Some(&row), Some(&here), None), Orphan::Keep);
        assert_eq!(
            orphan(&link, None, Some(&here), None),
            Orphan::Unlink(UnlinkReason::CharacterGone)
        );
        for edited in [
            Character {
                ign: "Heather".into(),
                ..character()
            },
            Character {
                world: "kms".into(),
                ..character()
            },
            Character {
                member_id: "1002".into(),
                ..character()
            },
        ] {
            assert_eq!(
                orphan(&link, Some(&edited), Some(&here), None),
                Orphan::Unlink(UnlinkReason::IdentityChanged)
            );
        }
        // Letter case alone is the same IGN.
        let cased = Character {
            ign: "WISTERIA".into(),
            ..character()
        };
        assert_eq!(orphan(&link, Some(&cased), Some(&here), None), Orphan::Keep);
        assert_eq!(
            orphan(&link, Some(&row), Some(&member(false)), None),
            Orphan::Unlink(UnlinkReason::MemberDeparted)
        );
        assert_eq!(
            orphan(&link, Some(&row), None, None),
            Orphan::Unlink(UnlinkReason::MemberDeparted)
        );
        let retired = Character {
            retired_at: Some(at(2, 0)),
            ..character()
        };
        assert_eq!(
            orphan(&link, Some(&retired), Some(&here), None),
            Orphan::Unlink(UnlinkReason::RetiredExpired)
        );
        let old = Link {
            consent_version: Some(NOTICE_VERSION - 1),
            ..link.clone()
        };
        assert_eq!(
            orphan(&old, Some(&row), Some(&here), None),
            Orphan::Reconsent
        );
        let pending = Link {
            status: LinkStatus::Pending,
            consent_version: None,
            consented_at: None,
            linked_at: None,
            ..link
        };
        assert_eq!(
            orphan(&pending, Some(&row), Some(&here), None),
            Orphan::Keep
        );
    }

    #[test]
    fn estimates_orphan_rules() {
        let (row, here) = (character(), member(true));
        let held = |sets: &[&str]| -> Vec<String> { sets.iter().map(|&set| set.into()).collect() };
        let own = held(&["wisteria", "heather,wisteria"]);
        assert!(!estimates_orphaned(&own, Some(&row), Some(&here)));
        let cased = Character {
            ign: "WISTERIA".into(),
            ..character()
        };
        assert!(
            !estimates_orphaned(&own, Some(&cased), Some(&here)),
            "letter case alone is the same IGN"
        );
        assert!(estimates_orphaned(&own, None, Some(&here)), "gone");
        let retired = Character {
            retired_at: Some(at(2, 0)),
            ..character()
        };
        assert!(estimates_orphaned(&own, Some(&retired), Some(&here)));
        let elsewhere = Character {
            world: "kms".into(),
            ..character()
        };
        assert!(estimates_orphaned(&own, Some(&elsewhere), Some(&here)));
        let renamed = Character {
            ign: "Lavender".into(),
            ..character()
        };
        assert!(estimates_orphaned(&own, Some(&renamed), Some(&here)));
        assert!(
            estimates_orphaned(&held(&["heather,lavender"]), Some(&row), Some(&here)),
            "a party total from before a rename"
        );
        assert!(estimates_orphaned(&own, Some(&row), Some(&member(false))));
        assert!(estimates_orphaned(&own, Some(&row), None), "no member row");
    }

    #[test]
    fn every_error_code_has_its_backoff() {
        for (code, failure) in [
            ("OPENAPI00001", Failure::Maintenance),
            ("OPENAPI00002", Failure::KeyRefused),
            ("OPENAPI00003", Failure::InvalidIdentifier),
            ("OPENAPI00004", Failure::Bug),
            ("OPENAPI00005", Failure::KeyRefused),
            ("OPENAPI00006", Failure::Bug),
            ("OPENAPI00007", Failure::RateLimited),
            ("OPENAPI00009", Failure::Preparing),
            ("OPENAPI00010", Failure::Maintenance),
            ("OPENAPI00011", Failure::Maintenance),
            ("OPENAPI00099", Failure::Bug),
        ] {
            assert_eq!(Failure::classify(Some(code), Some(400)), failure, "{code}");
        }
        assert_eq!(Failure::classify(None, Some(503)), Failure::Maintenance);
        assert_eq!(Failure::classify(None, None), Failure::Maintenance);
        assert_eq!(Failure::classify(None, Some(429)), Failure::RateLimited);
        assert_eq!(Failure::classify(None, Some(404)), Failure::Bug);
        let delays: Vec<u64> = (1..=8).map(|n| rate_limit_delay(n).as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60]);
    }

    #[test]
    fn the_batch_runs_at_four_in_singapore() {
        // 04:00 SGT is 20:00 UTC the day before.
        assert_eq!(next_batch(at(10, 12)), at(10, 20));
        assert_eq!(next_batch(at(10, 20)), at(11, 20));
        assert_eq!(next_batch(at(10, 19)), at(10, 20));
    }

    #[test]
    fn a_pending_request_expires_after_fourteen_days() {
        let pending = Link {
            status: LinkStatus::Pending,
            consent_version: None,
            consented_at: None,
            linked_at: None,
            ..link()
        };
        let asked = pending.requested_at;
        assert!(!pending_expired(
            &pending,
            asked + TimeDelta::days(14) - TimeDelta::seconds(1)
        ));
        assert!(pending_expired(&pending, asked + TimeDelta::days(14)));
        // Only requests: an active link never expires this way.
        assert!(!pending_expired(&link(), asked + TimeDelta::days(60)));
    }

    #[test]
    fn link_changes_record_status_words_only() {
        let change = link_change(
            "c1",
            None,
            Some(LinkStatus::Pending),
            &Actor::admin("token"),
            Surface::AdminPortal,
            at(1, 0),
        )
        .unwrap();
        assert_eq!(change.section, "characters");
        let diff = &change.values["link.c1"];
        assert_eq!(
            (diff.from.as_str(), diff.to.as_str()),
            ("unlinked", "pending")
        );
        assert!(
            link_change(
                "c1",
                Some(LinkStatus::Active),
                Some(LinkStatus::Active),
                &Actor::admin("token"),
                Surface::AdminPortal,
                at(1, 0)
            )
            .is_none()
        );
    }
}
