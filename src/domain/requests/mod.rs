//! Member requests: schedule changes a member asks for and an administrator
//! approves (merges), edits or rejects. A request is a draft of kind
//! `request`; this module holds the pure rules (types, operations,
//! authorisation, requester notices). The service lives on
//! `SchedulerService` beside the draft methods.
//!
//! Direct member writes (their own RSVP, this-boss-week moves of their own
//! runs) are not requests: they stay direct edits with preconditions.

use std::fmt;

use chrono::TimeDelta;

use crate::domain::drafts::{DraftOp, RequestLimits, Target};
use crate::domain::members::Directory;
use crate::domain::schedule::{
    AmendedRunChoice, FixedEdit, FixedEditChoices, NewFixedRun, Notice, NoticeChange,
    RequestDecision, ScheduleSnapshot,
};

/// Defaults (admin-adjustable later): three undecided requests per member
/// and six submissions per rolling 24 hours.
pub const DEFAULT_LIMITS: RequestLimits = RequestLimits {
    max_pending: 3,
    max_per_window: 6,
    window: TimeDelta::hours(24),
};

/// Anti-abuse freeze: a frozen member can neither submit nor resubmit.
pub trait MemberGate {
    fn is_frozen(&self, user_id: &str) -> bool;
}

/// Nobody is frozen.
pub struct NoFreezes;

impl MemberGate for NoFreezes {
    fn is_frozen(&self, _user_id: &str) -> bool {
        false
    }
}

/// What a request is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Run(String),
    Fixed(String),
}

impl Subject {
    /// The stored `subject` column: `run:<id>` or `fixed:<id>`.
    pub fn key(&self) -> String {
        match self {
            Self::Run(id) => format!("run:{id}"),
            Self::Fixed(id) => format!("fixed:{id}"),
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        if let Some(id) = key.strip_prefix("run:") {
            Some(Self::Run(id.to_owned()))
        } else {
            key.strip_prefix("fixed:")
                .map(|id| Self::Fixed(id.to_owned()))
        }
    }
}

/// A request type, as stored in `request_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestType {
    NewFixed,
    ChangeFixed,
    Join,
    Leave,
    Swap,
}

impl RequestType {
    pub const ALL: &[Self] = &[
        Self::NewFixed,
        Self::ChangeFixed,
        Self::Join,
        Self::Leave,
        Self::Swap,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewFixed => "new_fixed",
            Self::ChangeFixed => "change_fixed",
            Self::Join => "join",
            Self::Leave => "leave",
            Self::Swap => "swap",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str() == value)
    }
}

/// What a member asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestSpec {
    /// A new weekly run; the requester must own it and be on it.
    NewFixed(NewFixedRun),
    /// Change a weekly timing's day, time, party or channel. Per-run
    /// choices for amended runs are made at approval.
    ChangeFixed {
        fixed_id: String,
        edit: FixedEdit,
    },
    Join(Subject),
    Leave(Subject),
    /// The requester leaves and `with` takes their place.
    Swap {
        subject: Subject,
        with: String,
    },
}

impl RequestSpec {
    pub fn request_type(&self) -> RequestType {
        match self {
            Self::NewFixed(_) => RequestType::NewFixed,
            Self::ChangeFixed { .. } => RequestType::ChangeFixed,
            Self::Join(_) => RequestType::Join,
            Self::Leave(_) => RequestType::Leave,
            Self::Swap { .. } => RequestType::Swap,
        }
    }

    pub fn subject(&self) -> Option<Subject> {
        match self {
            Self::NewFixed(_) => None,
            Self::ChangeFixed { fixed_id, .. } => Some(Subject::Fixed(fixed_id.clone())),
            Self::Join(subject) | Self::Leave(subject) | Self::Swap { subject, .. } => {
                Some(subject.clone())
            }
        }
    }
}

/// Why a request action was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestRefusal {
    /// Not a guild member with the bossing role, not on the subject, or
    /// not the request's author.
    RequesterUnauthorised,
    /// Only administrators decide requests.
    NotAdmin,
    Frozen,
    UnknownSubject,
    /// A `change_fixed` request may change only day, time, party or channel.
    FieldNotAllowed,
    /// A `change_fixed` approval must carry per-run choices (or `UpdateAll`).
    ChoicesRequired,
    /// Choices only apply to `change_fixed`.
    ChoicesNotApplicable,
    /// A join or swap would add someone already on the run or timing.
    AlreadyInParty,
    /// A rejection needs a reason.
    ReasonRequired,
    /// A reason over 500 characters or with control characters.
    ReasonInvalid,
}

impl fmt::Display for RequestRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RequesterUnauthorised => "the requester may not make this request",
            Self::NotAdmin => "only administrators decide requests",
            Self::Frozen => "the member is frozen",
            Self::UnknownSubject => "no such run or weekly run",
            Self::FieldNotAllowed => "a change request may change only day, time, party or channel",
            Self::ChoicesRequired => "approving a change needs a choice for each moved run",
            Self::ChoicesNotApplicable => "choices apply only to change requests",
            Self::AlreadyInParty => "that member is already on it",
            Self::ReasonRequired => "a rejection needs a reason",
            Self::ReasonInvalid => "a reason is at most 500 characters without control characters",
        })
    }
}

fn on_roster(directory: &(dyn Directory + Sync), user_id: &str) -> bool {
    directory
        .member(user_id)
        .is_some_and(|member| member.has_role && !member.is_bot)
}

fn participants_of(snapshot: &ScheduleSnapshot, subject: &Subject) -> Option<Vec<String>> {
    match subject {
        Subject::Run(id) => snapshot
            .runs
            .iter()
            .find(|run| &run.id == id)
            .map(|run| run.participants.clone()),
        Subject::Fixed(id) => snapshot
            .fixed_runs
            .iter()
            .find(|row| &row.id == id)
            .map(|row| row.participants.clone()),
    }
}

/// May `requester` make (or still have approved) a request of this type on
/// `subject`, on `snapshot`? Join needs a guild member with the bossing
/// role; every other type needs a participant of the subject.
///
/// # Errors
/// [`RequestRefusal::RequesterUnauthorised`] or
/// [`RequestRefusal::UnknownSubject`].
pub fn authorise(
    kind: RequestType,
    subject: Option<&Subject>,
    requester: &str,
    snapshot: &ScheduleSnapshot,
    directory: &(dyn Directory + Sync),
) -> Result<(), RequestRefusal> {
    if !on_roster(directory, requester) {
        return Err(RequestRefusal::RequesterUnauthorised);
    }
    let Some(subject) = subject else {
        return Ok(());
    };
    let participants = participants_of(snapshot, subject).ok_or(RequestRefusal::UnknownSubject)?;
    match kind {
        RequestType::Join | RequestType::NewFixed => Ok(()),
        RequestType::ChangeFixed | RequestType::Leave | RequestType::Swap => {
            if participants.iter().any(|user| user == requester) {
                Ok(())
            } else {
                Err(RequestRefusal::RequesterUnauthorised)
            }
        }
    }
}

/// The draft operations a request stands for, on `snapshot`. Weekly-timing
/// changes use `UpdateAll` until an administrator chooses at approval.
///
/// # Errors
/// [`RequestRefusal`] for a malformed request.
pub fn operations(
    spec: &RequestSpec,
    requester: &str,
    snapshot: &ScheduleSnapshot,
) -> Result<Vec<DraftOp>, RequestRefusal> {
    let me = requester.to_owned();
    // Join and swap may not add someone already on the subject.
    let not_on = |subject: &Subject, user: &String| {
        let party = participants_of(snapshot, subject).ok_or(RequestRefusal::UnknownSubject)?;
        if party.contains(user) {
            Err(RequestRefusal::AlreadyInParty)
        } else {
            Ok(())
        }
    };
    // Parties change by delta (never a whole list), so concurrent requests
    // on one run or timing all apply.
    let delta = |subject: &Subject, remove: Vec<String>, add: Vec<String>| match subject {
        Subject::Run(run_id) => DraftOp::SwapParticipants {
            run: Target::Existing(run_id.clone()),
            remove,
            add,
            via_portal: true,
        },
        Subject::Fixed(fixed_id) => DraftOp::FixedParticipants {
            fixed: Target::Existing(fixed_id.clone()),
            add,
            remove,
        },
    };
    Ok(vec![match spec {
        RequestSpec::NewFixed(new) => {
            if new.owner_id != requester || !new.participants.contains(&me) {
                return Err(RequestRefusal::RequesterUnauthorised);
            }
            DraftOp::AddFixedRun(new.clone())
        }
        RequestSpec::ChangeFixed { fixed_id, edit } => {
            if edit.bosses.is_some() || edit.note.is_some() {
                return Err(RequestRefusal::FieldNotAllowed);
            }
            DraftOp::ApplyFixedEdit {
                fixed: Target::Existing(fixed_id.clone()),
                edit: edit.clone(),
                choices: FixedEditChoices::UpdateAll,
            }
        }
        RequestSpec::Join(subject) => {
            not_on(subject, &me)?;
            delta(subject, Vec::new(), vec![me])
        }
        RequestSpec::Leave(subject) => delta(subject, vec![me], Vec::new()),
        RequestSpec::Swap { subject, with } => {
            not_on(subject, with)?;
            delta(subject, vec![me], vec![with.clone()])
        }
    }])
}

/// The longest rejection reason, in characters.
pub const MAX_REASON: usize = 500;

/// Characters free text may not carry: controls (Cc), format characters
/// (Cf: bidi overrides, zero-width, U+FEFF, tags) and the line and paragraph
/// separators (Zl, Zp). Cf is listed per Unicode 15.1.
pub fn is_hidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        )
}

/// A rejection reason, trimmed: 1..=500 characters, none of them hidden
/// ([`is_hidden_char`]).
///
/// # Errors
/// [`RequestRefusal::ReasonRequired`] or [`RequestRefusal::ReasonInvalid`].
pub fn check_reason(reason: &str) -> Result<String, RequestRefusal> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(RequestRefusal::ReasonRequired);
    }
    if reason.chars().count() > MAX_REASON || reason.chars().any(is_hidden_char) {
        return Err(RequestRefusal::ReasonInvalid);
    }
    Ok(reason.to_owned())
}

/// The public description of a request's merge, generated from its type
/// and subject: member free text (the title) never reaches a channel.
pub fn public_summary(kind: RequestType, subject: Option<&Subject>) -> String {
    match subject {
        Some(subject) => format!("member request: {} {}", kind.as_str(), subject.key()),
        None => format!("member request: {}", kind.as_str()),
    }
}

/// How an approval's per-run choices are recorded in the `merged` event.
pub fn choices_note(choices: &FixedEditChoices) -> String {
    match choices {
        FixedEditChoices::UpdateAll => "choices=update_all".to_owned(),
        FixedEditChoices::PerRun(per_run) => {
            let picks: Vec<String> = per_run
                .iter()
                .map(|(run, pick)| {
                    let pick = match pick {
                        AmendedRunChoice::UpdateToFixed => "update_to_fixed",
                        AmendedRunChoice::KeepForThisWeek => "keep_for_this_week",
                    };
                    format!("{run}:{pick}")
                })
                .collect();
            format!("choices={}", picks.join(","))
        }
    }
}

/// The notice telling the requester what became of their request: it lists
/// only them, so it mentions nobody else. Posted in `channel_id` (the
/// subject's home channel) when set.
pub fn requester_notice(
    request_id: &str,
    requester: &str,
    decision: RequestDecision,
    reason: Option<String>,
    channel_id: Option<String>,
) -> Notice {
    Notice {
        change: NoticeChange::RequestDecided {
            request: request_id.to_owned(),
            decision,
            reason,
        },
        channel_id,
        listed: vec![requester.to_owned()],
        via_portal: true,
    }
}

/// The home channel a request's notice goes to: its subject's, or a new
/// weekly run's.
pub fn home_channel(
    subject: Option<&Subject>,
    ops: &[DraftOp],
    snapshot: &ScheduleSnapshot,
) -> Option<String> {
    match subject {
        Some(Subject::Run(id)) => snapshot
            .runs
            .iter()
            .find(|run| &run.id == id)
            .and_then(|run| run.channel_id.clone()),
        Some(Subject::Fixed(id)) => snapshot
            .fixed_runs
            .iter()
            .find(|row| &row.id == id)
            .and_then(|row| row.channel_id.clone()),
        None => ops.iter().find_map(|op| match op {
            DraftOp::AddFixedRun(new) => new.channel_id.clone(),
            _ => None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_and_types_round_trip() {
        for subject in [Subject::Run("r".into()), Subject::Fixed("f".into())] {
            assert_eq!(Subject::parse(&subject.key()), Some(subject));
        }
        assert_eq!(Subject::parse("other:x"), None);
        for kind in RequestType::ALL {
            assert_eq!(RequestType::parse(kind.as_str()), Some(*kind));
        }
    }

    #[test]
    fn notices_list_only_the_requester() {
        let notice = requester_notice("q-1", "7", RequestDecision::Rejected, None, None);
        assert_eq!(notice.listed, ["7"]);
        assert_eq!(notice.effect_kind(), "notice.request.rejected");
        assert_eq!(notice.effect_context(), ["request:q-1:rejected"]);
    }
}
