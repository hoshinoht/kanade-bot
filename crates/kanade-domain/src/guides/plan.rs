//! Slot planning (4.3): from the published slot map and the rendered slots
//! to the ordered Discord effects that make the channel show the guide.
//!
//! A slot is identified by `(channel_id, position)`, so during a channel move
//! the new set and the old set coexist until the old messages are deleted.
//! Execution order is the op order: edits (top to bottom), then removals in
//! the target channel (newest first), then creates (in order), then the old
//! channel's removals (newest first, only after every create is confirmed).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::hash;
use super::render::{RenderedSlot, UploadRef};

crate::spelled! {
    /// What is known about a published slot's message.
    SlotState {
        /// Discord confirmed it; `message_id` is set.
        Posted => "posted",
        /// Deleted by hand (an edit's 404 or the channel check).
        Gone => "gone",
        /// A create that may have landed but was not found; blocks the guide
        /// until an admin resolves it (decision 11).
        Unknown => "unknown",
    }
}

crate::spelled! {
    /// A plan op's kind (`guide_publish_ops.kind`).
    OpKind {
        Edit => "edit",
        Create => "create",
        Delete => "delete",
        /// Drop a slot row without a Discord call (gone, or left behind by a
        /// channel move without `delete_old`).
        Forget => "forget",
    }
}

crate::spelled! {
    /// A channel permission a plan needs.
    Grant {
        View => "view",
        Send => "send",
        History => "history",
        Embed => "embed",
        Attach => "attach",
    }
}

/// One row of the published slot map (`guide_slots`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedSlot {
    pub position: u32,
    pub channel_id: String,
    /// Set when posted; kept on a gone slot when known; `None` when unknown.
    pub message_id: Option<String>,
    /// The [`RenderedSlot::hash`] Discord last confirmed.
    pub hash: String,
    /// The files the message carries, by file name.
    pub uploads: Vec<UploadRef>,
    pub state: SlotState,
    /// An unknown slot's journal source (`guide-post:<op uuid>`).
    pub source: Option<String>,
    /// The publish run that last wrote the row.
    pub publish_id: String,
    pub updated_at: DateTime<Utc>,
}

/// The bot's permissions in the target channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuideGrants {
    pub view: bool,
    pub send: bool,
    pub history: bool,
    pub embed: bool,
    pub attach: bool,
}

impl GuideGrants {
    pub fn has(&self, grant: Grant) -> bool {
        match grant {
            Grant::View => self.view,
            Grant::Send => self.send,
            Grant::History => self.history,
            Grant::Embed => self.embed,
            Grant::Attach => self.attach,
        }
    }
}

/// A live read of the target channel (4.4), taken by the delivery side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inspection {
    /// Deleted, or the bot cannot see it.
    ChannelMissing,
    Channel {
        grants: GuideGrants,
        /// Positions of target-channel slots whose message is gone.
        gone: BTreeSet<u32>,
        /// The channel's newest message, whoever wrote it.
        newest_message_id: Option<String>,
    },
}

/// One Discord effect (or, for `Forget`, slot-map change) of a plan. Edits
/// and creates take their output from the rendered slot at `position`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanOp {
    Edit {
        position: u32,
        channel_id: String,
        message_id: String,
        hash: String,
    },
    Create {
        position: u32,
        channel_id: String,
        hash: String,
    },
    Delete {
        position: u32,
        channel_id: String,
        message_id: String,
    },
    Forget {
        position: u32,
        channel_id: String,
    },
}

impl PlanOp {
    pub fn kind(&self) -> OpKind {
        match self {
            Self::Edit { .. } => OpKind::Edit,
            Self::Create { .. } => OpKind::Create,
            Self::Delete { .. } => OpKind::Delete,
            Self::Forget { .. } => OpKind::Forget,
        }
    }

    pub fn position(&self) -> u32 {
        match self {
            Self::Edit { position, .. }
            | Self::Create { position, .. }
            | Self::Delete { position, .. }
            | Self::Forget { position, .. } => *position,
        }
    }

    pub fn channel_id(&self) -> &str {
        match self {
            Self::Edit { channel_id, .. }
            | Self::Create { channel_id, .. }
            | Self::Delete { channel_id, .. }
            | Self::Forget { channel_id, .. } => channel_id,
        }
    }

    pub fn message_id(&self) -> Option<&str> {
        match self {
            Self::Edit { message_id, .. } | Self::Delete { message_id, .. } => Some(message_id),
            Self::Create { .. } | Self::Forget { .. } => None,
        }
    }

    /// The rendered slot hash an edit or create writes.
    pub fn hash(&self) -> Option<&str> {
        match self {
            Self::Edit { hash, .. } | Self::Create { hash, .. } => Some(hash),
            Self::Delete { .. } | Self::Forget { .. } => None,
        }
    }
}

/// What an admin must confirm before a plan runs (decisions 7-9). As a
/// plan's `needs`, the confirmations required; as a publish request's
/// `confirm`, what the admin confirmed (stored in `guide_publishes.confirmed`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmations {
    /// Posted messages from this position on are deleted and posted again.
    #[serde(default)]
    pub repost_from: Option<u32>,
    /// New messages go below messages that are not the guide's.
    #[serde(default)]
    pub interleaved: bool,
    /// The guide moves to another channel.
    #[serde(default)]
    pub channel_move: bool,
}

impl Confirmations {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether these confirmations cover `needs` (the same `repost_from`).
    pub fn covers(&self, needs: &Self) -> bool {
        (needs.repost_from.is_none() || self.repost_from == needs.repost_from)
            && (!needs.interleaved || self.interleaved)
            && (!needs.channel_move || self.channel_move)
    }
}

/// Why a plan cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// An unconfirmed create needs an admin first (5.3); no ops.
    UnknownSlot { channel_id: String, position: u32 },
    /// The target channel is gone or hidden; no ops.
    ChannelMissing,
    /// The bot lacks these grants in the target channel; the ops are kept
    /// for the preview but must not run.
    Grants(Vec<Grant>),
}

/// The plan sheet's counts. `delete` counts Discord deletes only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanCounts {
    pub keep: u32,
    pub edit: u32,
    pub create: u32,
    pub delete: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The target channel.
    pub channel_id: String,
    pub ops: Vec<PlanOp>,
    pub needs: Confirmations,
    pub blocked: Option<Blocked>,
    /// Planned with a live [`Inspection`] ("Unchecked" otherwise).
    pub checked: bool,
    pub counts: PlanCounts,
    /// [`hash::plan_hash`] of channel, ops and needs.
    pub hash: String,
}

impl Plan {
    /// Nothing to do: the channel already shows the guide.
    pub fn no_op(&self) -> bool {
        self.ops.is_empty() && self.blocked.is_none()
    }

    fn new(channel_id: &str, ops: Vec<PlanOp>, needs: Confirmations, checked: bool) -> Self {
        let hash = hash::plan_hash(channel_id, &ops, &needs);
        let mut counts = PlanCounts::default();
        for op in &ops {
            match op.kind() {
                OpKind::Edit => counts.edit += 1,
                OpKind::Create => counts.create += 1,
                OpKind::Delete => counts.delete += 1,
                OpKind::Forget => {}
            }
        }
        Self {
            channel_id: channel_id.to_owned(),
            ops,
            needs,
            blocked: None,
            checked,
            counts,
            hash,
        }
    }

    fn blocked(channel_id: &str, blocked: Blocked, checked: bool) -> Self {
        Self {
            blocked: Some(blocked),
            ..Self::new(channel_id, Vec::new(), Confirmations::default(), checked)
        }
    }
}

/// Discord snowflakes as decimal text: longer is newer, then by text.
fn newer(a: &str, b: &str) -> bool {
    (a.len(), a) > (b.len(), b)
}

/// A target-channel slot as the plan sees it: posted only with a message
/// id and not reported gone by the inspection.
struct Current<'a> {
    slot: &'a PublishedSlot,
    message_id: Option<&'a str>,
}

impl Current<'_> {
    fn same(&self, desired: &RenderedSlot) -> bool {
        self.message_id.is_some() && self.slot.hash == desired.hash
    }

    /// An edit can turn it into `desired`: it cannot add or remove files.
    fn editable(&self, desired: &RenderedSlot) -> bool {
        self.message_id.is_some() && desired.same_uploads(&self.slot.uploads)
    }
}

/// Remove a slot: a Discord delete while it is posted, else forget the row.
fn removal(slot: &PublishedSlot, message_id: Option<&str>) -> PlanOp {
    match message_id {
        Some(message_id) => PlanOp::Delete {
            position: slot.position,
            channel_id: slot.channel_id.clone(),
            message_id: message_id.to_owned(),
        },
        None => PlanOp::Forget {
            position: slot.position,
            channel_id: slot.channel_id.clone(),
        },
    }
}

fn first_unknown(published: &[PublishedSlot]) -> Option<Blocked> {
    published
        .iter()
        .filter(|slot| slot.state == SlotState::Unknown)
        .min_by_key(|slot| (slot.position, &slot.channel_id))
        .map(|slot| Blocked::UnknownSlot {
            channel_id: slot.channel_id.clone(),
            position: slot.position,
        })
}

fn as_u32(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

/// The plan turning `published` into `desired` in `channel_id` (4.3).
///
/// - Positional comparison: slots that already match are kept, changed
///   slots with the same files are edited, so a middle insert or removal
///   shifts by edits and appends or deletes at the tail (decision 6).
/// - From the first slot that cannot be edited (`r`, gone slots included)
///   the rest is deleted (newest first) and posted again; that needs
///   `repost_from` when `r` is a middle slot and a posted message goes
///   (decisions 7, 10). A last-slot repost or a gone tail needs none.
/// - Appending after messages that are not the guide's needs `interleaved`
///   (decision 9); slots in another channel make a channel move, which
///   needs `channel_move` and deletes the old messages only with
///   `delete_old` (decision 8), else forgets them.
/// - An unknown slot or a missing channel blocks the plan with no ops.
pub fn plan(
    published: &[PublishedSlot],
    desired: &[RenderedSlot],
    channel_id: &str,
    delete_old: bool,
    inspection: Option<&Inspection>,
) -> Plan {
    let checked = inspection.is_some();
    if let Some(blocked) = first_unknown(published) {
        return Plan::blocked(channel_id, blocked, checked);
    }
    let (grants, gone, newest) = match inspection {
        Some(Inspection::ChannelMissing) => {
            return Plan::blocked(channel_id, Blocked::ChannelMissing, checked);
        }
        Some(Inspection::Channel {
            grants,
            gone,
            newest_message_id,
        }) => (Some(grants), Some(gone), newest_message_id.as_deref()),
        None => (None, None, None),
    };

    let mut current: Vec<Option<Current>> = Vec::new();
    let mut old: Vec<&PublishedSlot> = Vec::new();
    for slot in published {
        if slot.channel_id != channel_id {
            old.push(slot);
            continue;
        }
        let at = slot.position as usize;
        if current.len() <= at {
            current.resize_with(at + 1, || None);
        }
        let live = slot.state == SlotState::Posted
            && !gone.is_some_and(|gone| gone.contains(&slot.position));
        current[at] = Some(Current {
            slot,
            message_id: slot.message_id.as_deref().filter(|_| live),
        });
    }
    let (m, k) = (current.len(), desired.len());
    let n = m.min(k);
    let r = (0..n).find(|&at| {
        current[at]
            .as_ref()
            .is_none_or(|slot| !slot.same(&desired[at]) && !slot.editable(&desired[at]))
    });
    let kept = r.unwrap_or(n);

    let mut ops = Vec::new();
    for (at, slot) in current.iter().enumerate().take(kept) {
        let Some(slot) = slot else { continue };
        if let (false, Some(message_id)) = (slot.same(&desired[at]), slot.message_id) {
            ops.push(PlanOp::Edit {
                position: as_u32(at),
                channel_id: channel_id.to_owned(),
                message_id: message_id.to_owned(),
                hash: desired[at].hash.clone(),
            });
        }
    }
    let mut reposts_posted = false;
    for slot in current[kept..].iter().rev().flatten() {
        reposts_posted |= slot.message_id.is_some();
        ops.push(removal(slot.slot, slot.message_id));
    }
    let created = ops.len();
    for (at, slot) in desired.iter().enumerate().skip(kept) {
        ops.push(PlanOp::Create {
            position: as_u32(at),
            channel_id: channel_id.to_owned(),
            hash: slot.hash.clone(),
        });
    }
    let creates = ops.len() > created;
    old.sort_by(|a, b| (b.position, &b.channel_id).cmp(&(a.position, &a.channel_id)));
    for slot in &old {
        let message_id = slot
            .message_id
            .as_deref()
            .filter(|_| delete_old && slot.state == SlotState::Posted);
        ops.push(removal(slot, message_id));
    }

    // The newest message of ours before the plan runs.
    let ours = current
        .iter()
        .flatten()
        .filter_map(|slot| slot.message_id)
        .reduce(|a, b| if newer(b, a) { b } else { a });
    let needs = Confirmations {
        repost_from: r.filter(|&r| r + 1 < m && reposts_posted).map(as_u32),
        interleaved: creates
            && kept > 0
            && matches!((newest, ours), (Some(newest), Some(ours)) if newer(newest, ours)),
        channel_move: !old.is_empty(),
    };
    let mut plan = Plan::new(channel_id, ops, needs, checked);
    plan.counts.keep = as_u32(
        (0..kept)
            .filter(|&at| {
                current[at]
                    .as_ref()
                    .is_some_and(|slot| slot.same(&desired[at]))
            })
            .count(),
    );
    if let Some(grants) = grants {
        let missing = missing_grants(grants, &plan.ops, desired, channel_id);
        if !missing.is_empty() {
            plan.blocked = Some(Blocked::Grants(missing));
        }
    }
    plan
}

/// The grants the target-channel ops need that the bot lacks: view and
/// history for any op (the check and the ambiguous-create search read the
/// channel), send for creates, embed for embeds, attach for files.
fn missing_grants(
    grants: &GuideGrants,
    ops: &[PlanOp],
    desired: &[RenderedSlot],
    channel_id: &str,
) -> Vec<Grant> {
    let mut needed = BTreeSet::new();
    for op in ops.iter().filter(|op| op.channel_id() == channel_id) {
        needed.extend([Grant::View, Grant::History]);
        let slot = desired.get(op.position() as usize);
        let create = op.kind() == OpKind::Create;
        let writes = create || op.kind() == OpKind::Edit;
        if create {
            needed.insert(Grant::Send);
        }
        if writes && slot.is_some_and(|slot| !slot.embeds.is_empty()) {
            needed.insert(Grant::Embed);
        }
        if create && slot.is_some_and(|slot| !slot.uploads.is_empty()) {
            needed.insert(Grant::Attach);
        }
    }
    needed
        .into_iter()
        .filter(|&grant| !grants.has(grant))
        .collect()
}

/// The plan deleting every published message (newest first in each
/// channel) and forgetting gone slots; an unknown slot blocks it.
pub fn plan_unpublish(published: &[PublishedSlot], channel_id: &str) -> Plan {
    if let Some(blocked) = first_unknown(published) {
        return Plan::blocked(channel_id, blocked, false);
    }
    let mut slots: Vec<&PublishedSlot> = published.iter().collect();
    slots.sort_by(|a, b| (b.position, &b.channel_id).cmp(&(a.position, &a.channel_id)));
    let ops = slots
        .into_iter()
        .map(|slot| {
            let posted = slot.state == SlotState::Posted;
            removal(slot, slot.message_id.as_deref().filter(|_| posted))
        })
        .collect();
    Plan::new(channel_id, ops, Confirmations::default(), false)
}
