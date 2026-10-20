//! `setup.json#/$defs/SetupView`: admin Setup's read-only checklist. Facts
//! only (ids, states, codes, counts, ids of channels); the admin app owns
//! every word, and no secret, path or URL of the deployment is ever sent.

use serde::Serialize;

crate::domain::spelled! {
    /// One checklist item; [`SetupItemId::ALL`] is the answer's order,
    /// required items first.
    #[derive(Serialize)]
    #[cfg_attr(test, derive(ts_rs::TS))]
    #[serde(rename_all = "snake_case")]
    SetupItemId {
        DiscordGateway => "discord_gateway",
        DiscordIntents => "discord_intents",
        BotAccess => "bot_access",
        DigestChannel => "digest_channel",
        WatchedChannels => "watched_channels",
        ModelGateway => "model_gateway",
        ExtractionModel => "extraction_model",
        Chat => "chat",
        Persona => "persona",
        AdminSignIn => "admin_sign_in",
        PublicPortal => "public_portal",
        Roster => "roster",
        CharacterData => "character_data",
        Guides => "guides",
        Scouter => "scouter",
        Backups => "backups",
    }
}

impl SetupItemId {
    /// Setup is complete once every required item is `ok`.
    pub fn required(self) -> bool {
        matches!(
            self,
            Self::DiscordGateway
                | Self::DiscordIntents
                | Self::BotAccess
                | Self::DigestChannel
                | Self::WatchedChannels
                | Self::ModelGateway
                | Self::ExtractionModel
        )
    }
}

crate::domain::spelled! {
    #[derive(Serialize)]
    #[cfg_attr(test, derive(ts_rs::TS))]
    #[serde(rename_all = "snake_case")]
    SetupState {
        Ok => "ok",
        Missing => "missing",
        /// Set up, but something needs a look (see `reason`).
        Warn => "warn",
        /// An optional feature this deployment leaves off.
        Off => "off",
    }
}

crate::domain::spelled! {
    /// Where an item's value comes from.
    #[derive(Serialize)]
    #[cfg_attr(test, derive(ts_rs::TS))]
    #[serde(rename_all = "snake_case")]
    SetupSource {
        /// The deployment's environment (or kanade.toml): a restart applies a change.
        Environment => "environment",
        /// A deployment file.
        File => "file",
        /// Saved in admin Config.
        Saved => "saved",
        /// Worked out by the server from live state.
        Derived => "derived",
    }
}

crate::domain::spelled! {
    /// Why an item is not `ok`.
    #[derive(Serialize)]
    #[cfg_attr(test, derive(ts_rs::TS))]
    #[serde(rename_all = "snake_case")]
    SetupReason {
        /// The Discord gateway is switched off (`KANADE_DISCORD_GATEWAY=0`)
        /// or the server runs offline.
        GatewayOff => "gateway_off",
        /// The gateway is connecting or reconnecting.
        Connecting => "connecting",
        /// Discord ended the session for good (a bad token or a disallowed
        /// intent); the bot does not reconnect until it restarts.
        GatewayClosed => "gateway_closed",
        /// The guild is not loaded, so channel permissions are unknown.
        NotConnected => "not_connected",
        /// A fact this server could not read in time (Discord's application,
        /// a module's health).
        Unavailable => "unavailable",
        /// The model gateway or the scouter did not answer.
        Unreachable => "unreachable",
        /// A model is saved for the role but not running yet.
        NotRunning => "not_running",
        /// The chatbot is on but `missing_env` names settings it still needs.
        EnvMissing => "env_missing",
        /// The public listener exists but the member Discord sign-in is not configured.
        MemberSignInMissing => "member_sign_in_missing",
        /// Character data has no NEXON key.
        NexonNoKey => "nexon_no_key",
        /// NEXON refused the key.
        NexonKeyRefused => "nexon_key_refused",
        /// NEXON calls are held for maintenance.
        NexonBackoff => "nexon_backoff",
        /// The scouter wants a verification before it answers again.
        VerificationRequired => "verification_required",
    }
}

crate::domain::spelled! {
    /// A privileged gateway intent the bot needs (Developer Portal, Bot page).
    #[derive(Serialize)]
    #[cfg_attr(test, derive(ts_rs::TS))]
    #[serde(rename_all = "snake_case")]
    PrivilegedIntent {
        ServerMembers => "server_members",
        MessageContent => "message_content",
    }
}

/// `GET /api/admin/setup`.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SetupView {
    /// Every required item is `ok`.
    pub complete: bool,
    /// Every [`SetupItemId`], in its order.
    pub items: Vec<SetupItem>,
}

impl SetupView {
    pub fn new(items: Vec<SetupItem>) -> Self {
        Self {
            complete: items
                .iter()
                .all(|item| !item.required || item.state == SetupState::Ok),
            items,
        }
    }
}

/// One checklist item: its state plus the facts that item needs.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SetupItem {
    pub id: SetupItemId,
    pub required: bool,
    pub state: SetupState,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub source: Option<SetupSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub reason: Option<SetupReason>,
    /// `digest_channel`: the channel set.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub channel_id: Option<String>,
    /// `discord_intents`: the intents still off (empty when all are on).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub missing_intents: Option<Vec<PrivilegedIntent>>,
    /// `chat`: the env-only settings the chatbot still needs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub missing_env: Option<Vec<String>>,
    /// `bot_access`: the link that adds the bot with the six permissions
    /// the access report checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub invite_url: Option<String>,
    /// `bot_access`: watched and digest channels missing any of the six.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub channels_lacking: Option<u32>,
    /// `watched_channels`: watched channels plus watched categories.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub count: Option<u32>,
    /// `public_portal`: the live open switch (`self_service.public_portal`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub open: Option<bool>,
    /// `backups`: RFC 3339 UTC, the newest listed backup.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub latest_backup_at: Option<String>,
    /// `backups`: age recipients are configured (snapshots are encrypted).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub recipients: Option<bool>,
}

impl SetupItem {
    pub fn new(id: SetupItemId, state: SetupState) -> Self {
        Self {
            id,
            required: id.required(),
            state,
            source: None,
            reason: None,
            channel_id: None,
            missing_intents: None,
            missing_env: None,
            invite_url: None,
            channels_lacking: None,
            count: None,
            open: None,
            latest_backup_at: None,
            recipients: None,
        }
    }

    pub fn because(mut self, reason: SetupReason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn from_source(mut self, source: SetupSource) -> Self {
        self.source = Some(source);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serde spelling is the `spelled!` one, so the schema enums and
    /// `as_str` never drift apart.
    #[test]
    fn wire_spellings_match_as_str() {
        fn check<T: Serialize + Copy>(all: &[T], text: fn(T) -> &'static str) {
            for value in all {
                assert_eq!(serde_json::to_value(value).unwrap(), text(*value));
            }
        }
        check(SetupItemId::ALL, SetupItemId::as_str);
        check(SetupState::ALL, SetupState::as_str);
        check(SetupSource::ALL, SetupSource::as_str);
        check(SetupReason::ALL, SetupReason::as_str);
        check(PrivilegedIntent::ALL, PrivilegedIntent::as_str);
    }

    /// Complete once every required item is `ok`; optional ones never count.
    #[test]
    fn complete_needs_every_required_item_ok() {
        let all = |state: fn(SetupItemId) -> SetupState| {
            SetupView::new(
                SetupItemId::ALL
                    .iter()
                    .map(|&id| SetupItem::new(id, state(id)))
                    .collect(),
            )
        };
        let required_ok = all(|id| {
            if id.required() {
                SetupState::Ok
            } else {
                SetupState::Missing
            }
        });
        assert!(required_ok.complete);
        let one_short = all(|id| {
            if id == SetupItemId::ExtractionModel {
                SetupState::Warn
            } else {
                SetupState::Ok
            }
        });
        assert!(!one_short.complete);
        assert_eq!(
            SetupItemId::ALL.iter().filter(|id| id.required()).count(),
            7
        );
        assert!(SetupItemId::ALL[..7].iter().all(|id| id.required()));
    }
}
