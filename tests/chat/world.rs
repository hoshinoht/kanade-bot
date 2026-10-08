//! One clean seeded scheduler per case, as the v4 host seeds its repo: the
//! vector's rows with their ids, its channels, roster and catalog, a pinned
//! clock and the case's uuid sequence.

use std::collections::BTreeMap;

use chrono::Utc;
use chrono_tz::Tz;
use kanade::chat::answer::GuildView;
use kanade::chat::gate::{ChannelDirectory, ChannelInfo, PilotSettings};
use kanade::chat::tools::bundles::ToolOffer;
use kanade::chat::tools::dispatch::{self, Dispatched};
use kanade::chat::tools::propose::Proposer;
use kanade::chat::tools::read::resolve::Heard;
use kanade::chat::tools::read::{GuideError, StrategyGuides, ToolWorld};
use kanade::chat::tools::{ToolContext, ToolOutcome};
use kanade::domain::catalog::{BossReference, BossSpec, BossTable, CatalogSpec, DifficultySpec};
use kanade::domain::members::{Directory, Member};
use kanade::domain::schedule::{
    Change, ChangeSet, FixedRun, ReminderPolicy, Rsvp, RsvpSource, RsvpState, Run, RunSource,
    RunStatus, SchedulePolicy, ScheduleSnapshot,
};
use kanade::domain::scheduler::{Clock, ScheduleStore, Scope};
use kanade::infrastructure::llm::identity::PassthroughSession;
use kanade::infrastructure::store::MemoryScheduleStore;
use kanade::infrastructure::store::conformance::meta;
use serde_json::Value;

use crate::common::{self, SeqIds, TestClock, clock_time, strings, text, utc, weekday, zone};

/// The v4 host's `Settings` defaults the chat vectors leave implicit.
const PING_TIME: &str = "01:00";
const COUNTDOWNS: [u32; 1] = [60];

fn opt(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// A comma list of ids, as v4 `_int_list` reads a setting.
fn id_list(value: &Value) -> Vec<String> {
    value
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

pub struct Channels(BTreeMap<String, ChannelInfo>);

impl Channels {
    pub fn new(raw: &Value) -> Self {
        Self(
            raw.as_array()
                .expect("channels")
                .iter()
                .map(|spec| {
                    let info = ChannelInfo {
                        id: text(&spec["id"]).to_owned(),
                        name: opt(&spec["name"]),
                        category_id: opt(&spec["category_id"]),
                        parent_id: opt(&spec["parent_id"]),
                    };
                    (info.id.clone(), info)
                })
                .collect(),
        )
    }
}

impl ChannelDirectory for Channels {
    fn channel(&self, id: &str) -> Option<ChannelInfo> {
        self.0.get(id).cloned()
    }
}

pub fn pilot(settings: &Value) -> PilotSettings {
    PilotSettings {
        guild_id: text(&settings["guild_id"]).to_owned(),
        channel_ids: id_list(&settings["chat_pilot_channel_ids"]),
        category_ids: id_list(&settings["chat_pilot_category_ids"]),
        role_id: opt(&settings["chat_pilot_role_id"]),
    }
}

/// The roster plus the extractor's watch list (v4 `channel_is_watched`).
pub struct Guild {
    members: Vec<Member>,
    watched: Vec<String>,
}

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        self.members.iter().find(|m| m.user_id == user_id).cloned()
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        self.watched.iter().any(|id| id == channel_id)
    }
}

/// The vectors' stand-in strategy store.
pub struct Guides(Vec<String>);

impl StrategyGuides for Guides {
    fn render(&self, reference: &BossReference) -> Result<String, GuideError> {
        self.0
            .contains(&reference.short)
            .then(|| {
                format!(
                    "<guide {} difficulty={}>",
                    reference.short,
                    reference.difficulty.as_deref().unwrap_or("None")
                )
            })
            .ok_or(GuideError::Missing)
    }
}

/// Rebuild the fixture catalog as v4's `BossTable.from_dict` does.
pub fn catalog(raw: &Value) -> BossTable {
    let difficulties = raw["difficulties"]
        .as_array()
        .expect("difficulties")
        .iter()
        .map(|entry| DifficultySpec {
            prefix: text(&entry["prefix"]).to_owned(),
            label: text(&entry["label"]).to_owned(),
        })
        .collect();
    let bosses = raw["bosses"]
        .as_array()
        .expect("bosses")
        .iter()
        .map(|entry| BossSpec {
            short: text(&entry["short"]).to_owned(),
            full: Some(text(&entry["full"]).to_owned()),
            difficulties: entry.get("difficulties").map(strings),
            aliases: strings(&entry["aliases"]),
            ..BossSpec::default()
        })
        .collect();
    BossTable::from_spec(&CatalogSpec {
        difficulties,
        bosses,
    })
    .expect("fixture catalog is valid")
}

pub struct World {
    pub service: common::Service,
    pub clock: TestClock,
    pub ids: SeqIds,
    pub policy: SchedulePolicy,
    pub zone: Tz,
    pub guild: Guild,
    pub catalog: BossTable,
    pub channels: Channels,
    pub pilot: PilotSettings,
    pub guides: Guides,
    input: Value,
}

impl World {
    pub async fn new(input: &Value) -> Self {
        let (service, clock, ids) = common::service_with_ids(input);
        let zone = zone(input);
        let policy = SchedulePolicy::new(
            ReminderPolicy {
                zone,
                ping_time: clock_time(&Value::from(PING_TIME)),
                countdowns: COUNTDOWNS.to_vec(),
            },
            weekday(&input["reset_weekday"]),
            clock_time(&input["reset_time"]),
        );
        let world = &input["world"];
        let members = world["members"]
            .as_array()
            .expect("members")
            .iter()
            .map(|raw| Member {
                user_id: text(&raw["user_id"]).to_owned(),
                display_name: opt(&raw["display_name"]),
                nickname: opt(&raw["nickname"]),
                has_role: raw["has_role"].as_bool().expect("has_role"),
                ..Member::default()
            })
            .collect();
        let settings = &input["settings"];
        let this = Self {
            guild: Guild {
                members,
                watched: id_list(&settings["watched_channel_ids"]),
            },
            catalog: catalog(&input["catalog"]),
            channels: Channels::new(&input["channels"]),
            pilot: pilot(settings),
            guides: Guides(strings(&input["guides"])),
            service,
            clock,
            ids,
            policy,
            zone,
            input: input.clone(),
        };
        this.seed(world).await;
        this
    }

    async fn seed(&self, world: &Value) {
        let mut changes = Vec::new();
        for raw in world["fixed"].as_array().expect("fixed") {
            changes.push(Change::PutFixedRun(FixedRun {
                id: text(&raw["id"]).to_owned(),
                owner_id: text(&raw["owner_id"]).to_owned(),
                channel_id: opt(&raw["channel_id"]),
                bosses: strings(&raw["bosses"]),
                weekday: weekday(&raw["weekday"]),
                time: clock_time(&raw["time"]),
                participants: strings(&raw["participants"]),
                note: None,
                attendance_default: Default::default(),
                standing: Vec::new(),
            }));
        }
        for raw in world["runs"].as_array().expect("runs") {
            let fixed = opt(&raw["fixed_run_id"]);
            changes.push(Change::PutRun(Run {
                id: text(&raw["id"]).to_owned(),
                source: if fixed.is_some() {
                    RunSource::Fixed
                } else {
                    RunSource::Amend
                },
                fixed_run_id: fixed,
                channel_id: opt(&raw["channel_id"]),
                week_start: utc(&raw["week_start"]),
                datetime: utc(&raw["at"]),
                bosses: strings(&raw["bosses"]),
                participants: strings(&raw["participants"]),
                status: RunStatus::parse(text(&raw["status"])).expect("status"),
                attendance: Vec::new(),
                status_pin: None,
            }));
        }
        for raw in world["rsvps"].as_array().expect("rsvps") {
            changes.push(Change::PutRsvp(Rsvp {
                run_id: text(&raw["run_id"]).to_owned(),
                user_id: text(&raw["user_id"]).to_owned(),
                state: RsvpState::parse(text(&raw["state"])).expect("state"),
                source: RsvpSource::Reaction,
                at: self.clock.now(),
            }));
        }
        self.commit(changes).await;
    }

    async fn commit(&self, changes: Vec<Change>) {
        let store = self.service.store();
        let revision = store.load(&Scope::All).await.expect("load").revision;
        store
            .commit(revision, ChangeSet { changes }, meta())
            .await
            .expect("seed");
    }

    /// Seed one more row directly.
    pub async fn put(&self, change: Change) {
        self.commit(vec![change]).await;
    }

    pub async fn snapshot(&self) -> ScheduleSnapshot {
        common::snapshot(&self.service).await
    }

    /// Run one tool call through the dispatcher in v4's full-set mode.
    pub async fn run_tool(
        &mut self,
        step: &Value,
        session: &mut PassthroughSession,
    ) -> ToolOutcome {
        let ctx = self.context(step);
        let mut offer = ToolOffer::full_set(ctx.read_only);
        self.dispatch(
            &ctx,
            &mut offer,
            session,
            text(&step["tool"]),
            &step["arguments"],
        )
        .await
        .outcome
    }

    /// Dispatch one call with this world's state and scheduler.
    pub async fn dispatch(
        &mut self,
        ctx: &ToolContext,
        offer: &mut ToolOffer,
        session: &mut PassthroughSession,
        name: &str,
        arguments: &Value,
    ) -> Dispatched {
        let snapshot = self.snapshot().await;
        let Self {
            service,
            policy,
            zone,
            guild,
            catalog,
            channels,
            pilot,
            guides,
            ..
        } = self;
        let (guild, policy) = (&*guild, &*policy);
        let world = ToolWorld {
            snapshot: &snapshot,
            members: &guild.members,
            directory: guild,
            catalog: &*catalog,
            channels: &*channels,
            pilot: &*pilot,
            zone: *zone,
            reset_weekday: policy.reset_weekday,
            reset_time: policy.reset_time,
            pending: &[],
            guides: Some(&*guides),
            heard: Heard::default(),
        };
        let mut proposer = Proposer { service, policy };
        dispatch::run(ctx, &world, offer, &mut proposer, session, name, arguments).await
    }

    /// The loop's static guild view alone (the scheduler is the caller's).
    pub fn guild_view(&self) -> GuildView<'_> {
        GuildView {
            members: &self.guild.members,
            directory: &self.guild,
            catalog: &self.catalog,
            channels: &self.channels,
            pilot: &self.pilot,
            zone: self.zone,
            reset_weekday: self.policy.reset_weekday,
            reset_time: self.policy.reset_time,
            guides: Some(&self.guides),
        }
    }

    /// The loop's static guild view and the scheduler it proposes through.
    pub fn question_parts(
        &mut self,
    ) -> (
        GuildView<'_>,
        Proposer<'_, MemoryScheduleStore, SeqIds, TestClock>,
    ) {
        let Self {
            service,
            policy,
            zone,
            guild,
            catalog,
            channels,
            pilot,
            guides,
            ..
        } = self;
        let (guild, policy) = (&*guild, &*policy);
        let view = GuildView {
            members: &guild.members,
            directory: guild,
            catalog: &*catalog,
            channels: &*channels,
            pilot: &*pilot,
            zone: *zone,
            reset_weekday: policy.reset_weekday,
            reset_time: policy.reset_time,
            guides: Some(&*guides),
        };
        (view, Proposer { service, policy })
    }

    pub fn tool_world<'a>(&'a self, snapshot: &'a ScheduleSnapshot) -> ToolWorld<'a> {
        ToolWorld {
            snapshot,
            members: &self.guild.members,
            directory: &self.guild,
            catalog: &self.catalog,
            channels: &self.channels,
            pilot: &self.pilot,
            zone: self.zone,
            reset_weekday: self.policy.reset_weekday,
            reset_time: self.policy.reset_time,
            pending: &[],
            guides: Some(&self.guides),
            heard: Heard::default(),
        }
    }

    /// v4 `host.tool_context`: the step's trusted facts.
    pub fn context(&self, step: &Value) -> ToolContext {
        let flag = |name: &str| step.get(name).and_then(Value::as_bool).unwrap_or(false);
        let mut ctx = ToolContext::new(
            text(&step["author_id"]),
            text(&step["channel_id"]),
            step.get("message_id")
                .and_then(Value::as_str)
                .unwrap_or("990001"),
            self.clock.now().with_timezone(&Utc),
        );
        ctx.is_admin = flag("is_admin");
        ctx.read_only = flag("read_only");
        ctx.bot_user_id = opt(&self.input["bot_user"]["id"]);
        ctx.bot_names = vec![text(&self.input["bot_user"]["name"]).to_owned()];
        ctx.self_role_id = opt(&self.input["self_role_id"]);
        ctx.force_all_channels = flag("force_all_channels");
        ctx.force_channel_scope = flag("force_channel_scope");
        ctx.force_group_schedule = flag("force_group_schedule");
        ctx.self_schedule_requested = flag("self_schedule_requested");
        ctx.upcoming_only = flag("upcoming_only");
        ctx.next_only = flag("next_only");
        ctx
    }
}
