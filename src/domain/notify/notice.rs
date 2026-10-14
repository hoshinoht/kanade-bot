//! Schedule change notices as delivery intents (v4 `api.service._announce`).

use super::audience::resolve_mentions;
use super::dispatch::DeliverySettings;
use super::intent::{
    ChannelChoice, ChannelDirectory, DeliveryWarning, EffectKind, IntentContent,
    NotificationIntent, canonical_allow_list, choose_channel,
};
use super::policy::{PingKind, allowed_mentions};
use crate::domain::members::Directory;
use crate::domain::schedule::Notice;

/// Plan one change notice: its home channel, else the post channel; `None`
/// when neither is reachable (the change itself already happened). A fallback
/// carries [`DeliveryWarning::HomeChannelUnavailable`] exactly as reminder
/// dispatch does: whenever the post channel stands in, including for an unset
/// home channel; `run_ids` names the notice's run (empty for a weekly-timing
/// change). Only the
/// listed members the notice's kind may notify are mentioned; every schedule
/// notice kind is informational, so that means members on `all`.
///
/// Notices are operation-scoped: they bind no native row, so the journal
/// dedupes them by operation and `effect_context`, not by target.
pub fn plan_notice(
    notice: &Notice,
    members: &dyn Directory,
    channels: &dyn ChannelDirectory,
    settings: DeliverySettings<'_>,
) -> Option<NotificationIntent> {
    let (channel_id, warnings) = match choose_channel(
        notice.channel_id.as_deref(),
        settings.post_channel_id,
        channels,
    ) {
        ChannelChoice::Requested(channel_id) => (channel_id, Vec::new()),
        ChannelChoice::Fallback {
            channel_id,
            requested,
        } => (
            channel_id,
            vec![DeliveryWarning::HomeChannelUnavailable {
                home_channel_id: requested,
                run_ids: notice
                    .change
                    .run_id()
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
            }],
        ),
        ChannelChoice::Unavailable => return None,
    };
    let kind = PingKind::parse(notice.ping_kind());
    let resolved = resolve_mentions(members, &notice.listed, &kind);
    Some(NotificationIntent {
        effect: EffectKind::Notice(notice.effect_kind()),
        effect_context: notice.effect_context(),
        channel_id,
        targets: Vec::new(),
        mentions: canonical_allow_list(
            allowed_mentions(&resolved, None, settings.quiet_mode).users(),
        ),
        content: IntentContent::Notice(notice.clone()),
        warnings,
    })
}
