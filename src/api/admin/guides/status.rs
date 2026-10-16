//! A guide's list chip, derived on every read from the saved draft (as
//! rendered now), the published slots and the newest run; never stored.
//! The list does not ask Discord: what it knows of the channel is the
//! gateway cache's, and a missing grant shows on the plan.

use serde_json::Value;

use super::{Guides, Refusal, Renderer};
use crate::{
    api::{
        dto::{
            Art,
            guides::{GuideRow, GuideStatus, RunProgress, StatusReason, actor, settled, summary},
            iso_instant,
        },
        listeners::Site,
        state::ApiState,
    },
    domain::{
        guides::{
            Block, BlockError, Guide, Publish, PublishedSlot, Rendered, SlotState, StopReason, plan,
        },
        settings::SettingsChangeQuery,
    },
};

/// What one guide's row is built from.
pub(super) struct Facts {
    pub guide: Guide,
    pub slots: Vec<PublishedSlot>,
    pub latest: Option<Publish>,
    /// The live run's progress, read only while one is live.
    pub progress: Option<RunProgress>,
}

impl Guides {
    /// [`Facts`] of `guide` from the store.
    pub(super) async fn facts(&self, guide: Guide) -> Result<Facts, super::Refusal> {
        let book = self.book();
        let slots = book
            .slots(guide.id.clone())
            .await
            .map_err(super::Refusal::unavailable)?;
        let latest = book
            .latest_publish(guide.id.clone())
            .await
            .map_err(super::Refusal::unavailable)?;
        let progress = match latest.as_ref().filter(|run| run.live()) {
            Some(run) => Some(self.progress(run).await?),
            None => None,
        };
        Ok(Facts {
            guide,
            slots,
            latest,
            progress,
        })
    }

    /// A run's settled effects of those its request planned.
    pub(super) async fn progress(&self, run: &Publish) -> Result<RunProgress, super::Refusal> {
        let ops = self
            .book()
            .publish(run.id.clone())
            .await
            .map_err(super::Refusal::unavailable)?
            .map(|(_, ops)| ops)
            .unwrap_or_default();
        Ok(RunProgress {
            done: settled(&ops),
            total: self.planned_total(run).await?,
        })
    }

    /// The edits, posts and deletes a run's request planned, from its
    /// History record (`publish.<id>`, written with the run at
    /// `requested_at`); the run row keeps no counts.
    pub(super) async fn planned_total(&self, run: &Publish) -> Result<Option<u32>, super::Refusal> {
        let around = chrono::TimeDelta::milliseconds(1);
        let changes = self
            .state
            .store
            .settings_changes(SettingsChangeQuery {
                actor: None,
                from: Some(run.requested_at - around),
                until: Some(run.requested_at + around),
            })
            .await
            .map_err(super::Refusal::unavailable)?;
        let key = format!("publish.{}", run.id);
        Ok(changes
            .iter()
            .find_map(|change| change.values.get(&key))
            .and_then(|row| serde_json::from_str::<Value>(&row.to).ok())
            .and_then(|to| {
                let ops = to.get("ops")?;
                let count = |name: &str| ops.get(name).and_then(Value::as_u64).unwrap_or(0);
                u32::try_from(count("edit") + count("create") + count("delete")).ok()
            }))
    }
}

/// The chip, its reason and, for an unconfirmed post, its position.
pub(super) fn status(
    facts: &Facts,
    rendered: &Result<Rendered, Vec<BlockError>>,
    channel_missing: bool,
) -> (GuideStatus, Option<StatusReason>, Option<u32>) {
    let Facts {
        guide,
        slots,
        latest,
        ..
    } = facts;
    if latest.as_ref().is_some_and(Publish::live) {
        return (GuideStatus::Publishing, None, None);
    }
    if let Some(unknown) = slots.iter().find(|slot| slot.state == SlotState::Unknown) {
        return (
            GuideStatus::NeedsAttention,
            Some(StatusReason::UnconfirmedMessage),
            Some(unknown.position),
        );
    }
    let Ok(rendered) = rendered else {
        return (GuideStatus::Blocked, Some(StatusReason::DraftInvalid), None);
    };
    if channel_missing {
        return (
            GuideStatus::Blocked,
            Some(StatusReason::ChannelMissing),
            None,
        );
    }
    if latest
        .as_ref()
        .is_some_and(|run| run.stop_reason == Some(StopReason::Rejected))
    {
        return (
            GuideStatus::Blocked,
            Some(StatusReason::DiscordRefused),
            None,
        );
    }
    let Some(channel) = guide.channel_id.as_deref() else {
        return if slots.is_empty() {
            (
                GuideStatus::NotPublished,
                Some(StatusReason::NoChannel),
                None,
            )
        } else {
            (
                GuideStatus::OutOfDate,
                Some(StatusReason::ChannelChanged),
                None,
            )
        };
    };
    if slots.is_empty() {
        return (GuideStatus::NotPublished, None, None);
    }
    if plan(slots, &rendered.slots, channel, true, None).no_op() {
        return (GuideStatus::Published, None, None);
    }
    let reason = if slots.iter().any(|slot| slot.channel_id != channel) {
        StatusReason::ChannelChanged
    } else if slots.iter().any(|slot| slot.state == SlotState::Gone) {
        StatusReason::MessageDeleted
    } else if latest
        .as_ref()
        .is_none_or(|run| run.version != guide.version)
    {
        StatusReason::DraftChanged
    } else if art_changed(slots, rendered) {
        StatusReason::ArtChanged
    } else {
        StatusReason::BossFactsChanged
    };
    (GuideStatus::OutOfDate, Some(reason), None)
}

/// A posted message carries the same files as its rendered slot, but one
/// of them now has other bytes.
fn art_changed(slots: &[PublishedSlot], rendered: &Rendered) -> bool {
    slots.iter().any(|slot| {
        usize::try_from(slot.position)
            .ok()
            .and_then(|at| rendered.slots.get(at))
            .is_some_and(|wanted| {
                wanted.uploads.len() == slot.uploads.len()
                    && wanted
                        .uploads
                        .iter()
                        .zip(&slot.uploads)
                        .all(|(upload, stored)| upload.file_name == stored.file_name)
                    && !wanted.same_uploads(&slot.uploads)
            })
    })
}

/// The channel is set but the connected gateway does not list it.
pub(super) fn channel_missing(state: &ApiState, guide: &Guide) -> bool {
    guide.channel_id.as_deref().is_some_and(|channel| {
        state.channels.connected() && !state.channels.channels().iter().any(|c| c.id == channel)
    })
}

/// The list row of one guide.
pub(super) fn row(
    site: &Site,
    state: &ApiState,
    facts: &Facts,
    rendered: &Result<Rendered, Vec<BlockError>>,
) -> GuideRow {
    let guide = &facts.guide;
    let (status, reason, position) = status(facts, rendered, channel_missing(state, guide));
    let channel_name = guide.channel_id.as_deref().and_then(|id| {
        state
            .channels
            .channels()
            .into_iter()
            .find(|channel| channel.id == id)
            .map(|channel| channel.name)
    });
    let messages = facts
        .slots
        .iter()
        .filter(|slot| slot.state == SlotState::Posted)
        .count();
    GuideRow {
        id: guide.id.clone(),
        title: guide.title.clone(),
        channel_id: guide.channel_id.clone(),
        channel_name,
        version: guide.version,
        messages: u32::try_from(messages).unwrap_or(u32::MAX),
        status,
        reason,
        position,
        progress: facts.progress,
        last_run: facts.latest.as_ref().map(summary),
        portrait: portrait(site, state, &guide.blocks),
        updated_at: iso_instant(guide.updated_at),
        updated_by: actor(&guide.updated_by),
    }
}

/// The first boss's portrait URL, when its art exists.
fn portrait(site: &Site, state: &ApiState, blocks: &[Block]) -> Option<String> {
    let key = blocks.iter().find_map(|block| match block {
        Block::Bosses { entries, .. } => entries.first().map(|entry| entry.key.as_str()),
        _ => None,
    })?;
    let boss = state.catalog.boss(key)?;
    let art = Art {
        root: site.boss_dir.as_deref(),
    };
    art.url("portraits", key, boss.portrait().unwrap_or(boss.short()))
}

/// Rows for `guides`, each rendered as it is saved.
pub(super) async fn rows(
    guides: &Guides,
    site: &Site,
    all: Vec<Guide>,
) -> Result<Vec<GuideRow>, Refusal> {
    let mut facts = Vec::with_capacity(all.len());
    for guide in all {
        facts.push(guides.facts(guide).await?);
    }
    let rendered = Renderer::new(site, &guides.state)
        .render_all(
            facts
                .iter()
                .map(|facts| facts.guide.blocks.clone())
                .collect(),
        )
        .await?;
    Ok(facts
        .iter()
        .zip(&rendered)
        .map(|(facts, rendered)| row(site, &guides.state, facts, rendered))
        .collect())
}
