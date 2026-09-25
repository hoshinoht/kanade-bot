//! Reaction events: filtering, RSVP mapping and routing to the scheduler.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use twilight_model::id::{Id, marker::MessageMarker};

use kanade::bot::events::{
    BotEvent, CardIndex, LookupError, ReactionRouter, RouteError, RsvpAnswer, rsvp_reaction,
};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Member, Roster};
use kanade::domain::schedule::{NewRun, RsvpState, RunSource, RunStatus};
use kanade::domain::scheduler::{Clock, SchedulerService};
use kanade::infrastructure::store::MemoryScheduleStore;

use super::support::*;

const YES: &str = "\u{2705}";
const NO: &str = "\u{274c}";

/// Route a gateway event and map it to an RSVP, as the application will.
fn rsvp_from(
    event: twilight_gateway::Event,
    roster: &Roster,
) -> Option<kanade::bot::events::RsvpReaction> {
    match route(scope(), event)? {
        BotEvent::Reaction { reaction, added } => {
            rsvp_reaction(&reaction, added, Some(user(SELF_ID)), roster)
        }
        _ => None,
    }
}

fn alice_member() -> serde_json::Value {
    member_json(
        user_json(ALICE, "alice", None, false),
        None,
        &[BOSSING_ROLE],
    )
}

#[test]
fn yes_and_no_reactions_become_rsvps() {
    let roster = Roster::new();
    let add = rsvp_from(
        reaction_add(reaction(
            Some(GUILD),
            ALICE,
            unicode(YES),
            Some(alice_member()),
        )),
        &roster,
    )
    .expect("an RSVP");
    assert_eq!(add.answer, RsvpAnswer::Yes);
    assert!(add.added);
    assert_eq!(add.message_id, Id::new(MESSAGE));
    assert_eq!(add.user_id, user(ALICE));

    let remove = rsvp_from(
        reaction_remove(reaction(Some(GUILD), ALICE, unicode(NO), None)),
        &roster,
    )
    .expect("an RSVP removal");
    assert_eq!(remove.answer, RsvpAnswer::No);
    assert!(!remove.added);
}

#[test]
fn own_bot_and_other_bots_are_ignored() {
    let roster = Roster::new();
    let own = reaction(Some(GUILD), SELF_ID, unicode(YES), None);
    assert!(rsvp_from(reaction_add(own), &roster).is_none());

    let bot_member = member_json(user_json(BOB, "helper", None, true), None, &[BOSSING_ROLE]);
    let bot = reaction(Some(GUILD), BOB, unicode(YES), Some(bot_member));
    assert!(rsvp_from(reaction_add(bot), &roster).is_none());

    // Removals carry no member; the roster's bot flag decides.
    let mut roster = Roster::new();
    roster.upsert(Member {
        user_id: BOB.to_string(),
        is_bot: true,
        ..Member::default()
    });
    let removal = reaction(Some(GUILD), BOB, unicode(NO), None);
    assert!(rsvp_from(reaction_remove(removal), &roster).is_none());
}

#[test]
fn other_emoji_are_ignored() {
    let roster = Roster::new();
    for emoji in [unicode("👍"), unicode("✔️"), custom_emoji(77, "yes")] {
        let event = reaction_add(reaction(
            Some(GUILD),
            ALICE,
            emoji.clone(),
            Some(alice_member()),
        ));
        assert!(rsvp_from(event, &roster).is_none(), "{emoji}");
    }
}

#[test]
fn other_guilds_and_dms_are_ignored() {
    let other = reaction_add(reaction(Some(OTHER_GUILD), ALICE, unicode(YES), None));
    assert!(route(scope(), other).is_none());
    let dm = reaction_add(reaction(None, ALICE, unicode(YES), None));
    assert!(route(scope(), dm).is_none());
}

struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Default)]
struct Cards {
    runs: BTreeMap<Id<MessageMarker>, Vec<String>>,
    fail: bool,
}

impl CardIndex for Cards {
    async fn runs_for_message(
        &self,
        message: Id<MessageMarker>,
    ) -> Result<Vec<String>, LookupError> {
        if self.fail {
            return Err(LookupError("store offline".into()));
        }
        Ok(self.runs.get(&message).cloned().unwrap_or_default())
    }
}

type Service = SchedulerService<MemoryScheduleStore, RandomIds, FixedClock>;

async fn service_with_run(participants: &[u64]) -> (Service, String) {
    let week = Utc.with_ymd_and_hms(2026, 9, 24, 0, 0, 0).unwrap();
    let mut service = SchedulerService::new(
        MemoryScheduleStore::new(),
        RandomIds,
        FixedClock(Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap()),
    );
    let run_id = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(CHANNEL.to_string()),
            week_start: week,
            datetime: Utc.with_ymd_and_hms(2026, 9, 26, 20, 0, 0).unwrap(),
            bosses: vec!["lucid".into()],
            participants: participants.iter().map(u64::to_string).collect(),
            status: RunStatus::Planned,
            source: RunSource::Fixed,
        })
        .await
        .expect("run created");
    (service, run_id)
}

fn alice_yes() -> kanade::bot::events::RsvpReaction {
    rsvp_from(
        reaction_add(reaction(
            Some(GUILD),
            ALICE,
            unicode(YES),
            Some(alice_member()),
        )),
        &Roster::new(),
    )
    .unwrap()
}

#[tokio::test]
async fn card_reaction_reaches_the_scheduler() {
    let (service, run_id) = service_with_run(&[ALICE]).await;
    let mut cards = Cards::default();
    cards
        .runs
        .insert(Id::new(MESSAGE), vec!["deleted-run".into(), run_id.clone()]);
    let mut router = ReactionRouter::new(cards, service);

    let results = router.route(&alice_yes()).await.expect("routed");
    assert_eq!(results.len(), 1, "the deleted run is skipped");
    assert_eq!(results[0].run_id, run_id);
    assert!(results[0].applied);
    assert_eq!(results[0].state, Some(RsvpState::Yes));
    assert_eq!(results[0].new_status, RunStatus::Confirmed);
}

#[tokio::test]
async fn non_card_messages_do_nothing() {
    let (service, _) = service_with_run(&[ALICE]).await;
    let mut router = ReactionRouter::new(Cards::default(), service);
    assert!(router.route(&alice_yes()).await.unwrap().is_empty());
}

#[tokio::test]
async fn lookup_failures_surface() {
    let (service, _) = service_with_run(&[ALICE]).await;
    let cards = Cards {
        fail: true,
        ..Cards::default()
    };
    let mut router = ReactionRouter::new(cards, service);
    assert_eq!(
        router.route(&alice_yes()).await,
        Err(RouteError::Lookup(LookupError("store offline".into())))
    );
}
