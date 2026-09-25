//! Test doubles: a Discord that issues single-use codes and checks PKCE, and
//! a guild whose members the staff gate reads.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

use twilight_model::id::{Id, marker::UserMarker};

use super::{
    crypto,
    discord::{
        AccessToken, CodeExchange, DiscordApi, DiscordClient, DiscordError, DiscordFuture,
        DiscordUser,
    },
    staff::{GateFuture, GuildMembers},
};
use crate::bot::commands::Invoker;

struct Grant {
    user: DiscordUser,
    challenge: String,
}

#[derive(Default)]
struct DiscordState {
    grants: BTreeMap<String, Grant>,
    tokens: BTreeMap<String, DiscordUser>,
    used_tokens: BTreeSet<String>,
    exchanges: u32,
    revoked: u32,
    redirect_uris: Vec<String>,
    unavailable: bool,
}

/// Codes work once and only with the verifier whose S256 challenge they were issued for.
#[derive(Default)]
pub struct FakeDiscord(Mutex<DiscordState>);

impl FakeDiscord {
    fn state(&self) -> std::sync::MutexGuard<'_, DiscordState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What Discord would do after the user approves: bind `code` to the request's challenge.
    pub fn approve(&self, code: &str, challenge: &str, user: DiscordUser) {
        self.state().grants.insert(
            code.into(),
            Grant {
                user,
                challenge: challenge.into(),
            },
        );
    }

    pub fn set_unavailable(&self, unavailable: bool) {
        self.state().unavailable = unavailable;
    }

    pub fn exchanges(&self) -> u32 {
        self.state().exchanges
    }

    pub fn revoked(&self) -> u32 {
        self.state().revoked
    }

    pub fn redirect_uris(&self) -> Vec<String> {
        self.state().redirect_uris.clone()
    }

    /// Tokens handed out and not yet revoked.
    pub fn live_tokens(&self) -> usize {
        self.state().tokens.len()
    }
}

impl DiscordApi for FakeDiscord {
    fn exchange_code<'a>(
        &'a self,
        exchange: CodeExchange<'a>,
    ) -> DiscordFuture<'a, Result<AccessToken, DiscordError>> {
        Box::pin(async move {
            let mut state = self.state();
            state.exchanges += 1;
            state
                .redirect_uris
                .push(exchange.client.redirect_uri.clone());
            if state.unavailable {
                return Err(DiscordError::Unavailable);
            }
            let grant = state
                .grants
                .remove(exchange.code)
                .ok_or(DiscordError::Rejected)?;
            if crypto::sha256_base64url(exchange.code_verifier.as_bytes()) != grant.challenge {
                return Err(DiscordError::Rejected);
            }
            let token = crypto::random_token().ok_or(DiscordError::Unavailable)?;
            state.tokens.insert(token.clone(), grant.user);
            Ok(AccessToken::new(token))
        })
    }

    fn current_user<'a>(
        &'a self,
        token: &'a AccessToken,
    ) -> DiscordFuture<'a, Result<DiscordUser, DiscordError>> {
        Box::pin(async move {
            let mut state = self.state();
            if !state.used_tokens.insert(token.expose().to_owned()) {
                panic!("an access token was used twice");
            }
            state
                .tokens
                .get(token.expose())
                .cloned()
                .ok_or(DiscordError::Rejected)
        })
    }

    fn revoke<'a>(&'a self, _: &'a DiscordClient, token: AccessToken) -> DiscordFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.state();
            state.revoked += 1;
            state.tokens.remove(token.expose());
        })
    }
}

#[derive(Default)]
struct GuildState {
    members: BTreeMap<Id<UserMarker>, Invoker>,
    owner: Option<Id<UserMarker>>,
    unavailable: bool,
}

#[derive(Default)]
pub struct FakeGuild(Mutex<GuildState>);

impl FakeGuild {
    fn state(&self) -> std::sync::MutexGuard<'_, GuildState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn put(&self, invoker: Invoker) {
        self.state().members.insert(invoker.user_id, invoker);
    }

    pub fn remove(&self, user_id: Id<UserMarker>) {
        self.state().members.remove(&user_id);
    }

    pub fn set_owner(&self, owner: Option<Id<UserMarker>>) {
        self.state().owner = owner;
    }

    pub fn set_unavailable(&self, unavailable: bool) {
        self.state().unavailable = unavailable;
    }
}

impl GuildMembers for FakeGuild {
    fn member(&self, user_id: Id<UserMarker>) -> GateFuture<'_, Result<Option<Invoker>, ()>> {
        Box::pin(async move {
            let state = self.state();
            if state.unavailable {
                return Err(());
            }
            Ok(state.members.get(&user_id).cloned())
        })
    }

    fn owner_id(&self) -> Option<Id<UserMarker>> {
        self.state().owner
    }
}
