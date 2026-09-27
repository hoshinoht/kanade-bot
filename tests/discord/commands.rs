//! Command registration, authorisation and ephemeral replies, end to end
//! against the fake transport.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use twilight_gateway::Event;
use twilight_model::application::command::{Command, CommandType};
use twilight_model::application::interaction::{
    Interaction, InteractionContextType, InteractionData,
};
use twilight_model::gateway::payload::incoming::InteractionCreate;
use twilight_model::guild::Permissions;
use twilight_model::id::Id;

use kanade::bot::commands::{
    AccessPolicy, CommandError, CommandFuture, Denial, Dispatcher, Disposition, GENERIC_FAILURE,
    Gate, Handled, Invocation, SlashCommand, spawn_interaction,
};
use kanade::bot::events::{AdminRoles, BotEvent, EventHandler};
use kanade::bot::gateway::{
    ConnectionStatus, EventSource, GatewayError, RunExit, RunnerConfig, run,
};
use kanade::bot::handler::Fanout;
use kanade::bot::transport::{
    AmbiguousKind, Call, DiscordTransport, FakeDiscord, InteractionReply, Op, Outcome,
    RejectionKind, Step,
};
use kanade::domain::members::Roster;

use super::support::*;

const ADMINISTRATOR: u64 = 1 << 3;

fn policy() -> AccessPolicy {
    AccessPolicy {
        bossing_role_id: role(BOSSING_ROLE),
        admin_role_id: Some(role(ADMIN_ROLE)),
        debug_user_ids: vec![user(BOB)],
    }
}

/// A bossing-role command whose reply depends on its option.
struct Echo;

#[allow(deprecated)]
fn plain_command(name: &str) -> Command {
    Command {
        application_id: None,
        contexts: None,
        default_member_permissions: None,
        dm_permission: None,
        description: "test".into(),
        description_localizations: None,
        guild_id: None,
        id: None,
        integration_types: None,
        kind: CommandType::ChatInput,
        name: name.into(),
        name_localizations: None,
        nsfw: None,
        options: Vec::new(),
        version: Id::new(1),
    }
}

impl SlashCommand for Echo {
    fn definition(&self) -> Command {
        plain_command("echo")
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            let text = invocation
                .options
                .first()
                .map(|option| format!("{:?}", option.value))
                .unwrap_or_default();
            match text.as_str() {
                "String(\"refuse\")" => Err(CommandError::User("not like that".into())),
                "String(\"crash\")" => Err(CommandError::Internal("boom".into())),
                _ => Ok(InteractionReply::public("echoed")),
            }
        })
    }
}

/// A `/debug`-gated, admin-hidden command answering `ok`.
struct Probe;

impl SlashCommand for Probe {
    #[allow(deprecated)]
    fn definition(&self) -> Command {
        Command {
            contexts: Some(vec![InteractionContextType::Guild]),
            default_member_permissions: Some(Permissions::ADMINISTRATOR),
            ..plain_command("debug")
        }
    }

    fn gate(&self) -> Gate {
        Gate::Debug
    }

    fn run<'a>(&'a self, _: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async { Ok(InteractionReply::ephemeral("ok")) })
    }
}

fn dispatcher() -> Dispatcher {
    Dispatcher::new(policy())
        .register(Probe)
        .unwrap()
        .register(Echo)
        .unwrap()
}

fn echo(
    invoker: u64,
    roles: &[u64],
    value: &str,
) -> twilight_model::application::interaction::Interaction {
    command_interaction(
        Some(GUILD),
        invoker,
        roles,
        0,
        "echo",
        json!([{ "name": "text", "type": 3, "value": value }]),
    )
}

async fn reply_for(
    dispatcher: &Dispatcher,
    interaction: &twilight_model::application::interaction::Interaction,
) -> (InteractionReply, Disposition) {
    let invocation = Invocation::from_interaction(interaction).expect("a guild command");
    dispatcher.reply(&invocation, Some(user(OWNER))).await
}

#[test]
fn registration_payload_is_guild_scoped_and_admin_hidden() {
    let definitions = dispatcher().definitions();
    let debug = serde_json::to_value(&definitions[0]).unwrap();
    assert_eq!(debug["name"], json!("debug"));
    assert_eq!(debug["type"], json!(1));
    assert_eq!(debug["default_member_permissions"], json!("8"));
    assert_eq!(debug["contexts"], json!([0]));
    assert!(debug.get("guild_id").is_none());
    assert!(
        Dispatcher::new(policy())
            .register(Echo)
            .unwrap()
            .register(Echo)
            .is_err()
    );
}

#[tokio::test]
async fn registration_goes_through_the_transport() {
    let fake = FakeDiscord::new();
    let definitions = dispatcher().definitions();
    assert_eq!(
        fake.register_guild_commands(guild(), &definitions).await,
        Outcome::Delivered(())
    );
    let calls = fake.calls();
    let [
        Call::Register {
            guild: g, commands, ..
        },
    ] = calls.as_slice()
    else {
        panic!("one registration");
    };
    assert_eq!((*g, commands.len()), (guild(), 2));
}

#[tokio::test]
async fn debug_gate_admits_each_staff_route() {
    let dispatcher = dispatcher();
    let cases = [
        ("administrator permission", ALICE, vec![], ADMINISTRATOR),
        ("admin role", ALICE, vec![ADMIN_ROLE], 0),
        ("guild owner", OWNER, vec![], 0),
        ("debug user", BOB, vec![], 0),
    ];
    for (label, invoker, roles, permissions) in cases {
        let (reply, disposition) = reply_for(
            &dispatcher,
            &debug_status(Some(GUILD), invoker, &roles, permissions),
        )
        .await;
        assert_eq!(disposition, Disposition::Ran, "{label}");
        assert_eq!(reply, InteractionReply::ephemeral("ok"), "{label}");
    }
}

#[tokio::test]
async fn debug_status_denied_to_everyone_else() {
    let (reply, disposition) = reply_for(
        &dispatcher(),
        &debug_status(Some(GUILD), ALICE, &[BOSSING_ROLE], 0),
    )
    .await;
    assert_eq!(disposition, Disposition::Denied(Denial::DebugNotAllowed));
    assert_eq!(
        reply,
        InteractionReply::ephemeral(
            "❌ `/debug` is restricted to the server owner, the admin role, and users listed in `DEBUG_USER_IDS`."
        )
    );
}

#[tokio::test]
async fn bossing_role_gate_has_no_staff_bypass() {
    let dispatcher = dispatcher();
    let (reply, disposition) = reply_for(&dispatcher, &echo(ALICE, &[BOSSING_ROLE], "hi")).await;
    assert_eq!(disposition, Disposition::Ran);
    assert_eq!(reply.content, "echoed");

    for (invoker, roles) in [(ALICE, vec![]), (OWNER, vec![ADMIN_ROLE])] {
        let (reply, disposition) = reply_for(&dispatcher, &echo(invoker, &roles, "hi")).await;
        assert_eq!(disposition, Disposition::Denied(Denial::MissingBossingRole));
        assert_eq!(
            reply,
            InteractionReply::ephemeral("❌ You need the bossing role to use this bot.")
        );
    }
}

#[tokio::test]
async fn command_errors_are_ephemeral() {
    let dispatcher = dispatcher();
    let (reply, disposition) =
        reply_for(&dispatcher, &echo(ALICE, &[BOSSING_ROLE], "refuse")).await;
    assert_eq!(disposition, Disposition::UserError);
    assert_eq!(reply, InteractionReply::ephemeral("❌ not like that"));

    let (reply, disposition) = reply_for(&dispatcher, &echo(ALICE, &[BOSSING_ROLE], "crash")).await;
    assert_eq!(disposition, Disposition::Failed("boom".into()));
    assert_eq!(reply, InteractionReply::ephemeral(GENERIC_FAILURE));

    let unknown = command_interaction(Some(GUILD), ALICE, &[BOSSING_ROLE], 0, "nope", json!([]));
    let (reply, disposition) = reply_for(&dispatcher, &unknown).await;
    assert_eq!(disposition, Disposition::Unknown);
    assert_eq!(reply, InteractionReply::ephemeral(GENERIC_FAILURE));
}

#[test]
fn staff_gate_names_the_command() {
    assert_eq!(
        Denial::NotStaff.message("say"),
        "❌ `/say` is for server admins, the server owner and the admin role."
    );
}

#[tokio::test]
async fn handle_answers_through_the_transport_once() {
    let fake = FakeDiscord::new();
    let interaction = debug_status(Some(GUILD), ALICE, &[], 0);
    let handled = dispatcher()
        .handle(&fake, guild(), &interaction, Some(user(OWNER)))
        .await;
    assert_eq!(
        handled,
        Some((
            Disposition::Denied(Denial::DebugNotAllowed),
            Outcome::Delivered(())
        ))
    );
    let calls = fake.calls();
    let [
        Call::Respond {
            interaction, reply, ..
        },
    ] = calls.as_slice()
    else {
        panic!("one reply");
    };
    assert!(reply.ephemeral);
    assert_eq!(interaction.id, Id::new(7700));
    assert!(!format!("{calls:?}").contains("secret"), "token redacted");

    // A failed reply is reported, not retried.
    fake.script(Op::Respond, Step::Reject(RejectionKind::UnknownChannel));
    let handled = dispatcher()
        .handle(
            &fake,
            guild(),
            &debug_status(Some(GUILD), OWNER, &[], 0),
            None,
        )
        .await;
    assert_eq!(
        handled.map(|(_, outcome)| outcome),
        Some(Outcome::DefinitelyRejected(RejectionKind::UnknownChannel))
    );
    assert_eq!(fake.calls().len(), 2);
}

#[tokio::test]
async fn other_guilds_and_dms_are_not_handled() {
    let fake = FakeDiscord::new();
    for interaction in [
        debug_status(Some(OTHER_GUILD), OWNER, &[], ADMINISTRATOR),
        debug_status(None, OWNER, &[], ADMINISTRATOR),
    ] {
        assert_eq!(
            dispatcher()
                .handle(&fake, guild(), &interaction, None)
                .await,
            None
        );
    }
    assert!(fake.calls().is_empty());
}

/// A deferring command that takes a minute (on the paused clock).
struct Slow;

impl SlashCommand for Slow {
    fn definition(&self) -> Command {
        plain_command("slow")
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn defer(&self) -> Option<bool> {
        Some(true)
    }

    fn run<'a>(&'a self, _: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(InteractionReply::ephemeral("slow done"))
        })
    }
}

fn slow_dispatcher() -> Arc<Dispatcher> {
    Arc::new(Dispatcher::new(policy()).register(Slow).unwrap())
}

fn slow(roles: &[u64]) -> Interaction {
    command_interaction(Some(GUILD), ALICE, roles, 0, "slow", json!([]))
}

struct Fast;

impl SlashCommand for Fast {
    fn definition(&self) -> Command {
        plain_command("fast")
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn run<'a>(&'a self, _: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async { Ok(InteractionReply::ephemeral("fast done")) })
    }
}

fn drain_dispatcher() -> Arc<Dispatcher> {
    Arc::new(
        Dispatcher::new(policy())
            .register(Slow)
            .unwrap()
            .register(Fast)
            .unwrap(),
    )
}

fn registered(mut interaction: Interaction, id: u64) -> Interaction {
    interaction.id = Id::new(id);
    let Some(InteractionData::ApplicationCommand(command)) = &mut interaction.data else {
        panic!("application command fixture");
    };
    command.guild_id = Some(guild());
    interaction
}

#[tokio::test(start_paused = true)]
async fn fanout_bounds_task_drain_and_aborts_hanging_command_tasks() {
    let transport = Arc::new(FakeDiscord::new());
    let dispatcher = drain_dispatcher();
    let (roster, _roster_jobs) = mpsc::unbounded_channel();
    let (reactions, _reaction_jobs) = mpsc::unbounded_channel();
    let (guild_ready, _ready) = watch::channel(false);
    let commands = Arc::clone(&dispatcher);
    let mut fanout = Fanout::new(
        guild(),
        Arc::clone(&transport),
        Box::new(move |_| Arc::clone(&commands)),
        Arc::new(|| None),
        Arc::new(Roster::new()),
        roster,
        reactions,
        ConnectionStatus::new(),
        Box::new(|_| {}),
        guild_ready,
    );

    fanout
        .handle(BotEvent::Ready {
            self_id: user(SELF_ID),
            application_id: Id::new(9),
            name: "kanade".into(),
        })
        .await;
    fanout
        .handle(BotEvent::GuildAvailable {
            owner_id: user(OWNER),
            admin_roles: AdminRoles::default(),
        })
        .await;
    fanout
        .handle(BotEvent::Interaction(Box::new(registered(
            slow(&[BOSSING_ROLE]),
            8_001,
        ))))
        .await;
    fanout
        .handle(BotEvent::Interaction(Box::new(registered(
            command_interaction(Some(GUILD), ALICE, &[BOSSING_ROLE], 0, "fast", json!([])),
            8_002,
        ))))
        .await;

    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(3), fanout.finish())
        .await
        .expect("the shared handler-task grace bounds shutdown");
    assert!(started.elapsed() < Duration::from_secs(3));

    // If cancelling the outer logger task detached its nested command, this
    // advance would let the slow command complete its deferred response.
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    let calls = transport.calls();
    assert!(calls.iter().any(|call| matches!(call,
        Call::Respond { reply, .. } if reply.content == "fast done"
    )));
    assert!(calls.iter().any(|call| matches!(call, Call::Defer { .. })));
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, Call::CompleteDeferred { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn deferring_commands_acknowledge_then_complete() {
    let fake = FakeDiscord::new();
    let handled = slow_dispatcher()
        .handle(&fake, guild(), &slow(&[BOSSING_ROLE]), None)
        .await;
    assert_eq!(handled, Some((Disposition::Ran, Outcome::Delivered(()))));
    let calls = fake.calls();
    let [
        Call::Defer { ephemeral, .. },
        Call::CompleteDeferred { reply, .. },
    ] = calls.as_slice()
    else {
        panic!("defer then complete: {calls:?}");
    };
    assert!(*ephemeral);
    assert_eq!(reply.content, "slow done");
}

#[tokio::test(start_paused = true)]
async fn refused_deferring_commands_answer_directly() {
    let fake = FakeDiscord::new();
    let handled = slow_dispatcher()
        .handle(&fake, guild(), &slow(&[]), None)
        .await;
    assert_eq!(
        handled,
        Some((
            Disposition::Denied(Denial::MissingBossingRole),
            Outcome::Delivered(())
        ))
    );
    assert!(matches!(fake.calls().as_slice(), [Call::Respond { .. }]));
}

#[tokio::test(start_paused = true)]
async fn unacknowledged_deferral_does_not_run_the_command() {
    let fake = FakeDiscord::new();
    fake.script(
        Op::Defer,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    );
    let handled = slow_dispatcher()
        .handle(&fake, guild(), &slow(&[BOSSING_ROLE]), None)
        .await;
    assert_eq!(
        handled,
        Some((
            Disposition::NotAcknowledged,
            Outcome::Ambiguous(AmbiguousKind::Timeout)
        ))
    );
    assert_eq!(fake.calls().len(), 1);
}

/// The handler contract: interaction work is spawned, so a slow command
/// does not hold up the next gateway event.
struct Spawning {
    dispatcher: Arc<Dispatcher>,
    transport: Arc<FakeDiscord>,
    seen: Vec<String>,
    tasks: Vec<JoinHandle<Handled>>,
    stop: Option<oneshot::Sender<()>>,
}

impl EventHandler for Spawning {
    async fn handle(&mut self, event: BotEvent) {
        match event {
            BotEvent::Interaction(interaction) => {
                self.seen.push("interaction".into());
                self.tasks.push(spawn_interaction(
                    Arc::clone(&self.dispatcher),
                    Arc::clone(&self.transport),
                    guild(),
                    interaction,
                    None,
                ));
            }
            other => {
                self.seen.push(format!("{other:?}"));
                if let Some(stop) = self.stop.take() {
                    let _ = stop.send(());
                }
            }
        }
    }
}

struct Script(VecDeque<Event>);

impl EventSource for Script {
    async fn next_event(&mut self) -> Option<Result<Event, GatewayError>> {
        match self.0.pop_front() {
            Some(event) => Some(Ok(event)),
            None => std::future::pending().await,
        }
    }

    fn close(&mut self) {
        self.0.push_back(Event::GatewayClose(None));
    }
}

#[tokio::test(start_paused = true)]
async fn slow_interactions_do_not_block_later_events() {
    let transport = Arc::new(FakeDiscord::new());
    let (stop, stopped) = oneshot::channel();
    let mut handler = Spawning {
        dispatcher: slow_dispatcher(),
        transport: Arc::clone(&transport),
        seen: Vec::new(),
        tasks: Vec::new(),
        stop: Some(stop),
    };
    let mut source = Script(
        vec![
            Event::InteractionCreate(Box::new(InteractionCreate(slow(&[BOSSING_ROLE])))),
            member_remove(GUILD, user_json(BOB, "bob", None, false)),
        ]
        .into(),
    );
    let started = Instant::now();
    let exit = run(
        &mut source,
        &mut handler,
        RunnerConfig {
            scope: scope(),
            drain_timeout: Duration::from_secs(1),
        },
        async {
            let _ = stopped.await;
        },
        |_| {},
    )
    .await;
    assert!(matches!(exit, RunExit::Shutdown { drained: true, .. }));
    assert_eq!(handler.seen.len(), 2, "{:?}", handler.seen);
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "loop was not held up"
    );

    let task = handler.tasks.pop().unwrap();
    assert_eq!(
        task.await.unwrap(),
        Some((Disposition::Ran, Outcome::Delivered(())))
    );
    assert!(matches!(
        transport.calls().as_slice(),
        [Call::Defer { .. }, Call::CompleteDeferred { .. }]
    ));
}

#[test]
fn bot_event_debug_redacts_the_interaction_token() {
    let event = route(
        scope(),
        Event::InteractionCreate(Box::new(InteractionCreate(debug_status(
            Some(GUILD),
            ALICE,
            &[],
            0,
        )))),
    )
    .expect("routed");
    let shown = format!("{event:?}");
    assert!(shown.contains("7700"), "{shown}");
    assert!(!shown.contains("interaction-secret-token"), "{shown}");
}
