//! Weekly-timing ownership requests on the tick: posted once in the timing's
//! channel (pinging the owner, with buttons), never twice after an ambiguous
//! send, edited to the decision without buttons, and expired after 24 hours.

use chrono::{NaiveTime, Weekday};
use kanade::bot::transport::{AmbiguousKind, Call, Op, Step};
use kanade::domain::history::Origin;
use kanade::domain::ids::RandomIds;
use kanade::domain::ownership::{OwnerRequest, OwnerRequestStatus, OwnerRequestStore};
use kanade::domain::schedule::NewFixedRun;
use twilight_model::channel::message::Component;

use crate::scenarios::{HOME, delivery, now, world};
use crate::support::{self, Store, on_both_stores};

/// A timing in `HOME` with party 1001 (its owner) and 1002.
async fn timing<S: Store>(store: &S) -> String {
    let mut ids = RandomIds;
    support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .add_fixed_run(NewFixedRun {
            owner_id: "1001".into(),
            channel_id: Some(HOME.into()),
            bosses: vec!["Kalos".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
            owner_pinned: false,
        })
        .await
        .expect("timing")
}

/// 1002's open request `req-1` for `fixed_id`, made `hours_ago`.
async fn ask<S: OwnerRequestStore>(store: &S, fixed_id: String, hours_ago: i64) -> OwnerRequest {
    let request = OwnerRequest::open(
        "req-1".into(),
        fixed_id,
        "1002".into(),
        Some(HOME.into()),
        now() - chrono::Duration::hours(hours_ago),
    );
    store
        .create_owner_request(request.clone())
        .await
        .expect("request");
    request
}

async fn requested<S: Store + OwnerRequestStore>(store: &S, hours_ago: i64) -> OwnerRequest {
    let fixed_id = timing(store).await;
    ask(store, fixed_id, hours_ago).await
}

/// Request posts only (adding the timing posts its own notice).
fn creates(calls: &[Call]) -> Vec<&Call> {
    calls
        .iter()
        .filter(|call| {
            matches!(call, Call::Create { message, .. }
                if message.content.as_deref().is_some_and(|text| text.contains("asks to own")))
        })
        .collect()
}

fn button_ids(components: &[Component]) -> Vec<String> {
    let mut ids = Vec::new();
    for component in components {
        if let Component::ActionRow(row) = component {
            for button in &row.components {
                if let Component::Button(button) = button {
                    ids.extend(button.custom_id.clone());
                }
            }
        }
    }
    ids
}

async fn posted_once_then_settled<S: Store + OwnerRequestStore>(store: &S) {
    let world = world();
    let request = requested(store, 1).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(report.owner_requests.posted, ["req-1"]);
    let calls = world.fake.calls();
    let [
        Call::Create {
            channel, message, ..
        },
    ] = creates(&calls)[..]
    else {
        panic!("one post: {calls:?}");
    };
    assert_eq!(channel.to_string(), HOME);
    let content = message.content.as_deref().expect("text");
    assert!(
        content.starts_with("👑 <@1001>, <@1002> asks to own weekly timing"),
        "{content}"
    );
    assert_eq!(
        message
            .allowed_mentions
            .users
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["1001"],
        "only the owner is pinged"
    );
    assert_eq!(
        button_ids(&message.components),
        ["owner:accept:req-1", "owner:decline:req-1"]
    );
    let stored = store.owner_request("req-1").await.unwrap().unwrap();
    assert!(stored.message_id.is_some());

    let later = now() + chrono::Duration::minutes(1);
    let report = delivery.tick_at(later).await.expect("tick");
    assert!(report.owner_requests.posted.is_empty());
    assert_eq!(creates(&world.fake.calls()).len(), 1, "posted exactly once");

    store
        .close_owner_request(
            &request.id,
            OwnerRequestStatus::Accepted,
            "member:1001",
            later,
        )
        .await
        .unwrap();
    let report = delivery
        .tick_at(later + chrono::Duration::minutes(1))
        .await
        .expect("tick");
    assert_eq!(report.owner_requests.settled, ["req-1"]);
    let calls = world.fake.calls();
    let edits: Vec<_> = calls
        .iter()
        .filter_map(|call| match call {
            Call::Edit { edit, .. } => Some(edit),
            _ => None,
        })
        .collect();
    let [edit] = edits[..] else {
        panic!("one edit: {calls:?}");
    };
    assert!(
        edit.content
            .as_deref()
            .is_some_and(|text| text.starts_with("👑 <@1002> now owns weekly timing")),
        "{edit:?}"
    );
    assert_eq!(edit.components.as_deref(), Some(&[][..]), "buttons removed");
    assert!(
        edit.allowed_mentions.users.is_empty(),
        "the edit pings nobody"
    );
    let report = delivery
        .tick_at(later + chrono::Duration::minutes(2))
        .await
        .expect("tick");
    assert!(report.owner_requests.settled.is_empty(), "settled once");
}

#[tokio::test]
async fn an_ownership_request_is_posted_once_and_edited_to_its_decision() {
    on_both_stores!(posted_once_then_settled);
}

async fn ambiguous_post_is_never_resent<S: Store + OwnerRequestStore>(store: &S) {
    let world = world();
    let mut delivery = delivery(store, &world, &world.fake);
    let fixed_id = timing(store).await;
    // The timing's own notice first, so the scripted step hits the request.
    delivery
        .tick_at(now() - chrono::Duration::minutes(1))
        .await
        .expect("tick");
    ask(store, fixed_id, 1).await;
    world.fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    for minutes in [0, 1, 2] {
        let report = delivery
            .tick_at(now() + chrono::Duration::minutes(minutes))
            .await
            .expect("tick");
        assert!(report.owner_requests.posted.is_empty());
    }
    assert_eq!(
        creates(&world.fake.calls()).len(),
        1,
        "maybe posted: held, never resent"
    );
}

#[tokio::test]
async fn an_ambiguous_ownership_request_post_is_never_resent() {
    on_both_stores!(ambiguous_post_is_never_resent);
}

async fn expired_after_a_day_unposted<S: Store + OwnerRequestStore>(store: &S) {
    let world = world();
    requested(store, 25).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(report.owner_requests.expired, ["req-1"]);
    assert!(report.owner_requests.posted.is_empty());
    assert!(creates(&world.fake.calls()).is_empty());
    let stored = store.owner_request("req-1").await.unwrap().unwrap();
    assert_eq!(
        (stored.status, stored.decided_by.as_deref()),
        (OwnerRequestStatus::Expired, Some("system:expiry"))
    );
}

#[tokio::test]
async fn an_ownership_request_expires_after_a_day() {
    on_both_stores!(expired_after_a_day_unposted);
}
