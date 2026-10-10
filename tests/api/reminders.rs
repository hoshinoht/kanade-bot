//! Reminders on the server clock and the card preview: the list carries the
//! server's `generated_at` and each row's exact `fire_at`; the preview is the
//! card the delivery tick's own planner and `render` produce, read only and
//! admin only.

use std::collections::BTreeSet;

use chrono::{DateTime, TimeZone, Utc};
use kanade::{
    bot::delivery::{
        cards::{CardContext, CardRecord, ReminderCardStore, redesign::NO_MARKS},
        render,
    },
    domain::{
        attendance::AttendancePolicy,
        history::{Actor, ChangeHistory, ChangeMeta, Origin, Surface},
        members::{MemberStore, Roster},
        notify::{
            Claim, DedupeKey, DeliveryJournal, DeliverySettings, DeliveryTarget, DispatchInput,
            NotificationIntent, Receipt, plan_dispatch,
        },
        schedule::{Change, ChangeSet, Reminder, Run},
        scheduler::{ScheduleStore, Scope},
        settings::MessageStyle,
    },
};
use serde_json::Value;

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, Fixture, PUBLIC_HOST, public, request},
};

const SCHEMA: &str = "reminders.json#/$defs/ReminderPreview";

fn preview_path(id: &str) -> String {
    format!("/api/admin/reminders/{id}/preview")
}

async fn roster(reads: &Reads) -> Roster {
    let mut roster = Roster::new();
    for profile in reads.store.list_members().await.unwrap() {
        roster.upsert(profile.member);
    }
    roster
}

/// What the tick posts for `reminder_id` when it falls due at `at`.
async fn tick_post(
    reads: &Reads,
    reminder_id: &str,
    at: DateTime<Utc>,
    heading: Option<&str>,
) -> (NotificationIntent, kanade::bot::transport::OutgoingMessage) {
    let mut schedule = reads.store.load(&Scope::All).await.unwrap();
    // As at its fire time: not yet posted.
    for row in &mut schedule.reminders {
        if row.id == reminder_id {
            row.sent_at = None;
        }
    }
    let members = roster(reads).await;
    let channels: BTreeSet<String> = ["kalos-four".to_owned(), "star".to_owned()].into();
    let settings = DeliverySettings {
        post_channel_id: None,
        quiet_mode: false,
        attendance: AttendancePolicy::V4_COMPAT,
    };
    let plan = plan_dispatch(&DispatchInput {
        now: at,
        schedule: &schedule,
        members: &members,
        channels: &channels,
        journal: &BTreeSet::<DeliveryTarget>::new(),
        settings,
    });
    let target = DeliveryTarget::Reminder(reminder_id.to_owned());
    let intent = plan
        .sends
        .into_iter()
        .map(|send| send.intent)
        .find(|intent| intent.targets.contains(&target))
        .expect("the tick posts it");
    let ctx = CardContext {
        schedule: &schedule,
        attendance: AttendancePolicy::V4_COMPAT,
        zone: chrono_tz::Asia::Kuala_Lumpur,
        quiet: false,
        members: &members,
        catalog: Some(&reads.catalog()),
        style: MessageStyle::Classic,
        marks: &NO_MARKS,
        v2: None,
    };
    let message = render(&intent, &ctx, heading, None).await;
    (intent, message)
}

fn assert_same(card: &Value, message: &kanade::bot::transport::OutgoingMessage) {
    let embed = &message.embeds[0];
    assert_eq!(card["content"], message.content.clone().unwrap());
    assert_eq!(
        card["color"],
        format!("#{:06x}", embed.color.unwrap()),
        "colour bar"
    );
    assert_eq!(card["description"], serde_json::json!(embed.description));
    let fields: Vec<(String, String)> = embed
        .fields
        .iter()
        .map(|field| (field.name.clone(), field.value.clone()))
        .collect();
    let shown: Vec<(String, String)> = card["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| {
            (
                field["name"].as_str().unwrap().to_owned(),
                field["value"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(shown, fields);
    assert_eq!(
        card["footer"],
        serde_json::json!(embed.footer.as_ref().map(|f| f.text.clone()))
    );
}

#[tokio::test]
async fn reminders_carry_the_server_now_and_exact_fire_times() {
    let reads = Reads::new().await;
    let list = reads
        .read("/api/admin/reminders", "reminders.json#/$defs/Reminders")
        .await;
    assert_eq!(list["generated_at"], "2026-09-29T04:00:00Z");
    let row = list["upcoming"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "m-kalos-15")
        .unwrap();
    assert_eq!(row["fire_at"], "2026-09-29T13:45:00Z");
    assert_eq!(row["at"], "Tue 29 Sep 21:45");
}

#[tokio::test]
async fn a_countdown_preview_is_the_ticks_card_and_writes_nothing() {
    let reads = Reads::new().await;
    let before = (
        reads.store.load(&Scope::All).await.unwrap(),
        reads.store.history_head().await.unwrap(),
    );
    let preview = reads.read(&preview_path("m-kalos-15"), SCHEMA).await;
    assert_eq!(preview["reminder"]["id"], "m-kalos-15");
    assert_eq!(preview["reminder"]["state"], "queued");
    assert_eq!(preview["generated_at"], "2026-09-29T04:00:00Z");
    let at = Utc.with_ymd_and_hms(2026, 9, 29, 13, 45, 0).unwrap();
    let (intent, message) = tick_post(&reads, "m-kalos-15", at, None).await;
    let card = &preview["card"];
    assert_same(card, &message);
    assert!(card["content"].as_str().unwrap().contains("<@1001>"));
    assert_eq!(
        card["heading_final"], false,
        "no record yet: the seed phrase"
    );
    assert_eq!(
        card["thumbnail"],
        Value::Null,
        "no Kalos portrait file: left off"
    );
    assert_eq!(card["image"], Value::Null, "countdowns carry no entry art");

    // Read only: no schedule change, history record or card record.
    assert_eq!(reads.store.load(&Scope::All).await.unwrap(), before.0);
    assert_eq!(reads.store.history_head().await.unwrap(), before.1);
    let key = DedupeKey::native(&intent.targets).unwrap();
    assert_eq!(reads.store.card_record(key.as_str()).await.unwrap(), None);
}

/// The 09:00 KL morning ping of Tue 29 Sep.
fn morning() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 29, 1, 0, 0).unwrap()
}

async fn commit(reads: &Reads, changes: Vec<Change>) {
    let revision = reads.store.load(&Scope::All).await.unwrap().revision;
    reads
        .store
        .commit(
            revision,
            ChangeSet { changes },
            ChangeMeta {
                origin: Origin::new(Actor::admin("test"), Surface::AdminPortal),
                at: morning(),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Default::default(),
                outbox: Vec::new(),
            },
        )
        .await
        .unwrap();
}

fn day_of(id: &str, run: &str, fire: DateTime<Utc>, sent: Option<DateTime<Utc>>) -> Reminder {
    Reminder {
        id: id.into(),
        run_id: run.into(),
        kind: "day_of".into(),
        fire_at: fire,
        sent_at: sent,
        message_id: None,
    }
}

/// A run beside `r-kalos` on the same day, at `at`.
async fn sibling(reads: &Reads, id: &str, at: DateTime<Utc>, bosses: &[&str]) -> Run {
    let schedule = reads.store.load(&Scope::All).await.unwrap();
    let kalos = schedule
        .runs
        .iter()
        .find(|run| run.id == "r-kalos")
        .unwrap();
    Run {
        id: id.into(),
        fixed_run_id: None,
        datetime: at,
        bosses: bosses.iter().map(|b| (*b).to_owned()).collect(),
        ..kalos.clone()
    }
}

/// Post what the tick plans for `reminder_id` at the morning ping, as the
/// tick does: record (heading) first, then claim and bind `message_id`.
async fn post_morning(
    reads: &Reads,
    reminder_id: &str,
    heading: &str,
    message_id: &str,
) -> NotificationIntent {
    let (intent, _) = tick_post(reads, reminder_id, morning(), None).await;
    let key = DedupeKey::native(&intent.targets).unwrap();
    let record = CardRecord {
        kind: "day_of".into(),
        heading: Some(heading.into()),
    };
    reads
        .store
        .save_card_record(key.as_str(), &record, morning())
        .await
        .unwrap();
    let lease = reads
        .store
        .begin_lease("test", "delivery", morning())
        .await
        .unwrap();
    let Claim::Fresh(attempt) = reads
        .store
        .claim(&lease, &intent, None, morning())
        .await
        .unwrap()
    else {
        panic!("a fresh claim");
    };
    reads
        .store
        .bind(
            &lease,
            &attempt,
            &Receipt {
                channel_id: intent.channel_id.clone(),
                message_id: message_id.into(),
            },
            None,
            morning(),
        )
        .await
        .unwrap();
    intent
}

fn field_names(card: &Value) -> Vec<String> {
    card["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_posted_day_of_preview_is_the_refresh_edit_with_its_stored_heading() {
    let reads = Reads::new().await;
    // As before the morning ping: unsent.
    commit(
        &reads,
        vec![Change::PutReminder(day_of(
            "m-kalos-day",
            "r-kalos",
            morning(),
            None,
        ))],
    )
    .await;
    let heading = "Tuesday, and Kalos waits";
    post_morning(&reads, "m-kalos-day", heading, "901").await;
    let (_, message) = tick_post(&reads, "m-kalos-day", morning(), Some(heading)).await;

    let preview = reads.read(&preview_path("m-kalos-day"), SCHEMA).await;
    assert_eq!(preview["reminder"]["state"], "sent");
    assert!(
        preview["reminder"]["url"]
            .as_str()
            .unwrap()
            .ends_with("/901")
    );
    assert_eq!(preview["run_started"], false, "22:00 KL is still ahead");
    let card = &preview["card"];
    assert_same(card, &message);
    assert!(card["content"].as_str().unwrap().contains(heading));
    assert_eq!(card["heading_final"], true);
    assert_eq!(
        card["image"], "/art/entry/Kalos",
        "day-of carries entry art"
    );
}

/// A run added the same day after the ping: its day-of row is saved sent
/// with no message at the same fire time. The posted card stays Kalos's
/// alone with its stored heading; the late row is retired with no card.
#[tokio::test]
async fn a_late_same_day_run_never_joins_the_posted_card() {
    let reads = Reads::new().await;
    commit(
        &reads,
        vec![Change::PutReminder(day_of(
            "m-kalos-day",
            "r-kalos",
            morning(),
            None,
        ))],
    )
    .await;
    let heading = "Kalos before dinner";
    post_morning(&reads, "m-kalos-day", heading, "903").await;
    let late = sibling(
        &reads,
        "r-late",
        Utc.with_ymd_and_hms(2026, 9, 29, 15, 0, 0).unwrap(),
        &["NMaleficStar"],
    )
    .await;
    commit(
        &reads,
        vec![
            Change::PutRun(late),
            Change::PutReminder(day_of(
                "m-late-day",
                "r-late",
                morning(),
                Some(morning() + chrono::TimeDelta::hours(3)),
            )),
        ],
    )
    .await;

    let preview = reads.read(&preview_path("m-kalos-day"), SCHEMA).await;
    let card = &preview["card"];
    assert_eq!(field_names(card).len(), 1, "{card}");
    assert!(field_names(card)[0].contains("XKalos"));
    assert!(card["content"].as_str().unwrap().contains(heading));
    assert_eq!(card["heading_final"], true);

    let late = reads.read(&preview_path("m-late-day"), SCHEMA).await;
    assert_eq!(late["reminder"]["state"], "stale");
    assert_eq!(
        late["card"],
        Value::Null,
        "retired without posting: no card"
    );
}

/// Two runs posted on one morning card; one then moves to another day. The
/// card still names both (its card→run rows), as the refresh edit does, and
/// keeps its stored heading.
#[tokio::test]
async fn a_sibling_moved_after_posting_stays_on_the_posted_card() {
    let reads = Reads::new().await;
    let twin = sibling(
        &reads,
        "r-twin",
        Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap(),
        &["NMaleficStar"],
    )
    .await;
    commit(
        &reads,
        vec![
            Change::PutReminder(day_of("m-kalos-day", "r-kalos", morning(), None)),
            Change::PutRun(twin.clone()),
            Change::PutReminder(day_of("m-twin-day", "r-twin", morning(), None)),
        ],
    )
    .await;
    let heading = "Two for Tuesday";
    let intent = post_morning(&reads, "m-kalos-day", heading, "904").await;
    assert_eq!(intent.targets.len(), 2, "one card for both runs");

    let next_morning = morning() + chrono::TimeDelta::days(1);
    commit(
        &reads,
        vec![
            Change::PutRun(Run {
                datetime: twin.datetime + chrono::TimeDelta::days(1),
                ..twin
            }),
            Change::DeleteReminder("m-twin-day".into()),
            Change::PutReminder(day_of("m-twin-day-2", "r-twin", next_morning, None)),
        ],
    )
    .await;

    let preview = reads.read(&preview_path("m-kalos-day"), SCHEMA).await;
    let card = &preview["card"];
    let names = field_names(card);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.iter().any(|name| name.contains("XKalos")));
    assert!(names.iter().any(|name| name.contains("NMaleficStar")));
    assert!(card["content"].as_str().unwrap().contains(heading));
    assert_eq!(card["heading_final"], true);
}

/// An unsent countdown past its grace (the seeded T-1h moved to two hours
/// ago): the tick retires it without posting.
#[tokio::test]
async fn an_overdue_unsent_countdown_has_no_card() {
    let reads = Reads::new().await;
    commit(
        &reads,
        vec![Change::PutReminder(Reminder {
            id: "m-kalos-60".into(),
            run_id: "r-kalos".into(),
            kind: "countdown_60".into(),
            fire_at: Utc.with_ymd_and_hms(2026, 9, 29, 2, 0, 0).unwrap(),
            sent_at: None,
            message_id: None,
        })],
    )
    .await;
    let preview = reads.read(&preview_path("m-kalos-60"), SCHEMA).await;
    assert_eq!(preview["reminder"]["state"], "stale");
    assert_eq!(preview["card"], Value::Null);
}

/// A posted card is frozen once every run it names has started (refresh's
/// gate): a started run alone is; sharing a card with a run ahead is not.
#[tokio::test]
async fn run_started_follows_every_run_on_a_posted_card() {
    // 10:00 KL, two hours before the pinned now.
    let early_at = Utc.with_ymd_and_hms(2026, 9, 29, 2, 0, 0).unwrap();
    let alone = Reads::new().await;
    let early = sibling(&alone, "r-early", early_at, &["NMaleficStar"]).await;
    commit(
        &alone,
        vec![
            Change::PutRun(early.clone()),
            Change::PutReminder(day_of("m-early-day", "r-early", morning(), None)),
        ],
    )
    .await;
    post_morning(&alone, "m-early-day", "Early start", "905").await;
    let preview = alone.read(&preview_path("m-early-day"), SCHEMA).await;
    assert_eq!(field_names(&preview["card"]).len(), 1);
    assert_eq!(preview["run_started"], true);
    // MaleficStar has a clip; the card mirrors Discord, so it never animates.
    assert!(!preview["card"].to_string().contains("/art/animated/"));

    let shared = Reads::new().await;
    commit(
        &shared,
        vec![
            Change::PutReminder(day_of("m-kalos-day", "r-kalos", morning(), None)),
            Change::PutRun(early),
            Change::PutReminder(day_of("m-early-day", "r-early", morning(), None)),
        ],
    )
    .await;
    post_morning(&shared, "m-early-day", "Early and late", "906").await;
    let preview = shared.read(&preview_path("m-early-day"), SCHEMA).await;
    assert_eq!(field_names(&preview["card"]).len(), 2);
    assert_eq!(
        preview["run_started"], false,
        "Kalos (22:00 KL) is still ahead"
    );
}

#[tokio::test]
async fn the_preview_is_admin_only() {
    let reads = Reads::new().await;
    let path = preview_path("m-kalos-15");
    let anonymous = request(reads.admin, "GET", ADMIN_HOST, &path, &[]).await;
    assert_eq!(anonymous.status, 401);
    assert_valid("error.json#/$defs/ApiError", &path, &anonymous.json());
    let unknown = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        &preview_path("nope"),
        &[("Cookie", &reads.cookie)],
    )
    .await;
    assert_eq!(unknown.status, 404);

    let fixture = Fixture::new();
    let address = public(&fixture.http()).await;
    let reply = request(address, "GET", PUBLIC_HOST, &path, &[]).await;
    assert_eq!(reply.status, 404);
}
