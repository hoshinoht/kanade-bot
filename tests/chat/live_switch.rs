//! Live model switches reach a question through one route: the route read
//! for the question gives the alias and level, so a live switch cannot move
//! an in-flight session to another model.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use kanade::chat::answer::{AnswerDeps, Generation, Question, answer};
use kanade::chat::tools::bundles::ToolOffer;
use kanade::infrastructure::llm::governor::{
    Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, Role, RoleConfig,
    RoleRoute, RouteTarget, XorShift,
};
use kanade::infrastructure::llm::{
    ChatRequest, Effort, ExecutionLimits, FakeAction, FakeProvider, Message, RetryPolicy,
};

use crate::looping::{Ports, said, settings};
use crate::model::{Scripted, capabilities};
use crate::support::load;
use crate::wire::kanade;
use crate::world::World;

const HOME: &str = "home-chat";
const CLOUD: &str = "cloud-chat";

fn group(name: &str, alias: &str) -> GroupConfig {
    GroupConfig {
        name: name.into(),
        backend: format!("{name} backend"),
        permits: 1,
        requests_per_min: 6_000,
        burst: Some(1_000),
        aliases: vec![alias.into()],
    }
}

fn governor() -> Arc<Governor> {
    let config = GovernorConfig {
        groups: vec![group("home", HOME), group("cloud", CLOUD)],
        roles: BTreeMap::from([(
            Role::Chat,
            RoleConfig {
                alias: HOME.into(),
                external: false,
            },
        )]),
        policy: GovernorPolicy::default(),
    };
    Arc::new(Governor::new(&config, Arc::new(XorShift::new(3))).unwrap())
}

async fn ask(governor: Arc<Governor>, route: Option<&RoleRoute>) -> (Generation, Vec<ChatRequest>) {
    let input = load("loop.json")["cases"][0]["input"].clone();
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new([
            FakeAction::Response(said(HOME, "Tonight at nine.")),
            FakeAction::Response(said(CLOUD, "Tonight at nine.")),
        ]),
        caps: capabilities(&input["caps"]),
    });
    let client = ModelClient::new(
        governor,
        provider.clone(),
        ExecutionLimits::default(),
        RetryPolicy {
            total_deadline: Duration::from_secs(60),
            max_attempts: 3,
            backoff: Duration::from_millis(100),
        },
    )
    .unwrap();
    let ctx = world.context(&serde_json::json!({"author_id": "11", "channel_id": "900"}));
    let deps = AnswerDeps {
        client: &client,
        route,
    };
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation: vec![
                Message::System {
                    content: "You are Kanade.".into(),
                },
                Message::User {
                    content: "when is hstar?".into(),
                },
            ],
            reminder: kanade().voice_reminder(),
            offer: ToolOffer::full_set(false),
            settings: settings(&input, 8),
        };
        answer(&deps, question, &guild, &mut proposer, &Ports::default()).await
    };
    (generation, provider.fake.requests())
}

#[tokio::test(start_paused = true)]
async fn the_routes_live_level_is_sent_and_recorded_per_round() {
    let governor = governor();
    // What the model setup pushes after a saved switch.
    governor.set_effort(Role::Chat, Some(Effort::High));
    let (generation, requests) = ask(governor, None).await;
    assert_eq!(generation.failure, None, "{generation:?}");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, HOME);
    assert_eq!(requests[0].reasoning, Some(Effort::High));
    let sent = generation.model_rounds[0].sent.clone().unwrap();
    assert_eq!(
        (sent.alias.as_str(), sent.effort),
        (HOME, Some(Effort::High))
    );
}

#[tokio::test(start_paused = true)]
async fn a_save_between_prepare_and_answer_leaves_the_prepared_route_in_use() {
    let governor = governor();
    governor.set_effort(Role::Chat, Some(Effort::Low));
    let prepared = governor.route(Role::Chat).unwrap();
    // Saved after the question was prepared (prompt and log facts built).
    governor
        .reroute(
            [(
                Role::Chat,
                Some(RouteTarget {
                    alias: CLOUD.into(),
                    effort: Some(Effort::High),
                    external: Some(true),
                }),
            )],
            None,
        )
        .unwrap();
    let (generation, requests) = ask(governor.clone(), Some(&prepared)).await;
    assert_eq!(generation.failure, None, "{generation:?}");
    assert_eq!(requests[0].model, HOME);
    assert_eq!(requests[0].reasoning, Some(Effort::Low));
    let sent = generation.model_rounds[0].sent.clone().unwrap();
    assert_eq!(
        (sent.alias.as_str(), sent.effort),
        (HOME, Some(Effort::Low))
    );
    // The next question reads the saved route.
    let (generation, requests) = ask(governor, None).await;
    assert_eq!(requests[0].model, CLOUD);
    assert_eq!(requests[0].reasoning, Some(Effort::High));
    assert!(generation.external_unmasked);
    assert_eq!(generation.route(), "external_unmasked");
}
