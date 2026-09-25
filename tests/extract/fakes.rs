//! Pipeline-level fakes: a pinned wall clock, counted ids, a guild, the real
//! scheduler proposal service over the memory store, an outbox recorder, a
//! history source and a `FakeProvider` behind a real governor.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use kanade::chat::nudge::{GovernedRewriter, Nudger, SharedRewriter};
use kanade::chat::persona::{CompiledPersona, PersonaId, parse_bundle};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use kanade::bot::delivery::StoreRef;
use kanade::domain::catalog::BossTable;
use kanade::domain::drafts::{ProposalStore, StoredProposal};
use kanade::domain::history::Origin;
use kanade::domain::ids::IdGenerator;
use kanade::domain::members::{self, Directory};
use kanade::domain::model_log::{ExtractionFilter, ExtractionLog, ModelLogStore};
use kanade::domain::proposals::Approver;
use kanade::domain::schedule::{NewRun, ReminderPolicy, RunSource, RunStatus, SchedulePolicy};
use kanade::domain::scheduler::{
    ProposalApproved, ProposalError, ProposalRequest, ProposalResult, Proposed, SchedulerService,
    SupersedeScope,
};
use kanade::extract::pipeline::{
    AuthorKind, BacklogDrop, Card, ChatAnswer, Deps, Extractor, Guild, IncomingMessage,
    MessageEvent, MessageOrigin, Outbox, Personas, Pipeline, PipelineConfig, PostResult, Proposer,
    Redirected, SelfServiceDeps,
};
use kanade::extract::redirect::PublicPortalLinks;
use kanade::extract::rescan::History;
use kanade::infrastructure::llm::governor::{
    Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, Random, Role, RoleConfig,
};
use kanade::infrastructure::llm::identity::{IdentityCodec, Member, Passthrough};
use kanade::infrastructure::llm::{
    CompletionResponse, ExecutionLimits, FakeAction, FakeProvider, FinishReason, RetryPolicy, Usage,
};
use kanade::infrastructure::store::{MemoryScheduleStore, SqliteStore};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::common::TestClock;
use crate::support;

pub const CHANNEL: &str = "900";
pub const OTHER: &str = "901";
pub const ALIAS: &str = "extractor";
pub const MY: &str = "114200000000000011";
pub const ALVIN: &str = "114200000000000022";
pub const PRIYA: &str = "114200000000000033";
pub const KANON: &str = "114200000000000044";
pub const STRANGER: &str = "114200000000000099";

pub type Service<'a> = SchedulerService<StoreRef<'a, MemoryScheduleStore>, Ids, TestClock>;
pub type Core = Extractor<MemoryScheduleStore, FakeProvider, Scheduler, Recorder>;

pub fn zone() -> Tz {
    chrono_tz::Asia::Kuala_Lumpur
}

/// A guild-local wall clock instant.
pub fn local(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    zone()
        .with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .single()
        .expect("unambiguous")
        .with_timezone(&Utc)
}

/// Sunday 30 Aug 2026 13:07, as the plan vectors.
pub fn now() -> DateTime<Utc> {
    local(8, 30, 13, 7)
}

pub fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: zone(),
            ping_time: NaiveTime::from_hms_opt(18, 0, 0).expect("time"),
            countdowns: Vec::new(),
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

pub fn config() -> PipelineConfig {
    PipelineConfig::new(zone(), Weekday::Thu, NaiveTime::MIN)
}

/// Counted UUID-shaped ids; clones share the counter.
#[derive(Clone)]
pub struct Ids {
    next: Arc<AtomicUsize>,
    block: u32,
}

impl Ids {
    pub fn new(block: u32) -> Self {
        Self {
            next: Arc::new(AtomicUsize::new(1)),
            block,
        }
    }
}

impl IdGenerator for Ids {
    fn new_id(&mut self) -> String {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        format!("{:08x}-0000-4000-8000-{n:012}", self.block)
    }
}

fn member(user_id: &str, name: &str) -> Member {
    Member {
        user_id: user_id.into(),
        display_name: name.into(),
        nickname: None,
        aliases: Vec::new(),
    }
}

pub struct FakeGuild {
    pub members: Vec<Member>,
    pub roles: HashSet<String>,
    pub bosses: Arc<BossTable>,
    pub enabled: AtomicBool,
}

impl FakeGuild {
    fn new() -> Self {
        let raw = support::load("plan.json");
        let bosses = support::catalog(&raw["cases"][0]["input"]["catalog"]);
        Self {
            members: vec![
                // Names that are not English words, so the leak finder
                // cannot mistake prompt prose for a member.
                member(MY, "Mylene"),
                member(ALVIN, "Alvin"),
                member(PRIYA, "Priya"),
                member(KANON, "kanon"),
                member(STRANGER, "Zorblax"),
            ],
            roles: [MY, ALVIN, PRIYA, KANON].map(str::to_owned).into(),
            bosses: Arc::new(bosses),
            enabled: AtomicBool::new(true),
        }
    }
}

impl Guild for FakeGuild {
    fn extraction_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        channel_id == CHANNEL || channel_id == OTHER
    }

    fn members(&self) -> Vec<Member> {
        self.members.clone()
    }

    fn has_role(&self, user_id: &str) -> bool {
        self.roles.contains(user_id)
    }

    fn bosses(&self) -> Arc<BossTable> {
        self.bosses.clone()
    }

    fn channel_name(&self, channel_id: &str) -> String {
        format!("party-{channel_id}")
    }
}

/// Every roster member holds the bossing role.
pub struct Roster(HashSet<String>);

impl Directory for Roster {
    fn member(&self, user_id: &str) -> Option<members::Member> {
        self.0.contains(user_id).then(|| members::Member {
            user_id: user_id.to_owned(),
            has_role: true,
            ..members::Member::default()
        })
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

/// The scheduler's real proposal service, one short-lived service per call.
pub struct Scheduler {
    pub store: Arc<MemoryScheduleStore>,
    pub clock: TestClock,
    pub ids: Ids,
    pub directory: Roster,
    /// Answer every `propose` with `NoEffect`, as an up-front refusal.
    pub refuse: AtomicBool,
}

impl Scheduler {
    pub fn service(&self) -> Service<'_> {
        SchedulerService::new(StoreRef(&*self.store), self.ids.clone(), self.clock.clone())
    }

    pub async fn approve(&self, id: &str) -> ProposalResult<ProposalApproved> {
        let admin = Approver {
            user_id: "admin".into(),
            has_role: true,
            is_admin: true,
        };
        self.service()
            .approve_proposal(id, &admin, &policy(), &self.directory)
            .await
    }
}

impl Proposer for Scheduler {
    async fn supersede(&self, scope: SupersedeScope<'_>) -> ProposalResult<Vec<String>> {
        self.service().supersede_proposals(scope).await
    }

    async fn propose(&self, request: ProposalRequest) -> ProposalResult<Proposed> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(ProposalError::NoEffect);
        }
        self.service()
            .propose(request, &policy(), &self.directory)
            .await
    }
}

#[derive(Default)]
pub struct Recorder {
    pub cards: Mutex<Vec<Card>>,
    pub answers: Mutex<Vec<ChatAnswer>>,
    pub drops: Mutex<Vec<BacklogDrop>>,
    pub redirects: Mutex<Vec<Redirected>>,
    /// Report every card and link as never posted.
    pub fail_posts: AtomicBool,
    /// Report every card and link as saved for a later pass.
    pub pending_posts: AtomicBool,
}

impl Recorder {
    fn result(&self) -> PostResult {
        if self.fail_posts.load(Ordering::SeqCst) {
            PostResult::NotPosted
        } else if self.pending_posts.load(Ordering::SeqCst) {
            PostResult::Pending
        } else {
            PostResult::Posted
        }
    }
}

impl Outbox for Recorder {
    async fn redirect(&self, redirected: Redirected) -> PostResult {
        self.redirects.lock().unwrap().push(redirected);
        self.result()
    }

    async fn card(&self, card: Card) -> PostResult {
        self.cards.lock().unwrap().push(card);
        self.result()
    }

    async fn answers(&self, answers: Vec<ChatAnswer>) {
        self.answers.lock().unwrap().extend(answers);
    }

    async fn backlog_dropped(&self, drop: BacklogDrop) {
        self.drops.lock().unwrap().push(drop);
    }
}

/// Discord history by channel; `backfill` returns what is at or after `since`.
#[derive(Default)]
pub struct FakeHistory {
    pub messages: Mutex<BTreeMap<String, Vec<IncomingMessage>>>,
    pub calls: Mutex<Vec<(String, DateTime<Utc>)>>,
}

impl History for FakeHistory {
    async fn backfill(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<IncomingMessage>, String> {
        self.calls
            .lock()
            .unwrap()
            .push((channel_id.to_owned(), since));
        Ok(self
            .messages
            .lock()
            .unwrap()
            .get(channel_id)
            .map(|found| {
                found
                    .iter()
                    .filter(|message| message.created_at >= since)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }
}

pub const PORTAL: &str = "https://kanade-pub.example.dev";

/// The tracked Kanade bundle, no profile.
pub fn kanade() -> CompiledPersona {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/config/personas/bundles/kanade.yaml"
    );
    let text = std::fs::read_to_string(path).expect("tracked bundle");
    let bundle = parse_bundle(&text, &PersonaId::parse("kanade").unwrap()).expect("bundle");
    CompiledPersona::compile(&bundle, None)
}

struct KanadeForAll(CompiledPersona);

impl Personas for KanadeForAll {
    fn persona_for(&self, _member_id: &str) -> Option<CompiledPersona> {
        Some(self.0.clone())
    }
}

struct Fixed;

impl Random for Fixed {
    fn next_u64(&self) -> u64 {
        1 << 63
    }
}

pub fn reply(content: &str) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: ALIAS.into(),
        content: Some(content.to_owned()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
        }),
    })
}

/// A reply stopped by the provider's content filter.
pub fn filtered() -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: ALIAS.into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::ContentFilter,
        usage: Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
        }),
    })
}

/// No amendments.
pub fn nothing() -> FakeAction {
    reply(r#"{"amendments": [], "summary": "no schedule change"}"#)
}

/// `grouped: false` routes extraction to an alias in no backend group, which
/// the governor refuses for good.
pub fn client(
    actions: Vec<FakeAction>,
    grouped: bool,
) -> (Arc<FakeProvider>, Arc<ModelClient<FakeProvider>>) {
    let config = GovernorConfig {
        groups: vec![GroupConfig {
            name: "local".into(),
            backend: "local backend".into(),
            permits: 1,
            requests_per_min: 6_000,
            burst: Some(1_000),
            aliases: vec![ALIAS.into()],
        }],
        roles: [Role::Chat, Role::Extraction, Role::Rewrite]
            .into_iter()
            .map(|role| {
                let alias = if grouped || role != Role::Extraction {
                    ALIAS
                } else {
                    "ungrouped"
                };
                let route = RoleConfig {
                    alias: alias.into(),
                    external: false,
                };
                (role, route)
            })
            .collect::<BTreeMap<_, _>>(),
        policy: GovernorPolicy::default(),
    };
    let governor = Arc::new(Governor::new(&config, Arc::new(Fixed)).expect("valid config"));
    let provider = Arc::new(FakeProvider::new(actions));
    let retry = RetryPolicy {
        total_deadline: Duration::from_secs(30),
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    let client = ModelClient::new(
        governor,
        provider.clone(),
        ExecutionLimits::default(),
        retry,
    )
    .expect("valid client");
    (provider, Arc::new(client))
}

pub struct World {
    pub store: Arc<MemoryScheduleStore>,
    pub provider: Arc<FakeProvider>,
    pub outbox: Arc<Recorder>,
    pub guild: Arc<FakeGuild>,
    pub scheduler: Arc<Scheduler>,
    pub extractor: Arc<Core>,
    /// Mon 21:30 HMaleficStar+HFA (MY, Alvin, Priya), Tue 22:00
    /// HCarling+XKalos (MY, Alvin, kanon), both in channel 900.
    pub runs: [String; 2],
}

impl World {
    pub async fn new(actions: Vec<FakeAction>) -> Self {
        Self::with(actions, |_| {}, Arc::new(Passthrough)).await
    }

    pub async fn with(
        actions: Vec<FakeAction>,
        tune: impl FnOnce(&mut PipelineConfig),
        codec: Arc<dyn IdentityCodec>,
    ) -> Self {
        Self::build(actions, tune, codec, true, false).await
    }

    /// Extraction routed to an ungrouped alias.
    pub async fn ungrouped(actions: Vec<FakeAction>) -> Self {
        Self::build(actions, |_| {}, Arc::new(Passthrough), false, false).await
    }

    /// With self-service links wired: the portal at [`PORTAL`], the tracked
    /// Kanade persona for everyone, and rewrites through the same governor.
    pub async fn self_service(
        actions: Vec<FakeAction>,
        tune: impl FnOnce(&mut PipelineConfig),
    ) -> Self {
        Self::build(actions, tune, Arc::new(Passthrough), true, true).await
    }

    async fn build(
        actions: Vec<FakeAction>,
        tune: impl FnOnce(&mut PipelineConfig),
        codec: Arc<dyn IdentityCodec>,
        grouped: bool,
        self_service: bool,
    ) -> Self {
        let store = Arc::new(MemoryScheduleStore::new());
        let clock = TestClock::new(now().fixed_offset());
        let guild = Arc::new(FakeGuild::new());
        let scheduler = Arc::new(Scheduler {
            store: store.clone(),
            clock: clock.clone(),
            ids: Ids::new(0xd4af),
            directory: Roster(guild.roles.clone()),
            refuse: AtomicBool::new(false),
        });
        let week = local(8, 27, 0, 0);
        let mut runs = Vec::new();
        for (datetime, bosses, party) in [
            (
                local(8, 31, 21, 30),
                ["HMaleficStar", "HFA"],
                [MY, ALVIN, PRIYA],
            ),
            (
                local(9, 1, 22, 0),
                ["HCarling", "XKalos"],
                [MY, ALVIN, KANON],
            ),
        ] {
            let id = scheduler
                .service()
                .as_origin(Origin::for_tests())
                .create_run(NewRun {
                    fixed_run_id: None,
                    channel_id: Some(CHANNEL.into()),
                    week_start: week,
                    datetime,
                    bosses: bosses.map(str::to_owned).into(),
                    participants: party.map(str::to_owned).into(),
                    status: RunStatus::Planned,
                    source: RunSource::Amend,
                })
                .await
                .expect("create_run");
            runs.push(id);
        }
        let (provider, client) = client(actions, grouped);
        let outbox = Arc::new(Recorder::default());
        let mut config = config();
        tune(&mut config);
        let self_service = self_service.then(|| SelfServiceDeps {
            links: Arc::new(PublicPortalLinks::new(PORTAL).expect("origin")),
            nudger: Arc::new(Nudger::new(
                Arc::new(Fixed),
                SharedRewriter(Arc::new(GovernedRewriter::new(
                    client.clone(),
                    codec.clone(),
                ))),
            )),
            personas: Arc::new(KanadeForAll(kanade())),
        });
        let extractor = Arc::new(Extractor::new(
            Deps {
                store: store.clone(),
                client,
                codec,
                guild: guild.clone(),
                proposer: scheduler.clone(),
                outbox: outbox.clone(),
                clock: Arc::new(clock),
                ids: Box::new(Ids::new(0x106)),
                self_service,
            },
            config,
        ));
        Self {
            store,
            provider,
            outbox,
            guild,
            scheduler,
            extractor,
            runs: [runs[0].clone(), runs[1].clone()],
        }
    }

    /// The live loop over this world, fed by the returned sender.
    pub fn pipeline(&self) -> (mpsc::Sender<MessageEvent>, JoinHandle<()>) {
        let (sender, events) = mpsc::channel(1_024);
        let pipeline = Pipeline::new(self.extractor.clone());
        (sender, tokio::spawn(pipeline.run(events)))
    }

    /// This world's guild and outbox over a SQLite model-log store, whose
    /// writes really suspend (the memory store never yields).
    pub fn over_sqlite(
        &self,
        store: Arc<SqliteStore>,
    ) -> Arc<Extractor<SqliteStore, FakeProvider, Scheduler, Recorder>> {
        let (_, client) = client(Vec::new(), true);
        Arc::new(Extractor::new(
            Deps {
                store,
                client,
                codec: Arc::new(Passthrough),
                guild: self.guild.clone(),
                proposer: self.scheduler.clone(),
                outbox: self.outbox.clone(),
                clock: Arc::new(TestClock::new(now().fixed_offset())),
                ids: Box::new(Ids::new(0x5a1)),
                self_service: None,
            },
            config(),
        ))
    }

    /// Extraction log rows, oldest first.
    pub async fn logs(&self) -> Vec<ExtractionLog> {
        let filter = ExtractionFilter {
            limit: 200,
            ..ExtractionFilter::default()
        };
        let mut rows = self
            .store
            .list_extractions(&filter)
            .await
            .expect("logs")
            .items;
        rows.reverse();
        rows
    }

    pub async fn live_proposals(&self) -> Vec<StoredProposal> {
        self.store.list_proposals(true).await.expect("proposals")
    }

    pub async fn processed(&self, id: &str) -> bool {
        self.store
            .channel_messages(CHANNEL, local(8, 1, 0, 0), false)
            .await
            .expect("messages")
            .iter()
            .any(|message| message.id == id && message.processed_at.is_some())
    }

    pub fn requests(&self) -> usize {
        self.provider.requests().len()
    }
}

pub fn message(id: &str, author: &str, at: DateTime<Utc>, text: &str) -> IncomingMessage {
    IncomingMessage {
        id: id.into(),
        channel_id: CHANNEL.into(),
        author_id: author.into(),
        author: AuthorKind::Member,
        created_at: at,
        edited_at: None,
        content: text.into(),
        origin: MessageOrigin::Live,
        handled_by_chat: false,
    }
}

pub fn replayed(id: &str, author: &str, at: DateTime<Utc>, text: &str) -> IncomingMessage {
    IncomingMessage {
        origin: MessageOrigin::Replay,
        ..message(id, author, at, text)
    }
}

/// Let paused tokio time run `seconds` forward, tasks included.
pub async fn after(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}
