//! `guides.json`: the guide publisher's admin shapes (module `guides`). A
//! guide is ordered blocks rendered into Discord messages ("slots"); its
//! status is derived on every read from the draft, the published slots and
//! the newest run, never stored. Message ids are sent whole (the desktop's
//! last-four display is the app's).

use serde::{Deserialize, Serialize};

use super::{history::SettingsActor, iso_instant};
use kanade_domain::guides::{
    Block, BlockError, BossEntry, Confirmations, GuideEmbed, OpKind, OpOutcome, Plan, PlanCounts,
    PlanOp, Publish, PublishOp, PublishState, PublishedSlot, RenderWarning, RenderedSlot,
    StopReason, units,
};

/// One block as the editor sends and reads it (`kind` tags it). Ids are
/// hyphenated UUIDs the editor keeps across reorders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum GuideBlock {
    /// Discord Markdown: one message, 1 to 2,000 UTF-16 units.
    Text { id: String, content: String },
    /// One embed per boss; `heading` is the first message's content.
    Bosses {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        heading: Option<String>,
        entries: Vec<GuideBossEntry>,
    },
    /// A visual gap: one message holding a zero-width space.
    Spacer { id: String },
}

/// One boss of a bosses block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GuideBossEntry {
    /// The catalog key, exact case (`MaleficStar`).
    pub key: String,
    /// Up to 1,000 units, added under the embed's description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub note: Option<String>,
    /// Also show the entry artwork as the large image (the portrait
    /// thumbnail is always shown when the art exists); `false` when absent.
    #[serde(default)]
    pub large_image: bool,
}

impl From<GuideBlock> for Block {
    fn from(block: GuideBlock) -> Self {
        match block {
            GuideBlock::Text { id, content } => Self::Text { id, content },
            GuideBlock::Bosses {
                id,
                heading,
                entries,
            } => Self::Bosses {
                id,
                heading,
                entries: entries
                    .into_iter()
                    .map(|entry| BossEntry {
                        key: entry.key,
                        note: entry.note,
                        large_image: entry.large_image,
                    })
                    .collect(),
            },
            GuideBlock::Spacer { id } => Self::Spacer { id },
        }
    }
}

pub fn block(block: &Block) -> GuideBlock {
    match block {
        Block::Text { id, content } => GuideBlock::Text {
            id: id.clone(),
            content: content.clone(),
        },
        Block::Bosses {
            id,
            heading,
            entries,
        } => GuideBlock::Bosses {
            id: id.clone(),
            heading: heading.clone(),
            entries: entries
                .iter()
                .map(|entry| GuideBossEntry {
                    key: entry.key.clone(),
                    note: entry.note.clone(),
                    large_image: entry.large_image,
                })
                .collect(),
        },
        Block::Spacer { id } => GuideBlock::Spacer { id: id.clone() },
    }
}

/// The list chip (approved boards): `publishing` (a run is queued or
/// running), `needs_attention` (a post could not be confirmed),
/// `blocked`, `not_published` (no message in Discord), `out_of_date`,
/// `published`, in that order of precedence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum GuideStatus {
    NotPublished,
    Published,
    OutOfDate,
    Publishing,
    NeedsAttention,
    Blocked,
}

/// Why a guide is out of date, needs attention, is blocked or is not
/// published yet; the most telling one when several hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum StatusReason {
    /// Not published: no channel chosen yet.
    NoChannel,
    /// Saved since the last publish.
    DraftChanged,
    /// The guide now targets another channel.
    ChannelChanged,
    /// A boss picture changed.
    ArtChanged,
    /// The catalog or a knowledge lead changed what an embed says.
    BossFactsChanged,
    /// A message was deleted by hand.
    MessageDeleted,
    /// Needs attention: the post at `position` could not be confirmed.
    UnconfirmedMessage,
    /// Blocked: the channel is gone or the bot cannot see it.
    ChannelMissing,
    /// Blocked: the last run stopped because Discord refused (permissions).
    DiscordRefused,
    /// Blocked: the saved draft no longer renders (e.g. a boss left the
    /// catalog); the plan names the problems.
    DraftInvalid,
}

/// A live run's progress: Discord effects settled of those planned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunProgress {
    pub done: u32,
    /// Edits, posts and deletes the request planned; `null` when its
    /// History record cannot be read.
    pub total: Option<u32>,
}

/// The newest run, for the list's time and a stopped run's reason.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunSummary {
    pub id: String,
    #[cfg_attr(test, ts(type = "'publish' | 'unpublish' | 'search'"))]
    pub kind: &'static str,
    pub state: GuideRunState,
    pub stop_reason: Option<GuideStopReason>,
    pub requested_by: SettingsActor,
    pub requested_at: String,
    pub finished_at: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideRow {
    pub id: String,
    pub title: String,
    pub channel_id: Option<String>,
    /// `#name` while the bot lists the channel.
    pub channel_name: Option<String>,
    /// Send it back with a save, delete or publish.
    pub version: u32,
    /// Messages the guide has in Discord now (posted slots).
    pub messages: u32,
    pub status: GuideStatus,
    pub reason: Option<StatusReason>,
    /// `unconfirmed_message`'s slot (0-based; the app shows message n + 1).
    pub position: Option<u32>,
    /// While `publishing`.
    pub progress: Option<RunProgress>,
    pub last_run: Option<RunSummary>,
    /// The first boss's portrait (`/art/portraits/{key}`), if any.
    pub portrait: Option<String>,
    pub updated_at: String,
    pub updated_by: SettingsActor,
}

/// `GET /api/admin/guides`: by title.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideList {
    pub guides: Vec<GuideRow>,
}

/// One published slot.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideSlotRow {
    pub channel_id: String,
    pub position: u32,
    /// Whole; `null` while unknown.
    pub message_id: Option<String>,
    #[cfg_attr(test, ts(type = "'posted' | 'gone' | 'unknown'"))]
    pub state: &'static str,
    pub updated_at: String,
}

/// `GET /api/admin/guides/{id}`.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideDetail {
    pub guide: GuideRow,
    pub blocks: Vec<GuideBlock>,
    pub created_at: String,
    pub created_by: SettingsActor,
    /// By channel, then position.
    pub slots: Vec<GuideSlotRow>,
}

/// A create's or save's answer: what to send back next.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideSaved {
    pub id: String,
    pub version: u32,
    pub updated_at: String,
}

/// One boss embed as Discord shows it; pictures are same-origin art URLs.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PreviewEmbed {
    pub title: String,
    pub description: String,
    /// `#rrggbb`.
    pub color: String,
    /// `/art/portraits/{key}`.
    pub thumbnail: Option<String>,
    /// `/art/entry/{key}`.
    pub image: Option<String>,
}

/// One message the guide renders, in order.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PreviewSlot {
    pub position: u32,
    pub block_id: String,
    /// The message's index within its block.
    pub part: u32,
    pub content: String,
    /// UTF-16 units of `content` (Discord's count).
    pub content_units: u32,
    pub embeds: Vec<PreviewEmbed>,
    /// Title and description units of every embed (at most 6,000).
    pub embed_units: u32,
    /// The files the message uploads.
    pub files: Vec<String>,
    pub hash: String,
}

/// Something the preview shows that never fails a render.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideWarning {
    /// `missing_art`: the embed goes without that picture.
    #[cfg_attr(test, ts(type = "'missing_art'"))]
    pub kind: &'static str,
    pub block_id: String,
    /// The entry's index in its block.
    pub entry: u32,
    pub key: String,
    #[cfg_attr(test, ts(type = "'portrait' | 'entry'"))]
    pub art: &'static str,
}

/// `POST /api/admin/guides/{id}/render`.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuidePreview {
    pub slots: Vec<PreviewSlot>,
    pub warnings: Vec<GuideWarning>,
}

/// One reason blocks are refused, naming the block and field.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideProblem {
    #[cfg_attr(
        test,
        ts(
            type = "'too_many_blocks' | 'too_large' | 'bad_id' | 'duplicate_id' | 'empty' \
                   | 'too_long' | 'unknown_boss' | 'too_many_slots'"
        )
    )]
    pub problem: &'static str,
    pub block_id: Option<String>,
    /// `content`, `heading`, `entries`, `entries[i].key|note|title|description`.
    pub field: Option<String>,
    /// The value's length or count, and its limit.
    pub units: Option<u32>,
    pub limit: Option<u32>,
    /// `unknown_boss`'s key.
    pub key: Option<String>,
    pub message: String,
}

/// `422 invalid_blocks`: an `ApiError` plus every problem, in block order.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideBlocksRefused {
    #[cfg_attr(test, ts(type = "'invalid_blocks' | 'draft_invalid'"))]
    pub error: &'static str,
    pub message: String,
    pub problems: Vec<GuideProblem>,
}

/// What a plan needs confirmed, or what a publish confirms.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GuideConfirmations {
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

impl From<&Confirmations> for GuideConfirmations {
    fn from(needs: &Confirmations) -> Self {
        Self {
            repost_from: needs.repost_from,
            interleaved: needs.interleaved,
            channel_move: needs.channel_move,
        }
    }
}

impl From<GuideConfirmations> for Confirmations {
    fn from(confirm: GuideConfirmations) -> Self {
        Self {
            repost_from: confirm.repost_from,
            interleaved: confirm.interleaved,
            channel_move: confirm.channel_move,
        }
    }
}

/// `422 needs_confirmation`: an `ApiError` plus what the plan needs.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideNeedsConfirmation {
    #[cfg_attr(test, ts(type = "'needs_confirmation'"))]
    pub error: &'static str,
    pub message: String,
    pub needs: GuideConfirmations,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuidePlanCounts {
    pub keep: u32,
    pub edit: u32,
    pub create: u32,
    /// Discord deletes only.
    pub delete: u32,
}

/// One op in run order. `forget` drops a slot without a Discord call.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuidePlanOp {
    pub kind: GuideOpKind,
    pub position: u32,
    pub channel_id: String,
    /// Whole, for edits and deletes.
    pub message_id: Option<String>,
    /// The rendered message's block kind, for edits and creates.
    #[cfg_attr(test, ts(type = "'text' | 'bosses' | 'spacer' | null"))]
    pub block_kind: Option<&'static str>,
    /// Its first line of text or first embed title.
    pub summary: Option<String>,
}

/// Why a plan cannot run.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuidePlanBlocked {
    /// `unknown_slot`: settle it first (no ops); `channel_missing` (no
    /// ops); `missing_grants`: the ops are shown but must not run.
    #[cfg_attr(
        test,
        ts(type = "'unknown_slot' | 'channel_missing' | 'missing_grants'")
    )]
    pub reason: &'static str,
    pub channel_id: Option<String>,
    pub position: Option<u32>,
    #[cfg_attr(
        test,
        ts(type = "Array<'view' | 'send' | 'history' | 'embed' | 'attach'>")
    )]
    pub grants: Vec<&'static str>,
}

/// `GET /api/admin/guides/{id}/plan`: the saved version rendered and the
/// plan to publish (or unpublish) it, checked against the channel when
/// Discord answered in time.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuidePlan {
    #[cfg_attr(test, ts(type = "'publish' | 'unpublish'"))]
    pub kind: &'static str,
    pub version: u32,
    /// The target channel; empty only for an unpublish of a guide with no
    /// channel and no messages.
    pub channel_id: String,
    pub delete_old: bool,
    /// `false`: "Unchecked" (Discord was slow or offline); the worker
    /// checks again before acting.
    pub checked: bool,
    pub no_op: bool,
    /// Send it back with the publish.
    pub plan_hash: String,
    pub counts: GuidePlanCounts,
    pub needs: GuideConfirmations,
    pub blocked: Option<GuidePlanBlocked>,
    pub ops: Vec<GuidePlanOp>,
    /// The rendered messages (empty for an unpublish).
    pub slots: Vec<PreviewSlot>,
    pub warnings: Vec<GuideWarning>,
}

/// One op of a run's log.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideRunOp {
    pub ordinal: u32,
    pub kind: GuideOpKind,
    pub position: u32,
    pub channel_id: String,
    pub message_id: Option<String>,
    #[cfg_attr(
        test,
        ts(
            type = "'pending' | 'done' | 'gone' | 'recovered' | 'unknown' | 'rejected' | 'skipped'"
        )
    )]
    pub outcome: &'static str,
    /// An unconfirmed post's search matches (zero or several).
    pub candidates: Vec<String>,
    pub at: String,
}

/// `GET /api/admin/guides/{id}/publishes/{run}`: the progress screen.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuideRun {
    pub id: String,
    pub guide_id: String,
    #[cfg_attr(test, ts(type = "'publish' | 'unpublish' | 'search'"))]
    pub kind: &'static str,
    pub state: GuideRunState,
    pub stop_reason: Option<GuideStopReason>,
    pub version: u32,
    pub channel_id: String,
    pub plan_hash: String,
    pub confirmed: GuideConfirmations,
    pub delete_old: bool,
    pub requested_by: SettingsActor,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub cancel_requested_at: Option<String>,
    pub progress: RunProgress,
    /// By ordinal; each is recorded before its effect is tried.
    pub ops: Vec<GuideRunOp>,
}

/// A publish, unpublish or search request's answer: `202` with the queued
/// run, or `200` with `no_op` when the channel already shows the guide.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct GuidePublishResult {
    pub no_op: bool,
    pub publish: Option<GuideRun>,
}

/// A run's lifecycle; `done`, `stopped` and `cancelled` are final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum GuideRunState {
    Queued,
    Running,
    Done,
    Stopped,
    Cancelled,
}

impl From<PublishState> for GuideRunState {
    fn from(state: PublishState) -> Self {
        match state {
            PublishState::Queued => Self::Queued,
            PublishState::Running => Self::Running,
            PublishState::Done => Self::Done,
            PublishState::Stopped => Self::Stopped,
            PublishState::Cancelled => Self::Cancelled,
        }
    }
}

/// Why a run stopped before its last op.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum GuideStopReason {
    /// The channel changed since the preview, or an edit found its message
    /// gone; nothing more was sent.
    PlanChanged,
    /// A post could not be confirmed or found.
    UnknownSlot,
    EditUnconfirmed,
    /// A delete could not be confirmed; no post of the run followed.
    DeleteUnconfirmed,
    /// Discord refused an op (e.g. missing permissions).
    Rejected,
    DiscordUnavailable,
    /// The bot restarted; runs never resume on their own.
    Interrupted,
}

impl From<StopReason> for GuideStopReason {
    fn from(reason: StopReason) -> Self {
        match reason {
            StopReason::PlanChanged => Self::PlanChanged,
            StopReason::UnknownSlot => Self::UnknownSlot,
            StopReason::EditUnconfirmed => Self::EditUnconfirmed,
            StopReason::DeleteUnconfirmed => Self::DeleteUnconfirmed,
            StopReason::Rejected => Self::Rejected,
            StopReason::DiscordUnavailable => Self::DiscordUnavailable,
            StopReason::Interrupted => Self::Interrupted,
        }
    }
}

/// A plan op's kind; `forget` drops a slot without a Discord call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum GuideOpKind {
    Edit,
    Create,
    Delete,
    Forget,
}

impl From<OpKind> for GuideOpKind {
    fn from(kind: OpKind) -> Self {
        match kind {
            OpKind::Edit => Self::Edit,
            OpKind::Create => Self::Create,
            OpKind::Delete => Self::Delete,
            OpKind::Forget => Self::Forget,
        }
    }
}

/// `kind:id` as stored, as History names actors.
pub fn actor(text: &str) -> SettingsActor {
    let (kind, id) = text.split_once(':').unwrap_or(("system", text));
    let kind = match kind {
        "admin" => "admin",
        "member" => "member",
        _ => "system",
    };
    SettingsActor {
        kind,
        id: id.to_owned(),
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub fn summary(run: &Publish) -> RunSummary {
    RunSummary {
        id: run.id.clone(),
        kind: run.kind.as_str(),
        state: run.state.into(),
        stop_reason: run.stop_reason.map(Into::into),
        requested_by: actor(&run.requested_by),
        requested_at: iso_instant(run.requested_at),
        finished_at: run.finished_at.map(iso_instant),
    }
}

/// Discord effects settled so far: forgets make no call and pending ops are
/// still to come.
pub fn settled(ops: &[PublishOp]) -> u32 {
    count(
        ops.iter()
            .filter(|op| {
                !matches!(op.op, PlanOp::Forget { .. }) && op.outcome != OpOutcome::Pending
            })
            .count(),
    )
}

pub fn run(run: &Publish, ops: &[PublishOp], total: Option<u32>) -> GuideRun {
    GuideRun {
        id: run.id.clone(),
        guide_id: run.guide_id.clone(),
        kind: run.kind.as_str(),
        state: run.state.into(),
        stop_reason: run.stop_reason.map(Into::into),
        version: run.version,
        channel_id: run.channel_id.clone(),
        plan_hash: run.plan_hash.clone(),
        confirmed: (&run.confirmed).into(),
        delete_old: run.delete_old,
        requested_by: actor(&run.requested_by),
        requested_at: iso_instant(run.requested_at),
        started_at: run.started_at.map(iso_instant),
        finished_at: run.finished_at.map(iso_instant),
        cancel_requested_at: run.cancel_requested_at.map(iso_instant),
        progress: RunProgress {
            done: settled(ops),
            total,
        },
        ops: ops
            .iter()
            .map(|op| GuideRunOp {
                ordinal: op.ordinal,
                kind: op.op.kind().into(),
                position: op.op.position(),
                channel_id: op.op.channel_id().to_owned(),
                message_id: op.op.message_id().map(str::to_owned),
                outcome: op.outcome.as_str(),
                candidates: op.candidates.clone(),
                at: iso_instant(op.at),
            })
            .collect(),
    }
}

pub fn slot_row(slot: &PublishedSlot) -> GuideSlotRow {
    GuideSlotRow {
        channel_id: slot.channel_id.clone(),
        position: slot.position,
        message_id: slot.message_id.clone(),
        state: slot.state.as_str(),
        updated_at: iso_instant(slot.updated_at),
    }
}

/// `/art/{kind}/{key}` for an upload named `<key>-<portrait|entry>.<ext>`.
fn art_url(file_name: &str) -> Option<String> {
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    let (key, kind) = stem.rsplit_once('-')?;
    let kind = match kind {
        "portrait" => "portraits",
        "entry" => "entry",
        _ => return None,
    };
    Some(format!("/art/{kind}/{key}"))
}

fn embed(embed: &GuideEmbed) -> PreviewEmbed {
    PreviewEmbed {
        title: embed.title.clone(),
        description: embed.description.clone(),
        color: format!("#{:06x}", embed.colour & 0x00ff_ffff),
        thumbnail: embed.thumbnail.as_deref().and_then(art_url),
        image: embed.image.as_deref().and_then(art_url),
    }
}

pub fn preview_slots(slots: &[RenderedSlot]) -> Vec<PreviewSlot> {
    slots
        .iter()
        .enumerate()
        .map(|(position, slot)| PreviewSlot {
            position: count(position),
            block_id: slot.block_id.clone(),
            part: slot.part,
            content: slot.content.clone(),
            content_units: count(units(&slot.content)),
            embeds: slot.embeds.iter().map(embed).collect(),
            embed_units: count(slot.embeds.iter().map(GuideEmbed::units).sum()),
            files: slot
                .uploads
                .iter()
                .map(|upload| upload.file_name.clone())
                .collect(),
            hash: slot.hash.clone(),
        })
        .collect()
}

pub fn warnings(warnings: &[RenderWarning]) -> Vec<GuideWarning> {
    warnings
        .iter()
        .map(|warning| match warning {
            RenderWarning::MissingArt {
                block_id,
                entry,
                key,
                kind,
            } => GuideWarning {
                kind: "missing_art",
                block_id: block_id.clone(),
                entry: count(*entry),
                key: key.clone(),
                art: kind.as_str(),
            },
        })
        .collect()
}

pub fn problems(errors: &[BlockError]) -> Vec<GuideProblem> {
    errors
        .iter()
        .map(|error| {
            let message = error.to_string();
            let problem = |problem| GuideProblem {
                problem,
                block_id: None,
                field: None,
                units: None,
                limit: None,
                key: None,
                message: message.clone(),
            };
            match error {
                BlockError::TooManyBlocks { count: n, limit } => GuideProblem {
                    units: Some(count(*n)),
                    limit: Some(count(*limit)),
                    ..problem("too_many_blocks")
                },
                BlockError::TooLarge { bytes, limit } => GuideProblem {
                    units: Some(count(*bytes)),
                    limit: Some(count(*limit)),
                    ..problem("too_large")
                },
                BlockError::BadId { block_id } => GuideProblem {
                    block_id: Some(block_id.clone()),
                    ..problem("bad_id")
                },
                BlockError::DuplicateId { block_id } => GuideProblem {
                    block_id: Some(block_id.clone()),
                    ..problem("duplicate_id")
                },
                BlockError::Empty { block_id, field } => GuideProblem {
                    block_id: Some(block_id.clone()),
                    field: Some(field.clone()),
                    ..problem("empty")
                },
                BlockError::TooLong {
                    block_id,
                    field,
                    units,
                    limit,
                } => GuideProblem {
                    block_id: Some(block_id.clone()),
                    field: Some(field.clone()),
                    units: Some(count(*units)),
                    limit: Some(count(*limit)),
                    ..problem("too_long")
                },
                BlockError::UnknownBoss {
                    block_id,
                    field,
                    key,
                } => GuideProblem {
                    block_id: Some(block_id.clone()),
                    field: Some(field.clone()),
                    key: Some(key.clone()),
                    ..problem("unknown_boss")
                },
                BlockError::TooManySlots { count: n, limit } => GuideProblem {
                    units: Some(count(*n)),
                    limit: Some(count(*limit)),
                    ..problem("too_many_slots")
                },
            }
        })
        .collect()
}

pub fn counts(counts: &PlanCounts) -> GuidePlanCounts {
    GuidePlanCounts {
        keep: counts.keep,
        edit: counts.edit,
        create: counts.create,
        delete: counts.delete,
    }
}

/// The plan's ops, each edit and create labelled from the rendered slot it
/// writes (`desired`, by position).
pub fn plan_ops(plan: &Plan, desired: &[RenderedSlot], blocks: &[Block]) -> Vec<GuidePlanOp> {
    plan.ops
        .iter()
        .map(|op| {
            let rendered = op
                .hash()
                .and_then(|_| desired.get(usize::try_from(op.position()).ok()?));
            GuidePlanOp {
                kind: op.kind().into(),
                position: op.position(),
                channel_id: op.channel_id().to_owned(),
                message_id: op.message_id().map(str::to_owned),
                block_kind: rendered.and_then(|slot| {
                    blocks
                        .iter()
                        .find(|block| block.id() == slot.block_id)
                        .map(|block| match block {
                            Block::Text { .. } => "text",
                            Block::Bosses { .. } => "bosses",
                            Block::Spacer { .. } => "spacer",
                        })
                }),
                summary: rendered.and_then(slot_summary),
            }
        })
        .collect()
}

/// A message's first line of text, else its first embed title.
fn slot_summary(slot: &RenderedSlot) -> Option<String> {
    slot.content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != kanade_domain::guides::SPACER)
        .map(str::to_owned)
        .or_else(|| slot.embeds.first().map(|embed| embed.title.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_urls_come_from_upload_names() {
        assert_eq!(
            art_url("MaleficStar-portrait.png").as_deref(),
            Some("/art/portraits/MaleficStar")
        );
        assert_eq!(
            art_url("Kalos-entry.webp").as_deref(),
            Some("/art/entry/Kalos")
        );
        assert_eq!(art_url("Kalos.webp"), None);
    }

    #[test]
    fn stored_actors_split_on_the_first_colon() {
        let admin = actor("admin:token");
        assert_eq!((admin.kind, admin.id.as_str()), ("admin", "token"));
        let system = actor("system:guides");
        assert_eq!((system.kind, system.id.as_str()), ("system", "guides"));
    }
}
