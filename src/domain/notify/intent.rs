//! Typed notification intents: what to post where, keyed by the native row it
//! binds, for a delivery journal to execute at most once.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::digest::DigestInclusion;
use crate::domain::schedule::Notice;
use crate::domain::time::{DateOutOfRange, to_iso};

/// The native row a delivered message binds to; also the journal's dedupe key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeliveryTarget {
    /// A reminder row, by id.
    Reminder(String),
    /// A boss week's digest, by week start.
    Digest(DateTime<Utc>),
    /// A proposal's card, by proposal (draft) id (v4 `DeliveryTarget.card`).
    Card(String),
}

impl DeliveryTarget {
    /// v4 `BindingType` spelling.
    pub fn binding_type(&self) -> &'static str {
        match self {
            Self::Reminder(_) => "reminder",
            Self::Digest(_) => "digest",
            Self::Card(_) => "card",
        }
    }

    /// v4 `key_primary`: the reminder id, or the week start as UTC ISO text.
    ///
    /// # Errors
    /// [`DateOutOfRange`] for a week start outside v4's years.
    pub fn key_primary(&self) -> Result<String, DateOutOfRange> {
        match self {
            Self::Reminder(id) | Self::Card(id) => Ok(id.clone()),
            Self::Digest(week) => to_iso(week),
        }
    }
}

/// What the journal already knows about earlier attempts.
pub trait JournalView {
    /// True while an unresolved (possibly delivered) attempt claims `target`;
    /// such a target is never sent again.
    fn holds(&self, target: &DeliveryTarget) -> bool;
}

impl JournalView for BTreeSet<DeliveryTarget> {
    fn holds(&self, target: &DeliveryTarget) -> bool {
        self.contains(target)
    }
}

/// The v4 journal effect kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    Reminder,
    Digest,
    /// A proposal card (v4 effect kind `card`).
    Card,
    /// A change notice, e.g. `notice.run.status.cancelled`.
    Notice(String),
}

impl EffectKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Reminder => "reminder",
            Self::Digest => "digest",
            Self::Card => "card",
            Self::Notice(kind) => kind,
        }
    }
}

/// What the message is about; rendering reads the rows by these ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntentContent {
    /// One morning card for a channel's runs, in time order.
    DayOf {
        run_ids: Vec<String>,
    },
    Countdown {
        run_id: String,
        minutes: i64,
    },
    Digest {
        week_start: DateTime<Utc>,
        inclusion: DigestInclusion,
    },
    /// A schedule change notice, rendered from the event itself.
    Notice(Notice),
    /// One card for these proposals, rendered from their stored details.
    ProposalCard {
        proposal_ids: Vec<String>,
    },
    /// A message its sender renders (a self-service link, an approval problem).
    Plain,
}

/// Admin-visible problems found while planning a delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryWarning {
    /// v5 deviation: the runs' home channel is unset or unreachable, so the
    /// post went to the configured post channel and binds there.
    HomeChannelUnavailable {
        home_channel_id: Option<String>,
        run_ids: Vec<String>,
    },
}

/// One outbound post.
///
/// Executor obligations the plan cannot enforce:
/// * the delivery journal records `channel_id` (the channel actually used,
///   including a post-channel fallback), because later card edits and
///   reaction lookups must address that channel, not the run's home channel;
/// * planned [`DeliveryTarget`] rows may be retired between planning and
///   sending, so the executor re-checks under its lease that each still exists
///   (and is still unsent) before sending and binding;
/// * `mentions` is already quiet-gated, but rendering in quiet mode also needs
///   the quiet flag itself for v4's `quiet_line` text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationIntent {
    pub effect: EffectKind,
    /// v4 `effect_context`: what makes an operation-scoped post distinct.
    /// Empty for posts keyed by their [`DeliveryTarget`]s.
    pub effect_context: Vec<String>,
    /// The channel actually used; bindings are validated against it.
    pub channel_id: String,
    /// Native rows bound on delivery, in plan order; empty for notices.
    pub targets: Vec<DeliveryTarget>,
    /// The user allow-list after the quiet-mode gate, in the journal's
    /// canonical form (sorted text, unique). Roles and `@everyone` are never
    /// allowed.
    pub mentions: Vec<String>,
    pub content: IntentContent,
    pub warnings: Vec<DeliveryWarning>,
}

/// v4 `SendPayload`'s canonical allow-list: sorted as text, duplicates dropped.
pub(crate) fn canonical_allow_list(users: &[String]) -> Vec<String> {
    let mut users = users.to_vec();
    users.sort_unstable();
    users.dedup();
    users
}

/// Whether the journal should attempt an intent at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendDisposition {
    Send,
    /// An earlier unresolved attempt holds one of the targets.
    Suppressed,
}

/// An intent and whether to attempt it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedSend {
    pub intent: NotificationIntent,
    pub disposition: SendDisposition,
}

impl PlannedSend {
    pub(crate) fn new(intent: NotificationIntent, journal: &dyn JournalView) -> Self {
        let disposition = if intent.targets.iter().any(|target| journal.holds(target)) {
            SendDisposition::Suppressed
        } else {
            SendDisposition::Send
        };
        Self {
            intent,
            disposition,
        }
    }
}

/// Where a post can go.
pub trait ChannelDirectory {
    /// True when the bot can post in this channel now.
    fn is_reachable(&self, channel_id: &str) -> bool;
}

impl ChannelDirectory for BTreeSet<String> {
    fn is_reachable(&self, channel_id: &str) -> bool {
        self.contains(channel_id)
    }
}

/// The destination chosen for a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelChoice {
    /// The requested channel.
    Requested(String),
    /// The configured post channel stood in for an unset/unreachable one.
    Fallback {
        channel_id: String,
        requested: Option<String>,
    },
    /// Nowhere to post; the rows stay queued.
    Unavailable,
}

/// v4 `find_channel`: the requested channel, else the post channel.
pub fn choose_channel(
    requested: Option<&str>,
    post_channel_id: Option<&str>,
    channels: &dyn ChannelDirectory,
) -> ChannelChoice {
    if let Some(channel) = requested.filter(|id| channels.is_reachable(id)) {
        return ChannelChoice::Requested(channel.to_owned());
    }
    match post_channel_id.filter(|id| channels.is_reachable(id)) {
        Some(channel) => ChannelChoice::Fallback {
            channel_id: channel.to_owned(),
            requested: requested.map(str::to_owned),
        },
        None => ChannelChoice::Unavailable,
    }
}
