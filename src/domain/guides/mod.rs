//! Guide publisher (module `guides`, behind `modules.guides`): admins write
//! boss guides as ordered blocks, preview them as Discord shows them and
//! publish them to one channel as an ordered set of messages ("slots"),
//! later publishes editing in place. This module is the pure part: blocks,
//! rendering within Discord's limits, content hashes, slot planning and the
//! store port. Guide content never reaches a model: chat and extraction
//! never import it (pinned below).

mod blocks;
mod hash;
mod plan;
mod port;
mod render;

use std::time::Duration;

pub use blocks::{
    Block, BlockError, BossEntry, CONTENT_LIMIT, MAX_BLOCKS, MAX_BLOCKS_BYTES, NOTE_LIMIT, units,
    validate,
};
pub use hash::{plan_hash, sha256_hex, slot_hash};
pub use plan::{
    Blocked, Confirmations, Grant, GuideGrants, Inspection, OpKind, Plan, PlanCounts, PlanOp,
    PublishedSlot, SlotState, plan, plan_unpublish,
};
pub use port::{
    CancelRequest, GUIDES_SECTION, Guide, GuideChange, GuideRecord, GuideStore, GuideWrite,
    NotPosted, OpOutcome, Publish, PublishKind, PublishOp, PublishRequest, PublishRequested,
    PublishState, SlotEffect, StopReason, StoreFuture,
};
pub use render::{
    ArtFile, ArtKind, DEFAULT_COLOUR, DESCRIPTION_LIMIT, EMBED_TOTAL_LIMIT, GuideEmbed,
    GuideSources, MAX_EMBEDS, MAX_SLOTS, MAX_UPLOAD_BYTES, MAX_UPLOADS, RenderWarning, Rendered,
    RenderedSlot, SPACER, TITLE_LIMIT, Upload, UploadRef, render,
};

/// How long the plan preview waits for the channel check (4.4) before
/// showing the plan "Unchecked".
pub const INSPECT_BUDGET: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests;
