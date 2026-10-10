//! Checks `web/packages/api-types/src/generated.ts` against the API DTOs (ts-rs).
//! Regenerate it with `KANADE_WRITE_TS=1 cargo test --all-features --lib ts_bindings`.

use std::{fs, path::PathBuf};

use ts_rs::{Config, TS};

use super::{
    admin::{
        self,
        config::{AccessReport, AccessRow},
    },
    assets::Identity,
    dto::{
        self, account, bosses, config, events, fixed, history, inbox, inbox_past, limits, logs,
        members, reminders, rescan, sign_ins, week,
    },
    error, public,
};

const TARGET: &str = "web/packages/api-types/src/generated.ts";

/// Hand-written vocabulary (`manual.ts`) the `#[ts(type)]` overrides name.
const MANUAL: [&str; 26] = [
    "ActorKind",
    "Answer",
    "ChangeRecord",
    "ChatOutcome",
    "ChatRoute",
    "ContextSource",
    "Difficulty",
    "DifficultyName",
    "ExtractionOutcome",
    "IdListSource",
    "KnowledgeDoc",
    "MessageStyle",
    "MissionSeries",
    "PingLevel",
    "ProposalKind",
    "Refusal",
    "RewriteKind",
    "RewriteStage",
    "RewriteVerdict",
    "RowKey",
    "RunStatus",
    "SelfServiceMode",
    "SignInEvent",
    "SignInMethod",
    "SignInRealm",
    "Surface",
];

struct Out {
    cfg: Config,
    body: String,
}

impl Out {
    fn add<T: TS>(&mut self) -> &mut Self {
        self.body.push('\n');
        if let Some(docs) = T::docs() {
            self.body.push_str(&docs);
        }
        self.body.push_str("export ");
        self.body.push_str(&T::decl(&self.cfg));
        self.body.push('\n');
        self
    }
}

fn bindings() -> String {
    // The web apps treat every integer as `number`; no API value exceeds 2^53.
    let mut out = Out {
        cfg: Config::new().with_large_int("number"),
        body: String::new(),
    };
    out.add::<dto::Boss>()
        .add::<dto::Named>()
        .add::<dto::RoleRow>()
        .add::<public::PublicStatus>()
        .add::<dto::public::PublicMember>()
        .add::<dto::public::PublicSession>()
        .add::<dto::public::PublicSessionRow>()
        .add::<dto::public::PublicSessions>()
        .add::<dto::public::MemberRun>()
        .add::<dto::public::MemberWeek>()
        .add::<dto::public::MemberAllowance>()
        .add::<dto::public::MemberOwnerRequest>()
        .add::<dto::public::MemberTiming>()
        .add::<dto::public::MemberTimings>()
        .add::<dto::public::MemberRunResult>()
        .add::<dto::public::MemberMoveResult>()
        .add::<dto::public::MemberRemoved>()
        .add::<dto::public::MemberTimingSlot>()
        .add::<dto::public::MemberRunLink>()
        .add::<dto::public::MemberProposed>()
        .add::<dto::public::MemberRequest>()
        .add::<dto::public::MemberRequestOptions>()
        .add::<dto::public::MemberRequests>()
        .add::<dto::public::MemberRequestLimit>()
        .add::<error::Body>()
        .add::<Identity>()
        .add::<admin::SessionView>()
        .add::<admin::Methods>()
        // Week board
        .add::<week::CardKind>()
        .add::<week::CardState>()
        .add::<week::WeekKey>()
        .add::<week::WeekDay>()
        .add::<week::Tally>()
        .add::<week::Participant>()
        .add::<week::ReminderCard>()
        .add::<week::RosterChange>()
        .add::<week::RunDto>()
        .add::<week::Week>()
        .add::<week::DayStat>()
        .add::<week::Stats>()
        .add::<week::NextRun>()
        .add::<week::Model>()
        .add::<week::Summary>()
        .add::<week::TonightRun>()
        .add::<week::Tonight>()
        .add::<admin::RunResult>()
        .add::<admin::Previous>()
        .add::<admin::MoveResult>()
        .add::<admin::SwapResult>()
        .add::<admin::Message>()
        // Members, timings, reminders, bosses
        .add::<members::MemberRow>()
        .add::<members::Persona>()
        .add::<fixed::FixedRunLink>()
        .add::<fixed::FixedRow>()
        .add::<admin::ValidateResult>()
        .add::<reminders::ReminderState>()
        .add::<reminders::ReminderRow>()
        .add::<reminders::Reminders>()
        .add::<reminders::CardField>()
        .add::<reminders::CardPreview>()
        .add::<reminders::EmbedPreview>()
        .add::<reminders::ReminderPreview>()
        .add::<bosses::DifficultyOption>()
        .add::<bosses::BossRow>()
        .add::<bosses::MissionStop>()
        .add::<bosses::Knowledge>()
        .add::<bosses::PublicKnowledge>()
        .add::<bosses::EventBoss>()
        // Inbox
        .add::<inbox::InboxTab>()
        .add::<inbox::ProposalSource>()
        .add::<inbox::ProposalFlag>()
        .add::<inbox::Evidence>()
        .add::<inbox::ThreadMessage>()
        .add::<inbox::FieldChange>()
        .add::<inbox::FieldConflict>()
        .add::<inbox::Preview>()
        .add::<inbox::Choice>()
        .add::<inbox::SelfService>()
        .add::<inbox::ProposalDto>()
        .add::<inbox::OwnerRequestDto>()
        .add::<inbox_past::PastOutcome>()
        .add::<inbox_past::Decider>()
        .add::<inbox_past::PastItem>()
        .add::<inbox_past::PastPage>()
        // Config
        .add::<config::ConfigView>()
        .add::<config::Pings>()
        .add::<config::Watching>()
        .add::<config::Rate>()
        .add::<config::Chatbot>()
        .add::<config::Notifications>()
        .add::<config::SelfService>()
        .add::<config::PersonaEntry>()
        .add::<config::ReplyProfile>()
        .add::<config::RoleProfile>()
        .add::<config::Persona>()
        .add::<config::Models>()
        .add::<config::ModelInfo>()
        .add::<config::Admission>()
        .add::<config::Roles>()
        .add::<config::RoleModel>()
        .add::<config::Running>()
        .add::<config::EffectiveContext>()
        .add::<config::ContextRole>()
        .add::<config::ContextSettings>()
        .add::<config::CapacityGroup>()
        .add::<config::AliasLimit>()
        .add::<config::KeyLimits>()
        .add::<config::CapacityCheck>()
        .add::<config::RunLengths>()
        .add::<config::RunLengthOverride>()
        .add::<config::Profanity>()
        .add::<config::ManageMessages>()
        .add::<config::EnvRow>()
        .add::<config::LastDigest>()
        .add::<AccessReport>()
        .add::<AccessRow>()
        // Chat and Extractions logs, rescans
        .add::<logs::LogFacets>()
        .add::<logs::UsageSummary>()
        .add::<logs::ExtractionSummary>()
        .add::<logs::ExtractionRow>()
        .add::<logs::Extractions>()
        .add::<logs::Amendment>()
        .add::<logs::ReadMessage>()
        .add::<logs::CallContext>()
        .add::<logs::ExtractionRefusal>()
        .add::<logs::Extraction>()
        .add::<logs::RewriteFacets>()
        .add::<logs::RewriteSummary>()
        .add::<logs::RewriteRow>()
        .add::<logs::Rewrites>()
        .add::<logs::Rewrite>()
        .add::<rescan::JobState>()
        .add::<rescan::ChannelState>()
        .add::<rescan::RescanChannel>()
        .add::<rescan::RescanJob>()
        .add::<logs::ChatRow>()
        .add::<logs::ChatSummary>()
        .add::<logs::Chat>()
        .add::<logs::ChatToolCall>()
        .add::<logs::RoundGuardrail>()
        .add::<logs::ChatRoundFacts>()
        .add::<logs::ChatCard>()
        .add::<logs::MaskedRoundView>()
        .add::<logs::TokenName>()
        .add::<logs::ModelView>()
        .add::<logs::ProfanityDetail>()
        .add::<logs::ChatTurn>()
        // Limits
        .add::<limits::Permits>()
        .add::<limits::QueuedCall>()
        .add::<limits::RateLevel>()
        .add::<limits::RetryLevel>()
        .add::<limits::Breaker>()
        .add::<limits::BackendGroup>()
        .add::<limits::AdmissionWindow>()
        .add::<limits::Quota>()
        .add::<limits::Allowance>()
        .add::<limits::Limits>()
        // Account
        .add::<account::ReplyStyleRef>()
        .add::<account::ReplyStyle>()
        .add::<account::MeMember>()
        .add::<account::Me>()
        .add::<account::AccountSession>()
        .add::<account::AccountSessions>()
        .add::<account::SessionsEnded>()
        // History
        .add::<history::ChainHead>()
        .add::<history::HistoryPage>()
        .add::<history::SettingsActor>()
        .add::<history::SettingRowDiff>()
        .add::<history::SettingsChangeRow>()
        .add::<history::RowChange>()
        .add::<history::RowConflict>()
        .add::<history::SkippedKey>()
        .add::<history::PlanOutcome>()
        .add::<history::RevertPlan>()
        .add::<history::BackupAnchor>()
        .add::<history::BackupRow>()
        .add::<history::Verified>()
        .add::<history::Checkpoints>()
        .add::<sign_ins::SignInRow>()
        .add::<sign_ins::SignInPage>()
        // Live updates
        .add::<events::Topic>()
        .add::<events::EventHint>()
        .add::<events::EventReady>();
    let used: Vec<&str> = MANUAL
        .into_iter()
        .filter(|name| mentions(&out.body, name))
        .collect();
    format!(
        "// Generated from the Rust API DTOs by src/api/ts_bindings.rs; do not edit.\n\
         // Regenerate: KANADE_WRITE_TS=1 cargo test --all-features --lib ts_bindings\n\n\
         import type {{ {} }} from './manual';\n{}",
        used.join(", "),
        out.body
    )
}

/// `name` appears as a whole identifier.
fn mentions(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(name)
        .any(|(at, _)| !text[..at].ends_with(ident) && !text[at + name.len()..].starts_with(ident))
}

#[test]
fn generated_typescript_is_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TARGET);
    let fresh = bindings();
    if std::env::var_os("KANADE_WRITE_TS").is_some() {
        fs::write(&path, &fresh).expect("write generated.ts");
        return;
    }
    let current = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == fresh,
        "{TARGET} is stale: regenerate it with `KANADE_WRITE_TS=1 cargo test --all-features --lib ts_bindings`"
    );
}
