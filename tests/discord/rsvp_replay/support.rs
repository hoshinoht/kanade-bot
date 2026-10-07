use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::bot::delivery::{AlertRecorder, FixedClock};
use kanade::bot::rsvp_replay::RsvpReplay;
use kanade::bot::transport::{FakeDiscord, MessageId};
use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::{
    AttemptId, Claim, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent, Lease,
    NotificationIntent, Receipt,
};
use kanade::domain::schedule::{NewRun, ReminderPolicy, RunSource, RunStatus, SchedulePolicy};
use kanade::domain::scheduler::{ScheduleStore, SchedulerService};
use kanade::infrastructure::store::MemoryScheduleStore;
use twilight_model::id::Id;

pub const CHANNEL: u64 = 300;
pub const SELF: u64 = 900;
pub const MEMBER: u64 = 1001;

pub fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0)
        .single()
        .unwrap()
}

pub fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::UTC,
            ping_time: NaiveTime::MIN,
            countdowns: vec![],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

#[derive(Clone)]
pub struct TestClock(pub Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(now())))
    }
    pub fn get(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
    pub fn set(&self, at: DateTime<Utc>) {
        *self.0.lock().unwrap() = at;
    }
    pub fn wall(&self) -> kanade::api::auth::Clock {
        let clock = self.clone();
        Arc::new(move || clock.get())
    }
}

pub struct World {
    pub store: Arc<MemoryScheduleStore>,
    pub fake: Arc<FakeDiscord>,
    pub alerts: Arc<AlertRecorder>,
    pub clock: TestClock,
}

impl World {
    pub fn new() -> Self {
        Self {
            store: Arc::new(MemoryScheduleStore::new()),
            fake: Arc::new(FakeDiscord::new()),
            alerts: Arc::new(AlertRecorder::new()),
            clock: TestClock::new(),
        }
    }

    pub async fn run(&self, users: &[u64], start: DateTime<Utc>, status: RunStatus) -> String {
        SchedulerService::new(
            Arc::clone(&self.store),
            RandomIds,
            FixedClock(self.clock.get()),
        )
        .as_origin(Origin::new(Actor::admin("seed"), Surface::AdminPortal))
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(CHANNEL.to_string()),
            week_start: self.clock.get(),
            datetime: start,
            bosses: vec!["HFA".into()],
            participants: users.iter().map(u64::to_string).collect(),
            status,
            source: RunSource::Amend,
        })
        .await
        .unwrap()
    }

    pub async fn card(&self, run: &str, message: u64) {
        let _ = self.card_attempt(run, message).await;
    }

    pub async fn card_attempt(&self, run: &str, message: u64) -> (Lease, AttemptId) {
        let lease = self
            .store
            .begin_lease(&format!("test-{message}"), "delivery", self.clock.get())
            .await
            .unwrap();
        let intent = NotificationIntent {
            effect: EffectKind::DebugCard,
            effect_context: vec![],
            channel_id: CHANNEL.to_string(),
            targets: vec![DeliveryTarget::DebugCard {
                run_id: run.into(),
                kind: "day_of".into(),
            }],
            mentions: vec![],
            content: IntentContent::Plain,
            warnings: vec![],
        };
        let Claim::Fresh(attempt) = self
            .store
            .claim(&lease, &intent, None, self.clock.get())
            .await
            .unwrap()
        else {
            panic!("claim");
        };
        self.store
            .bind(
                &lease,
                &attempt,
                &Receipt {
                    channel_id: CHANNEL.to_string(),
                    message_id: message.to_string(),
                },
                None,
                self.clock.get(),
            )
            .await
            .unwrap();
        self.fake.seed_message(Id::new(CHANNEL), Id::new(message));
        (lease, attempt)
    }

    pub async fn carded_run(&self) -> (String, MessageId) {
        let run = self
            .run(
                &[MEMBER],
                self.clock.get() + chrono::TimeDelta::days(1),
                RunStatus::Planned,
            )
            .await;
        let message = Id::new(400);
        self.card(&run, message.get()).await;
        (run, message)
    }

    pub fn replay(&self) -> RsvpReplay<MemoryScheduleStore, FakeDiscord, AlertRecorder> {
        RsvpReplay::new(
            Arc::clone(&self.store),
            Arc::clone(&self.fake),
            self.clock.wall(),
            Arc::clone(&self.alerts),
        )
    }

    pub async fn snapshot(&self, run: &str) -> kanade::domain::schedule::ScheduleSnapshot {
        self.store
            .load(&kanade::domain::scheduler::Scope::Run(run.into()))
            .await
            .unwrap()
    }
}
