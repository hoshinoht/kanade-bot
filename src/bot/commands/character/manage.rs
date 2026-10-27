//! `/character unlink` (no confirm: deletes the NEXON data at once, the
//! roster keeps its copied class and level, and queues the shared cards for
//! stripping), `/character data` (everything kept, with dates) and
//! `/character share` (a public card the bot posts itself, its `posts` row
//! written first).

use super::super::dispatch::CommandError;
use super::super::invocation::Invocation;
use super::super::options::Args;
use super::{CharacterCommand, reply, resolve, store_failed};
use crate::app::ids::id_text;
use crate::discord::transport::{DiscordTransport, InteractionReply, RejectionKind};
use crate::{bot::chardata::Sent, cards::chardata};
use kanade_chardata::character_data::Tracked;
use kanade_domain::chardata::{LinkStatus, UnlinkReason};
use kanade_domain::history::{Actor, Surface};

impl<T: DiscordTransport> CharacterCommand<T> {
    pub(super) async fn unlink(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let typed = Args(&invocation.options).text("character").unwrap_or("");
        let actor = Actor::member(id_text(invocation.invoker.user_id));
        let characters = self.mine(invocation).await?;
        let unlink = |id: String| {
            let actor = actor.clone();
            async move {
                self.service
                    .unlink(&id, &actor, Surface::Discord, UnlinkReason::Requested, None)
                    .await
                    .map_err(store_failed)
            }
        };
        if typed.trim().eq_ignore_ascii_case(chardata::ALL) {
            let mut names = Vec::new();
            for entry in characters.iter().filter(|entry| entry.link.is_some()) {
                if unlink(entry.character.id.clone()).await?.is_some() {
                    names.push(entry.character.ign.as_str());
                }
            }
            if names.is_empty() {
                return reply(chardata::NOTHING_KEPT);
            }
            return reply(chardata::unlinked_all(&names));
        }
        let Some(entry) = resolve(&characters, typed) else {
            return reply(chardata::not_on_roster(typed));
        };
        let ign = entry.character.ign.as_str();
        let Some(link) = &entry.link else {
            return reply(chardata::not_linked(ign));
        };
        if unlink(entry.character.id.clone()).await?.is_none() {
            return reply(chardata::not_linked(ign));
        }
        let cards: Vec<&str> = link
            .posts
            .iter()
            .map(|post| post.channel_id.as_str())
            .collect();
        reply(chardata::unlinked(ign, &cards))
    }

    pub(super) async fn data(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let characters = self.mine(invocation).await?;
        if characters.iter().all(|entry| entry.link.is_none()) {
            return reply(chardata::NOTHING_KEPT);
        }
        let entries: Vec<chardata::DataEntry<'_>> = characters
            .iter()
            .filter_map(|entry| {
                let ign = entry.character.ign.as_str();
                Some(match &entry.link {
                    Some(link) => match link.link.status {
                        LinkStatus::Active => chardata::DataEntry::Linked {
                            ign,
                            snapshot: link.snapshot.as_ref(),
                            linked_at: link.link.linked_at,
                            // An admin's link carries no consent of the member's.
                            notice: link
                                .link
                                .consent_version
                                .filter(|_| !link.link.admin_linked()),
                            cards: link
                                .posts
                                .iter()
                                .map(|post| post.channel_id.as_str())
                                .collect(),
                            retired: !entry.character.is_active(),
                        },
                        LinkStatus::Pending => chardata::DataEntry::Pending {
                            ign,
                            requested_at: link.link.requested_at,
                        },
                        LinkStatus::Stale => chardata::DataEntry::Stale { ign },
                    },
                    None if entry.linkable() => chardata::DataEntry::NotLinked { ign },
                    None => return None,
                })
            })
            .collect();
        Ok(InteractionReply::v2(
            chardata::data(&entries, self.privacy_url.as_deref()),
            true,
        ))
    }

    pub(super) async fn share(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let Some(channel) = invocation.channel_id.map(id_text) else {
            return Err(CommandError::Internal("share without a channel".into()));
        };
        let typed = Args(&invocation.options).text("character").unwrap_or("");
        let characters = self.mine(invocation).await?;
        let Some(entry) = resolve(&characters, typed) else {
            return reply(chardata::not_on_roster(typed));
        };
        let ign = entry.character.ign.as_str();
        let linked = entry
            .link
            .as_ref()
            .is_some_and(|link| link.link.status == LinkStatus::Active);
        if !linked || !entry.linkable() {
            return reply(chardata::share_not_linked(ign));
        }
        if !self.ctx.channels.can_send(&channel) {
            return reply(chardata::cant_post(&channel));
        }
        let (post, snapshot) = match self
            .service
            .track_post(&entry.character.id, &channel)
            .await
            .map_err(store_failed)?
        {
            Tracked::Ready(post, snapshot) => (post, snapshot),
            Tracked::NoData => return reply(chardata::share_no_data(ign)),
        };
        let owner = invocation.invoker_name.as_deref().unwrap_or("A member");
        let card = chardata::share_card(owner, ign, &snapshot, &post.reference);
        match self.posts.send(&channel, card, &post.reference).await {
            Sent::Posted(message_id) => {
                // Unrecorded, the strip finds it again by its ref.
                let _ = self.service.post_sent(&post.reference, &message_id).await;
                reply(chardata::posted(&channel))
            }
            Sent::Refused(kind) => {
                self.service
                    .post_refused(&post.reference)
                    .await
                    .map_err(store_failed)?;
                match kind {
                    RejectionKind::MissingAccess
                    | RejectionKind::MissingPermissions
                    | RejectionKind::UnknownChannel => reply(chardata::cant_post(&channel)),
                    other => Err(CommandError::Internal(format!(
                        "share card refused: {other:?}"
                    ))),
                }
            }
            Sent::Unknown => {
                self.service
                    .post_unknown(&post)
                    .await
                    .map_err(store_failed)?;
                reply(chardata::unconfirmed(&channel))
            }
        }
    }
}
