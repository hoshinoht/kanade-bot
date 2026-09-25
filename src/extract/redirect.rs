//! Self-service redirect (user decision 2026-09-24): classify a detected
//! change and decide whether its ✅ card is kept and which pre-filled public
//! portal link goes with it. Pure: the caller supplies the clock, the target
//! run and the configured mode; the weekly tip is claimed by `chat::nudge`.

use chrono::{DateTime, Datelike, NaiveTime, Timelike, Utc, Weekday};

use crate::chat::persona::NudgePurpose;
use crate::domain::notify::WeekReset;
use crate::domain::proposals::{ChangeKind, Payload, ProposedChange};
use crate::domain::schedule::Run;

/// Admin setting `self_service.mode` (guild-wide).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelfServiceMode {
    /// Launch default: the card stays and the link is added.
    #[default]
    CardsAndLink,
    /// Self-serviceable moves get only the link.
    LinkFirst,
    /// v4 behaviour: cards, no links.
    CardsOnly,
}

impl SelfServiceMode {
    pub const ALL: [Self; 3] = [Self::CardsAndLink, Self::LinkFirst, Self::CardsOnly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CardsAndLink => "cards_and_link",
            Self::LinkFirst => "link_first",
            Self::CardsOnly => "cards_only",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }
}

/// No links while the public portal is closed, whatever is configured.
pub fn effective_mode(configured: SelfServiceMode, public_portal_open: bool) -> SelfServiceMode {
    if public_portal_open {
        configured
    } else {
        SelfServiceMode::CardsOnly
    }
}

/// The four cases of the recorded decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirectCase {
    /// (a) The author moves a live run they are on, within the current boss week.
    SelfService,
    /// (b) Someone else's run, a multi-change message, a move out of the
    /// current boss week, or any other kind: the ✅ card as today.
    NeedsApproval,
    /// (c) A change to an existing weekly timing: pre-filled request form.
    FixedRun,
    /// (d) Low confidence, a question, or no target/time to pre-fill: today's
    /// card or clarifying question, no link.
    Unclear,
}

/// What the caller knows about one kept change.
#[derive(Clone, Copy, Debug)]
pub struct RedirectFacts<'a> {
    pub author_id: &'a str,
    pub change: &'a ProposedChange,
    /// The change's target run, when it has one.
    pub run: Option<&'a Run>,
    pub confidence: f64,
    pub is_question: bool,
    /// The extraction's own threshold; below it the change is unclear.
    pub min_confidence: f64,
    /// Changes the message (burst) produced; more than one is never redirected.
    pub changes_in_message: usize,
    pub reset: WeekReset,
    pub now: DateTime<Utc>,
}

pub fn classify(facts: &RedirectFacts<'_>) -> RedirectCase {
    let change = facts.change;
    let low = facts.confidence.is_nan() || facts.confidence < facts.min_confidence;
    if facts.is_question || low {
        return RedirectCase::Unclear;
    }
    if facts.changes_in_message > 1 {
        return RedirectCase::NeedsApproval;
    }
    match change.kind {
        ChangeKind::Fix => match &change.payload {
            Payload::FixEdit {
                fixed_run_id: Some(_),
                ..
            }
            | Payload::FixRemove {
                fixed_run_id: Some(_),
            } => RedirectCase::FixedRun,
            // A new timing has nothing to pre-fill a request against.
            Payload::Fix { .. } => RedirectCase::NeedsApproval,
            _ => RedirectCase::Unclear,
        },
        ChangeKind::Move => classify_move(facts),
        _ => RedirectCase::NeedsApproval,
    }
}

fn classify_move(facts: &RedirectFacts<'_>) -> RedirectCase {
    let (Some(run), Some(to)) = (facts.run, facts.change.new_datetime) else {
        return RedirectCase::Unclear;
    };
    if facts.change.run_id.as_deref() != Some(run.id.as_str()) {
        return RedirectCase::Unclear;
    }
    let on_run = run.participants.iter().any(|id| id == facts.author_id);
    if !on_run || !run.status.is_live() {
        return RedirectCase::NeedsApproval;
    }
    let (Ok(current), Ok(target)) = (
        facts.reset.current_week(facts.now),
        facts.reset.current_week(to),
    ) else {
        return RedirectCase::NeedsApproval;
    };
    if run.week_start == current && target == current {
        RedirectCase::SelfService
    } else {
        RedirectCase::NeedsApproval
    }
}

/// A requested change to a weekly timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedChange {
    Edit,
    Remove,
}

impl FixedChange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Remove => "remove",
        }
    }
}

/// Public-portal deep links. They carry only ids and the proposed slot, never
/// a secret; the portal signs the member in and the server re-validates.
pub trait PortalLinks {
    /// Opens the run with the move pre-filled for confirmation.
    fn move_run(&self, run_id: &str, to: DateTime<Utc>) -> String;
    /// Opens the request form for a weekly timing, pre-filled.
    fn request_fixed(
        &self,
        fixed_run_id: &str,
        change: FixedChange,
        weekday: Option<Weekday>,
        time: Option<NaiveTime>,
    ) -> String;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidOrigin;

impl std::fmt::Display for InvalidOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("public origin must be https://<host> with no path, query or credentials")
    }
}

impl std::error::Error for InvalidOrigin {}

/// Provisional route shapes (the public PWA routes are not specified yet):
/// `{origin}/runs/{run_id}?move_to=<UTC RFC 3339>` and
/// `{origin}/requests/new?fixed={id}&change={edit|remove}[&day=thu][&time=HH:MM]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicPortalLinks {
    origin: String,
}

impl PublicPortalLinks {
    pub fn new(origin: &str) -> Result<Self, InvalidOrigin> {
        let origin = origin.strip_suffix('/').unwrap_or(origin);
        let host = origin.strip_prefix("https://").ok_or(InvalidOrigin)?;
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
        if !valid {
            return Err(InvalidOrigin);
        }
        Ok(Self {
            origin: origin.to_ascii_lowercase(),
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }
}

impl PortalLinks for PublicPortalLinks {
    fn move_run(&self, run_id: &str, to: DateTime<Utc>) -> String {
        format!(
            "{}/runs/{}?move_to={}",
            self.origin,
            encode(run_id),
            utc_stamp(to)
        )
    }

    fn request_fixed(
        &self,
        fixed_run_id: &str,
        change: FixedChange,
        weekday: Option<Weekday>,
        time: Option<NaiveTime>,
    ) -> String {
        let mut link = format!(
            "{}/requests/new?fixed={}&change={}",
            self.origin,
            encode(fixed_run_id),
            change.as_str()
        );
        if let Some(weekday) = weekday {
            link.push_str("&day=");
            link.push_str(day_code(weekday));
        }
        if let Some(time) = time {
            link.push_str(&format!("&time={:02}:{:02}", time.hour(), time.minute()));
        }
        link
    }
}

/// RFC 3986 unreserved characters pass; every other byte is percent-encoded.
/// An all-dot id keeps its dots encoded so `.`/`..` segments cannot normalise.
fn encode(value: &str) -> String {
    let all_dots = !value.is_empty() && value.bytes().all(|byte| byte == b'.');
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'~')
            || (byte == b'.' && !all_dots);
        if keep {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn utc_stamp(at: DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

fn day_code(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedirectLink {
    pub purpose: NudgePurpose,
    pub url: String,
}

/// What to post for one change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    pub case: RedirectCase,
    /// `false` only for a self-service move in link-first mode.
    pub keep_card: bool,
    pub link: Option<RedirectLink>,
}

/// Applies the effective mode (see [`effective_mode`]) to the classification.
/// A link is only ever added to cases (a) and (c); in link-first mode only (a)
/// loses its card, since (c) still needs an approval.
pub fn plan<L: PortalLinks + ?Sized>(
    facts: &RedirectFacts<'_>,
    mode: SelfServiceMode,
    links: &L,
) -> Redirect {
    let case = classify(facts);
    let unchanged = Redirect {
        case,
        keep_card: true,
        link: None,
    };
    if mode == SelfServiceMode::CardsOnly {
        return unchanged;
    }
    let change = facts.change;
    match case {
        RedirectCase::SelfService => {
            let (Some(run_id), Some(to)) = (change.run_id.as_deref(), change.new_datetime) else {
                return unchanged;
            };
            Redirect {
                case,
                keep_card: mode != SelfServiceMode::LinkFirst,
                link: Some(RedirectLink {
                    purpose: NudgePurpose::SelfService,
                    url: links.move_run(run_id, to),
                }),
            }
        }
        RedirectCase::FixedRun => {
            let url = match &change.payload {
                Payload::FixEdit {
                    fixed_run_id: Some(id),
                    weekday,
                    time,
                    ..
                } => links.request_fixed(id, FixedChange::Edit, *weekday, *time),
                Payload::FixRemove {
                    fixed_run_id: Some(id),
                } => links.request_fixed(id, FixedChange::Remove, None, None),
                _ => return unchanged,
            };
            Redirect {
                case,
                keep_card: true,
                link: Some(RedirectLink {
                    purpose: NudgePurpose::RequestForm,
                    url,
                }),
            }
        }
        RedirectCase::NeedsApproval | RedirectCase::Unclear => unchanged,
    }
}
