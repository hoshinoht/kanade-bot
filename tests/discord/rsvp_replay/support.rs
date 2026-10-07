use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::bot::cards::{Authority, CardDesk, CardOutbox, CardSettings, DeskDeps};
use kanade::bot::delivery::{AlertRecorder, FixedClock};
use kanade::bot::rsvp_replay::RsvpReplay;
use kanade::bot::transport::{FakeDiscord, MessageId};
use kanade::domain::history::{Actor, ChangeMeta, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Directory, Member};
use kanade::domain::notify::{
    AttemptId, Claim, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent, Lease,
    NotificationIntent, Receipt,
};
use kanade::domain::proposals::Approver;
use kanade::domain::schedule::{
    Change, ChangeSet, NewRun, Reminder, ReminderPolicy, RunSource, RunStatus, SchedulePolicy,
};
use kanade::domain::scheduler::{ScheduleStore, SchedulerService};
use kanade::infrastructure::store::MemoryScheduleStore;
use twilight_model::id::Id;

pub const CHANNEL: u64 = 300;
pub const SELF: u64 = 900;
pub const MEMBER: u64 = 1001;
pub const OTHER: u64 = 1002;

struct TestDirectory;

impl Directory for TestDirectory {
    fn member(&self, _user_id: &str) -> Option<Member> {
        None
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

struct TestAuthority;

impl Authority for TestAuthority {
    fn approver(&self, user_id: &str) -> Approver {
        Approver {
            user_id: user_id.to_owned(),
            has_role: false,
            is_admin: false,
            via_portal: false,
        }
    }
}

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

    pub async fn unbound_card_mapping(&self, run: &str, message: u64) {
        let snapshot = self.snapshot(run).await;
        let at = self.clock.get();
        self.store
            .commit(
                snapshot.revision,
                ChangeSet {
                    changes: vec![Change::PutReminder(Reminder {
                        id: format!("unbound-{message}"),
                        run_id: run.to_owned(),
                        kind: format!("countdown_{message}"),
                        fire_at: at,
                        sent_at: Some(at),
                        message_id: Some(message.to_string()),
                    })],
                },
                ChangeMeta {
                    origin: Origin::new(Actor::admin("test"), Surface::AdminPortal),
                    at,
                    notices: Vec::new(),
                    refs: Vec::new(),
                    request_digest: None,
                    expect: Default::default(),
                    outbox: Vec::new(),
                },
            )
            .await
            .unwrap();
        self.fake.seed_message(Id::new(CHANNEL), Id::new(message));
    }

    pub fn card_outbox(
        &self,
    ) -> CardOutbox<MemoryScheduleStore, FakeDiscord, RandomIds, AlertRecorder> {
        CardOutbox(Arc::new(CardDesk::new(
            DeskDeps {
                store: Arc::clone(&self.store),
                transport: Arc::clone(&self.fake),
                ids: RandomIds,
                clock: Arc::new(FixedClock(self.clock.get())),
                directory: Arc::new(TestDirectory),
                authority: Arc::new(TestAuthority),
                alerts: Arc::clone(&self.alerts),
                decline_retraction: None,
            },
            CardSettings {
                zone: chrono_tz::UTC,
                policy: policy(),
                instance_id: "rsvp-replay-tests".into(),
            },
        )))
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
