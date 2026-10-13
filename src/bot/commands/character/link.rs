//! `/character link`: the ephemeral notice (version `NOTICE_VERSION`) with
//! Link / Cancel. Link records the consent and runs the first fetch
//! (`CharacterData::confirm_link`, under the limiter, holds and budget) and
//! replaces the notice with what is kept now; Cancel leaves a pending
//! request pending. The buttons stop working after 15 minutes.

use chrono::TimeDelta;

use super::super::dispatch::CommandError;
use super::super::invocation::Invocation;
use super::super::options::Args;
use super::{CharacterCommand, admin_request, reply, resolve};
use crate::bot::chardata::copy;
use crate::bot::delivery::cards::redesign::ButtonId;
use crate::bot::ids::id_text;
use crate::bot::transport::{DiscordTransport, InteractionReply};
use crate::domain::chardata::{LINK_REGION, LinkStatus, NOTICE_VERSION, UnlinkReason};
use crate::domain::history::{Actor, Surface};
use crate::infrastructure::character_data::{ConfirmOutcome, FetchState};

/// How long the notice's buttons work (an interaction token's life).
const NOTICE_LIFE: TimeDelta = TimeDelta::minutes(15);
/// The most a notice may claim to come from the future (clock skew between
/// the notice and the press); later is forged or stale-clocked: refused.
const NOTICE_SKEW: TimeDelta = TimeDelta::seconds(60);

impl<T: DiscordTransport> CharacterCommand<T> {
    pub(super) async fn link(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        if !self.service.connected() {
            return reply(copy::NOT_CONNECTED);
        }
        let typed = Args(&invocation.options).text("character").unwrap_or("");
        let characters = self.mine(invocation).await?;
        let Some(entry) = resolve(&characters, typed) else {
            return reply(copy::not_on_roster(typed));
        };
        let character = &entry.character;
        if character.world != LINK_REGION {
            return reply(copy::OTHER_REGION);
        }
        if !character.is_active() {
            return reply(copy::retired(&character.ign));
        }
        let request = match &entry.link {
            Some(link) if link.link.status == LinkStatus::Active => {
                return reply(copy::already_linked(&character.ign));
            }
            Some(link) if admin_request(&link.link) => Some(self.requester(&link.link).await),
            _ => None,
        };
        let link_id = ButtonId::CharacterLink {
            character_id: character.id.clone(),
            notice: NOTICE_VERSION,
            issued: self.ctx.now().timestamp(),
        }
        .custom_id();
        let cancel_id = ButtonId::CharacterCancel(character.id.clone()).custom_id();
        Ok(InteractionReply::v2(
            copy::notice(
                &character.ign,
                request.as_ref().map(Option::as_deref),
                self.privacy_url.as_deref(),
                link_id,
                cancel_id,
            ),
            true,
        ))
    }

    /// Link pressed: consent, then the first fetch.
    pub(super) async fn link_press(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let id = args.text("character").unwrap_or("");
        let notice = args
            .text("notice")
            .and_then(|text| text.parse::<u32>().ok());
        let issued = args
            .text("issued")
            .and_then(|text| text.parse::<i64>().ok())
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0));
        let (Some(notice), Some(issued)) = (notice, issued) else {
            return reply(copy::EXPIRED);
        };
        let age = self.ctx.now() - issued;
        if age > NOTICE_LIFE || age < -NOTICE_SKEW {
            return reply(copy::EXPIRED);
        }
        let member = id_text(invocation.invoker.user_id);
        let characters = self.mine(invocation).await?;
        let Some(entry) = characters.iter().find(|entry| entry.character.id == id) else {
            return reply(copy::not_on_roster(id));
        };
        let ign = entry.character.ign.as_str();
        let outcome = self
            .service
            .confirm_link(
                &member,
                id,
                notice,
                crate::domain::chardata::ConsentSurface::Discord,
            )
            .await
            .map_err(super::store_failed)?;
        match outcome {
            ConfirmOutcome::Linked(_, FetchState::Stored) => {
                match self.service.data(id).await.map_err(super::store_failed)? {
                    Some(snapshot) => Ok(InteractionReply::v2(copy::linked(ign, &snapshot), true)),
                    None => reply(copy::linked_later(ign)),
                }
            }
            ConfirmOutcome::Linked(_, FetchState::Failed { .. }) => reply(copy::linked_later(ign)),
            // NEXON has no such character: nothing is kept, not even the link.
            ConfirmOutcome::Linked(_, FetchState::Stale) => {
                self.service
                    .unlink(
                        id,
                        &Actor::member(member),
                        Surface::Discord,
                        UnlinkReason::NotFound,
                        None,
                    )
                    .await
                    .map_err(super::store_failed)?;
                reply(copy::not_found(ign))
            }
            ConfirmOutcome::Linked(_, FetchState::Unlinked) => Err(CommandError::Internal(
                "the link changed during its first fetch".into(),
            )),
            ConfirmOutcome::NotConnected => reply(copy::NOT_CONNECTED),
            ConfirmOutcome::NoticeOutdated | ConfirmOutcome::Conflict => reply(copy::EXPIRED),
            ConfirmOutcome::NotEligible => reply(copy::not_on_roster(ign)),
            ConfirmOutcome::AlreadyLinked => reply(copy::already_linked(ign)),
            ConfirmOutcome::Busy => reply(copy::BUSY),
            ConfirmOutcome::Unavailable(_) => reply(copy::UNAVAILABLE),
        }
    }

    /// Cancel pressed: nothing changes.
    pub(super) async fn cancel_press(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let id = Args(&invocation.options).text("character").unwrap_or("");
        let characters = self.mine(invocation).await?;
        let pending = characters
            .iter()
            .find(|entry| entry.character.id == id)
            .and_then(|entry| {
                let link = entry.link.as_ref()?;
                admin_request(&link.link).then_some((entry.character.ign.as_str(), &link.link))
            });
        match pending {
            Some((ign, link)) => {
                let name = self.requester(link).await;
                reply(copy::cancelled(ign, true, name.as_deref()))
            }
            None => reply(copy::NOTHING_LINKED),
        }
    }
}
