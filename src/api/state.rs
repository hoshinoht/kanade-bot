//! What admin read handlers need: one shared store (reads use reader
//! connections, never the writer), the schedule policy, the boss catalog and
//! the guild facts owned elsewhere (channels, personas, staff rule).

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};

use crate::{
    app::{
        clock::Clock,
        config::ConfigDesk,
        guild::{ChannelList, GuildAccess},
        ports::{
            DeclineRetraction, DigestPost, GuidePort, HeaderRewritePort, ModelLimits,
            ProposalCardRefresh,
        },
        read::{CharacterRoster, ReadStore},
        rescan::RescanDesk,
        write::Writer,
    },
    chat::persona::PersonaOption,
};
use kanade_domain::{catalog::BossTable, schedule::SchedulePolicy};

/// The bot's application (`GET /applications/@me` through serve's
/// transport) for admin Setup: `None` when Discord did not answer or
/// refused. Absent while Discord is not composed.
pub type ApplicationPort = Arc<
    dyn Fn() -> Pin<
            Box<dyn Future<Output = Option<crate::discord::transport::ApplicationInfo>> + Send>,
        > + Send
        + Sync,
>;

/// Where `kanade backup` writes (History checkpoints read it, never write).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackupDir {
    /// `KANADE_BACKUP_DIR`; `None` lists no backups.
    pub dir: Option<PathBuf>,
    /// The open store's schema version: older backups are flagged.
    pub schema_version: i64,
}

/// Deployment facts admin Setup reports, fixed for the process and only as
/// presence (never a value, a path or a secret).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetupFacts {
    /// A public listener with a host: the member portal exists.
    pub public_listener: bool,
    /// The member realm's Discord sign-in (client id, secret, redirect).
    pub member_sign_in: bool,
    /// `modules.*`, read once at startup.
    pub modules: crate::runtime::config::Modules,
    /// The backups directory holds `recipients` (snapshots are encrypted).
    pub backup_recipients: bool,
}

/// Checks a candidate secret against its service without keeping it.
pub trait SecretChecks: Send + Sync {
    fn check<'a>(
        &'a self,
        name: super::dto::setup::SecretName,
        value: &'a str,
    ) -> Pin<Box<dyn Future<Output = super::dto::setup::SecretCheck> + Send + 'a>>;
}

/// Setup's "Restart now": checks the next start's configuration, then
/// starts the graceful shutdown; the error names what would fail.
pub type RestartPort = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// Secrets an admin may save from Setup (present only with a saved
/// directory), with their checks and the restart that applies them.
pub struct SecretDesk {
    pub files: Vec<crate::runtime::secrets::SecretFile>,
    pub checks: Arc<dyn SecretChecks>,
    /// `None` without a supervisor to start the process again.
    pub restart: Option<RestartPort>,
    /// Saved since this process started (read only at the next start).
    saved: std::sync::Mutex<std::collections::BTreeSet<&'static str>>,
}

impl SecretDesk {
    pub fn new(
        files: Vec<crate::runtime::secrets::SecretFile>,
        checks: Arc<dyn SecretChecks>,
        restart: Option<RestartPort>,
    ) -> Self {
        Self {
            files,
            checks,
            restart,
            saved: Default::default(),
        }
    }

    pub fn file(&self, name: &str) -> Option<&crate::runtime::secrets::SecretFile> {
        self.files.iter().find(|file| file.name() == name)
    }

    pub fn mark_saved(&self, name: &'static str) {
        self.saved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name);
    }

    pub fn restart_pending(&self) -> bool {
        !self
            .saved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    }
}

/// Everything the admin read routes compose.
pub struct ApiState {
    pub store: Arc<dyn ReadStore>,
    /// The one scheduler writer (a mutex inside); reads never go through it.
    pub writer: Arc<dyn Writer>,
    pub policy: SchedulePolicy,
    pub catalog: Arc<BossTable>,
    pub channels: Arc<dyn ChannelList>,
    pub access: Arc<GuildAccess>,
    /// Tracked `boss/knowledge/` (schema v2).
    pub knowledge_dir: Option<PathBuf>,
    /// For message links on posted cards.
    pub guild_id: Option<String>,
    pub clock: Clock,
    /// Rescan jobs; `None` until the extractor is composed (503).
    pub rescans: Option<Arc<RescanDesk>>,
    /// Runtime settings (A9); `None` answers `unavailable`.
    pub config: Option<Arc<ConfigDesk>>,
    /// The chat pilot's Limits view and status (for A8); empty until serve
    /// starts chat.
    pub chat: Option<Arc<crate::chat::driver::ChatHandle>>,
    /// Live model-governor groups for the Limits page.
    pub model_limits: Option<ModelLimits>,
    /// Bounded idempotency memory for Limits and manual digest operations.
    pub limits: Arc<super::admin::limits::LimitsDesk>,
    /// The shared CardDesk refresh, attached only after Discord composition.
    pub proposal_refresh: Option<ProposalCardRefresh>,
    /// The shared delivery retraction, attached only after Discord composition.
    pub decline_retraction: Option<DeclineRetraction>,
    /// The delivery-owned manual digest port, absent while Discord is offline.
    pub digest_post: Option<DigestPost>,
    /// The delivery-owned manual header rewrite, absent while Discord is offline.
    pub header_rewrite: Option<HeaderRewritePort>,
    /// The bot's application for admin Setup, attached with serve's Discord
    /// side; absent while Discord is offline.
    pub application: Option<ApplicationPort>,
    pub backups: BackupDir,
    /// What admin Setup reports about the deployment.
    pub setup: SetupFacts,
    /// Secrets saved from Setup; `None` without a saved directory.
    pub setup_secrets: Option<Arc<SecretDesk>>,
    /// Member and admin portraits; `None` serves monograms only.
    pub avatars: Option<Arc<crate::discord::avatars::AvatarCache>>,
    /// Change hints for open admin pages (`GET /api/admin/events`), fed by
    /// the store's write hook.
    pub events: Arc<super::events::Hub>,
    /// Who else has an Inbox request or a run sheet open
    /// (`/api/admin/presence`): this process's memory only, never stored and
    /// never read by a write; a new state (a restart) starts it empty.
    pub presence: super::presence::Presence,
    /// Difficulty emojis for redesigned card previews, as the bot listed
    /// them at startup; empty (written labels) until Discord composes.
    pub marks: crate::cards::DifficultyMarks,
    /// The manual character roster (`modules.roster`, read at startup);
    /// `None` answers its routes `404`.
    pub characters: Option<Arc<dyn CharacterRoster>>,
    /// Character data (`modules.character_data`, read at startup); `None`
    /// answers its routes `404` and leaves Config without its health block.
    pub character_data: Option<Arc<kanade_chardata::character_data::CharacterData>>,
    /// The guide publisher (`modules.guides`, read at startup), attached by
    /// serve with its worker; `None` answers every guide route `404`.
    pub guides: Option<Arc<dyn GuidePort>>,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("zone", &self.policy.zone())
            .field("personas", &self.profile_options().len())
            .field("knowledge_dir", &self.knowledge_dir)
            .finish_non_exhaustive()
    }
}

impl ApiState {
    /// Public reply-profile choices from the config desk. Missing settings
    /// fail closed rather than inferring visibility from readable files.
    pub fn profile_options(&self) -> Vec<PersonaOption> {
        self.config
            .as_ref()
            .map(|desk| desk.profile_choices().options)
            .unwrap_or_default()
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// When runs end, live from the config desk (defaults without one).
    pub fn run_ends_source(&self) -> kanade_domain::completion::RunEndsSource {
        match &self.config {
            Some(desk) => desk.run_ends(Arc::clone(&self.catalog), self.policy.clone()),
            None => kanade_domain::completion::RunEndsSource::fixed(
                Default::default(),
                Some(Arc::clone(&self.catalog)),
                self.policy.clone(),
            ),
        }
    }

    /// Refresh the proposal cards touched by a committed inbox decision.
    pub async fn refresh_proposals(&self, proposal_ids: Vec<String>) {
        if !proposal_ids.is_empty()
            && let Some(refresh) = &self.proposal_refresh
        {
            refresh(proposal_ids).await;
        }
    }

    /// Delete a bound decline notice after its RSVP commit. Delivery failures
    /// are intentionally contained: S2 keeps the durable pending retraction
    /// for the next recovery tick.
    pub async fn retract_decline(&self, run_id: String, user_id: String) {
        if let Some(retract) = &self.decline_retraction {
            retract(run_id, user_id, self.now()).await;
        }
    }
}
