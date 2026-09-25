//! Replies over Discord's limits: split into follow-ups at line boundaries,
//! a fallback edit when completion is refused, gated autocomplete limits
//! and the `interaction_handled` log fields.

use serde_json::json;
use twilight_model::application::command::{Command, CommandOptionChoiceValue, CommandType};
use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::EmbedField;
use twilight_model::id::Id;

use kanade::bot::commands::{
    AccessPolicy, COMPLETION_FALLBACK, CONTENT_LIMIT, ChoicesFuture, CommandFuture, Dispatcher,
    Disposition, Gate, Invocation, SlashCommand, choice,
};
use kanade::bot::handler::handled_fields;
use kanade::bot::transport::{
    AmbiguousKind, Call, FakeDiscord, InteractionReply, Op, Outcome, RejectionKind, Step,
};

use super::support::*;

/// Answers with a fixed reply; `defer` as given; offers `choices`.
struct Fixed {
    reply: InteractionReply,
    defer: Option<bool>,
}

#[allow(deprecated)]
fn definition(name: &str) -> Command {
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

impl SlashCommand for Fixed {
    fn definition(&self) -> Command {
        definition("fixedreply")
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn defer(&self) -> Option<bool> {
        self.defer
    }

    fn run<'a>(&'a self, _: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move { Ok(self.reply.clone()) })
    }

    fn autocomplete<'a>(&'a self, _: &'a Invocation) -> ChoicesFuture<'a> {
        Box::pin(async {
            let mut choices: Vec<_> = (0..30)
                .map(|index| choice(&format!("{index} {}", "x".repeat(150)), index.to_string()))
                .collect();
            choices.insert(0, choice("too long a value", "v".repeat(101)));
            choices
        })
    }
}

fn dispatcher(reply: InteractionReply, defer: Option<bool>) -> Dispatcher {
    Dispatcher::new(AccessPolicy {
        bossing_role_id: role(BOSSING_ROLE),
        admin_role_id: Some(role(ADMIN_ROLE)),
        debug_user_ids: Vec::new(),
    })
    .register(Fixed { reply, defer })
    .unwrap()
}

async fn handle(dispatcher: &Dispatcher, fake: &FakeDiscord) -> (Disposition, Outcome<()>) {
    let interaction = command_interaction(
        Some(GUILD),
        ALICE,
        &[BOSSING_ROLE],
        0,
        "fixedreply",
        json!([]),
    );
    dispatcher
        .handle(fake, guild(), &interaction, None)
        .await
        .expect("handled")
}

/// Every message sent for the reply, in order, with its outcome.
fn messages(fake: &FakeDiscord) -> Vec<(InteractionReply, Outcome<()>)> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Respond { reply, outcome, .. }
            | Call::CompleteDeferred { reply, outcome, .. }
            | Call::Followup { reply, outcome, .. } => Some((reply, outcome)),
            _ => None,
        })
        .collect()
}

fn lines(count: usize, width: usize) -> String {
    (0..count)
        .map(|index| format!("{index:03} {}", "y".repeat(width)))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn long_deferred_replies_follow_up_at_line_boundaries() {
    let text = lines(30, 150);
    let fake = FakeDiscord::new();
    let (disposition, outcome) = handle(
        &dispatcher(InteractionReply::public(text.clone()), Some(true)),
        &fake,
    )
    .await;
    assert_eq!(
        (disposition, outcome),
        (Disposition::Ran, Outcome::Delivered(()))
    );
    let calls = fake.calls();
    assert!(matches!(
        calls[0],
        Call::Defer {
            ephemeral: true,
            ..
        }
    ));
    assert!(matches!(calls[1], Call::CompleteDeferred { .. }));
    assert!(
        calls[2..]
            .iter()
            .all(|call| matches!(call, Call::Followup { .. }))
    );
    let sent = messages(&fake);
    assert!(sent.len() >= 3);
    for (reply, _) in &sent {
        assert!(reply.content.encode_utf16().count() <= CONTENT_LIMIT);
        // The deferral fixed the visibility.
        assert!(reply.ephemeral);
    }
    let pieces: Vec<&str> = sent
        .iter()
        .map(|(reply, _)| reply.content.as_str())
        .collect();
    assert_eq!(pieces.join("\n"), text, "whole lines, in order");
}

#[tokio::test]
async fn one_over_long_line_is_cut_on_a_char_boundary() {
    let line = "🧪".repeat(1_700);
    let fake = FakeDiscord::new();
    handle(
        &dispatcher(InteractionReply::ephemeral(line.clone()), None),
        &fake,
    )
    .await;
    let sent = messages(&fake);
    assert_eq!(sent.len(), 2);
    assert!(matches!(fake.calls()[0], Call::Respond { .. }));
    assert_eq!(
        sent.iter()
            .map(|(reply, _)| reply.content.as_str())
            .collect::<String>(),
        line
    );
    assert!(
        sent.iter()
            .all(|(reply, _)| reply.content.encode_utf16().count() <= CONTENT_LIMIT)
    );
}

#[tokio::test]
async fn a_failed_follow_up_stops_the_rest() {
    let fake = FakeDiscord::new();
    fake.script(Op::Followup, Step::Reject(RejectionKind::RateLimited));
    let (_, outcome) = handle(
        &dispatcher(InteractionReply::public(lines(40, 150)), Some(false)),
        &fake,
    )
    .await;
    assert_eq!(
        outcome,
        Outcome::DefinitelyRejected(RejectionKind::RateLimited)
    );
    assert_eq!(fake.count(Op::Followup), 1);
}

#[tokio::test]
async fn a_refused_completion_gets_one_fallback_edit() {
    let fake = FakeDiscord::new();
    let rejected = RejectionKind::Http {
        status: 400,
        code: Some(50035),
    };
    fake.script(Op::CompleteDeferred, Step::Reject(rejected.clone()));
    fake.set_default(Op::CompleteDeferred, Some(Step::Reject(rejected.clone())));
    let (disposition, outcome) = handle(
        &dispatcher(InteractionReply::ephemeral("ok"), Some(true)),
        &fake,
    )
    .await;
    assert_eq!(outcome, Outcome::DefinitelyRejected(rejected));
    let sent = messages(&fake);
    assert_eq!(sent.len(), 2, "the reply, then one fallback, never a loop");
    assert_eq!(sent[1].0, InteractionReply::ephemeral(COMPLETION_FALLBACK));
    assert_eq!(fake.count(Op::Followup), 0);
    let fields = handled_fields(&disposition, &outcome);
    assert_eq!(
        fields,
        json!({ "disposition": "Ran", "delivered": false, "rejected": "http_400" })
    );

    // An ambiguous completion may have landed: no fallback over it.
    let fake = FakeDiscord::new();
    fake.script(
        Op::CompleteDeferred,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    handle(
        &dispatcher(InteractionReply::ephemeral("ok"), Some(true)),
        &fake,
    )
    .await;
    assert_eq!(fake.count(Op::CompleteDeferred), 1);
}

#[test]
fn log_fields_name_the_failure_kind_only() {
    let delivered = handled_fields(&Disposition::Ran, &Outcome::Delivered(()));
    assert_eq!(
        delivered,
        json!({ "disposition": "Ran", "delivered": true })
    );
    for (outcome, label) in [
        (
            Outcome::DefinitelyRejected(RejectionKind::RateLimited),
            "rate_limited",
        ),
        (
            Outcome::DefinitelyRejected(RejectionKind::NotSent),
            "not_sent",
        ),
        (
            Outcome::Ambiguous(AmbiguousKind::Timeout),
            "ambiguous_timeout",
        ),
    ] {
        let fields = handled_fields(&Disposition::Failed("secret store detail".into()), &outcome);
        assert_eq!(
            fields,
            json!({ "disposition": "Failed", "delivered": false, "rejected": label })
        );
    }
}

fn wide_embed(fields: usize) -> Embed {
    serde_json::from_value(json!({
        "type": "rich",
        "title": "Boss week of Thu 24 Sep (all)",
        "fields": (0..fields)
            .map(|index| json!({ "name": format!("day {index}"), "value": lines(6, 150), "inline": false }))
            .collect::<Vec<_>>(),
        "footer": { "text": "footer" },
    }))
    .unwrap()
}

#[tokio::test]
async fn embeds_over_the_limits_continue_in_follow_ups() {
    let fake = FakeDiscord::new();
    let reply = InteractionReply::public("").with_embed(wide_embed(30));
    handle(&dispatcher(reply, None), &fake).await;
    let sent = messages(&fake);
    assert!(sent.len() > 1);
    let mut fields: Vec<EmbedField> = Vec::new();
    for (reply, _) in &sent {
        assert!(!reply.ephemeral);
        let [embed] = reply.embeds.as_slice() else {
            panic!("one embed per message");
        };
        assert!(embed.fields.len() <= 25);
        fields.extend(embed.fields.iter().cloned());
    }
    assert_eq!(fields.len(), 30);
    assert_eq!(
        sent[0].0.embeds[0].title.as_deref(),
        Some("Boss week of Thu 24 Sep (all)")
    );
    assert!(sent.last().unwrap().0.embeds[0].footer.is_some());
}

#[tokio::test]
async fn autocomplete_answers_within_discords_limits() {
    let fake = FakeDiscord::new();
    let mut interaction = command_interaction(
        Some(GUILD),
        ALICE,
        &[BOSSING_ROLE],
        0,
        "fixedreply",
        json!([]),
    );
    interaction.kind =
        twilight_model::application::interaction::InteractionType::ApplicationCommandAutocomplete;
    let dispatcher = dispatcher(InteractionReply::ephemeral("ok"), None);
    dispatcher.handle(&fake, guild(), &interaction, None).await;
    let calls = fake.calls();
    let [Call::Autocomplete { choices, .. }] = calls.as_slice() else {
        panic!("one autocomplete answer: {calls:?}");
    };
    assert_eq!(choices.len(), 25);
    for choice in choices {
        assert!(choice.name.chars().count() <= 100);
        let CommandOptionChoiceValue::String(value) = &choice.value else {
            panic!("string");
        };
        assert!(value.chars().count() <= 100, "oversized values are dropped");
    }
}
