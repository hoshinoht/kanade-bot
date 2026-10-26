//! Weekly timings, last week's, this week's and next week's runs, their
//! answers and reminder cards (mock `seed::fixed_runs`, `seed::runs`,
//! `member_runs::past_runs`, `Store::cards`), committed as the mock's
//! History tells it (`history::seed_history`): the week as it stood, then
//! each change by the actor and surface that made it, ending in a rollback.

use chrono::{DateTime, Duration, Utc, Weekday};
use kanade_domain::{
    attendance::AttendanceDefault,
    history::{Actor, Surface},
    schedule::{
        Change, DAY_OF, FixedRun, Reminder, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
        countdown_kind,
    },
};

use super::{Seed, hhmm, ids::*, minute_of};
use crate::{Fail, fakes::discord};

struct Timing {
    id: &'static str,
    weekday: Weekday,
    time: &'static str,
    bosses: &'static [&'static str],
    party: &'static [&'static str],
    channel: &'static str,
    owner: &'static str,
}

const TIMINGS: [Timing; 8] = [
    Timing {
        id: "f-baldrix",
        weekday: Weekday::Thu,
        time: "21:30",
        bosses: &["NBaldrix"],
        party: &[ASAHI, REN, MIKA, YUZU],
        channel: BALDRIX_CREW,
        owner: ASAHI,
    },
    // Runs at 22:00 this week: an amended run.
    Timing {
        id: "f-kalos",
        weekday: Weekday::Fri,
        time: "21:30",
        bosses: &["XKalos"],
        party: &[ASAHI, REN, TSUBAME, HINATA],
        channel: KALOS_FOUR,
        owner: REN,
    },
    Timing {
        id: "f-limbo",
        weekday: Weekday::Fri,
        time: "23:30",
        bosses: &["HLimbo"],
        party: &[MIKA, NAGI, SORA],
        channel: LIMBO_TRIO,
        owner: MIKA,
    },
    Timing {
        id: "f-fa",
        weekday: Weekday::Mon,
        time: "20:00",
        bosses: &["HFA"],
        party: &[KAITO, YUZU, RIN, HOTARU],
        channel: FA_NIGHT,
        owner: KAITO,
    },
    Timing {
        id: "f-jupiter",
        weekday: Weekday::Mon,
        time: "21:00",
        bosses: &["HJupiter"],
        party: &[MINATO, ASAHI, SORA],
        channel: JUPITER_TRIO,
        owner: MINATO,
    },
    Timing {
        id: "f-carling",
        weekday: Weekday::Tue,
        time: "22:00",
        bosses: &["HCarling", "HMaleficStar"],
        party: &[ASAHI, REN, MIKA, YUZU, TSUBAME, HOTARU],
        channel: HSTAR_PARTY,
        owner: ASAHI,
    },
    Timing {
        id: "f-bm",
        weekday: Weekday::Tue,
        time: "23:30",
        bosses: &["XBM"],
        party: &[MINATO, KAITO, SORA],
        channel: BM_TRIO,
        owner: MINATO,
    },
    Timing {
        id: "f-seren",
        weekday: Weekday::Wed,
        time: "21:00",
        bosses: &["HSeren"],
        party: &[RIN, NAGI, HINATA],
        channel: SEREN_TRIO,
        owner: RIN,
    },
];

/// An answer; `None` is still waiting.
type Answer = Option<RsvpState>;
const W: Answer = None;
const Y: Answer = Some(RsvpState::Yes);
const N: Answer = Some(RsvpState::No);
const M: Answer = Some(RsvpState::Maybe);

struct Plan {
    id: &'static str,
    /// -1 last week, 0 this week, 1 next week.
    week: i64,
    day: i64,
    /// `None`: own time (`otot`).
    time: Option<&'static str>,
    status: RunStatus,
    bosses: &'static [&'static str],
    party: &'static [(&'static str, Answer)],
    channel: &'static str,
    timing: Option<&'static str>,
}

/// Every run as the week stood before its first recorded change.
const RUNS: [Plan; 14] = [
    Plan {
        id: "p-kalos",
        week: -1,
        day: 1,
        time: Some("22:00"),
        status: RunStatus::Done,
        bosses: &["XKalos"],
        party: &[(ASAHI, Y), (REN, Y), (TSUBAME, Y), (HINATA, Y)],
        channel: KALOS_FOUR,
        timing: Some("f-kalos"),
    },
    Plan {
        id: "r-baldrix",
        week: 0,
        day: 0,
        time: Some("21:30"),
        status: RunStatus::Confirmed,
        bosses: &["NBaldrix"],
        party: &[(ASAHI, Y), (REN, Y), (MIKA, Y), (YUZU, Y)],
        channel: BALDRIX_CREW,
        timing: Some("f-baldrix"),
    },
    Plan {
        id: "r-kalos",
        week: 0,
        day: 1,
        time: Some("21:30"),
        status: RunStatus::Planned,
        bosses: &["XKalos"],
        party: &[(ASAHI, Y), (REN, Y), (TSUBAME, W), (HINATA, Y)],
        channel: KALOS_FOUR,
        timing: Some("f-kalos"),
    },
    Plan {
        id: "r-limbo",
        week: 0,
        day: 1,
        time: Some("23:30"),
        status: RunStatus::Planned,
        bosses: &["HLimbo"],
        party: &[(MIKA, Y), (NAGI, Y), (SORA, W)],
        channel: LIMBO_TRIO,
        timing: Some("f-limbo"),
    },
    // Star again this boss week, from chat: Ren's RenShade is on both.
    Plan {
        id: "r-nstar",
        week: 0,
        day: 2,
        time: Some("21:00"),
        status: RunStatus::Done,
        bosses: &["NMaleficStar"],
        party: &[(REN, Y), (MINATO, Y)],
        channel: HSTAR_PARTY,
        timing: None,
    },
    Plan {
        id: "r-fa",
        week: 0,
        day: 4,
        time: Some("20:00"),
        status: RunStatus::Planned,
        bosses: &["HFA"],
        party: &[(KAITO, Y), (YUZU, Y), (RIN, Y), (HOTARU, W)],
        channel: FA_NIGHT,
        timing: Some("f-fa"),
    },
    Plan {
        id: "r-jupiter",
        week: 0,
        day: 4,
        time: Some("21:00"),
        status: RunStatus::Confirmed,
        bosses: &["HJupiter"],
        party: &[(MINATO, Y), (ASAHI, Y), (SORA, Y)],
        channel: JUPITER_TRIO,
        timing: Some("f-jupiter"),
    },
    // Added from chat, not from a weekly timing.
    Plan {
        id: "r-bellona",
        week: 0,
        day: 4,
        time: None,
        status: RunStatus::Otot,
        bosses: &["NBellona"],
        party: &[(NAGI, Y), (HINATA, Y), (RIN, M), (KAITO, W)],
        channel: BELLONA_OTOT,
        timing: None,
    },
    Plan {
        id: "r-carling",
        week: 0,
        day: 5,
        time: Some("22:00"),
        status: RunStatus::Planned,
        bosses: &["HCarling", "HMaleficStar"],
        party: &[
            (ASAHI, Y),
            (REN, Y),
            (MIKA, Y),
            (YUZU, Y),
            (TSUBAME, W),
            (HOTARU, W),
        ],
        channel: HSTAR_PARTY,
        timing: Some("f-carling"),
    },
    Plan {
        id: "r-bm",
        week: 0,
        day: 5,
        time: Some("23:30"),
        status: RunStatus::Planned,
        bosses: &["XBM"],
        party: &[(MINATO, Y), (KAITO, Y), (SORA, W)],
        channel: BM_TRIO,
        timing: Some("f-bm"),
    },
    Plan {
        id: "r-seren",
        week: 0,
        day: 6,
        time: Some("21:00"),
        status: RunStatus::Planned,
        bosses: &["HSeren"],
        party: &[(RIN, N), (NAGI, N), (HINATA, W)],
        channel: SEREN_TRIO,
        timing: Some("f-seren"),
    },
    Plan {
        id: "n-kalos",
        week: 1,
        day: 1,
        time: Some("21:30"),
        status: RunStatus::Planned,
        bosses: &["XKalos"],
        party: &[(ASAHI, W), (REN, W), (TSUBAME, W), (HINATA, W)],
        channel: KALOS_FOUR,
        timing: Some("f-kalos"),
    },
    Plan {
        id: "n-fa",
        week: 1,
        day: 4,
        time: Some("20:00"),
        status: RunStatus::Planned,
        bosses: &["HFA"],
        party: &[(KAITO, W), (YUZU, W), (RIN, W), (HOTARU, W)],
        channel: FA_NIGHT,
        timing: Some("f-fa"),
    },
    Plan {
        id: "n-carling",
        week: 1,
        day: 5,
        time: Some("22:00"),
        status: RunStatus::Planned,
        bosses: &["HCarling", "HMaleficStar"],
        party: &[
            (ASAHI, W),
            (REN, W),
            (MIKA, W),
            (YUZU, W),
            (TSUBAME, W),
            (HOTARU, W),
        ],
        channel: HSTAR_PARTY,
        timing: Some("f-carling"),
    },
];

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|id| (*id).to_owned()).collect()
}

impl Seed<'_> {
    fn run(&self, plan: &Plan, time: Option<&str>) -> Result<Run, Fail> {
        Ok(Run {
            id: plan.id.into(),
            fixed_run_id: plan.timing.map(str::to_owned),
            channel_id: Some(plan.channel.into()),
            week_start: self.week + Duration::weeks(plan.week),
            datetime: self.at(plan.week, plan.day, time)?,
            bosses: ids(plan.bosses),
            participants: plan.party.iter().map(|(id, _)| (*id).to_owned()).collect(),
            status: plan.status,
            source: if plan.timing.is_some() {
                RunSource::Fixed
            } else {
                RunSource::Amend
            },
            attendance: Vec::new(),
            status_pin: None,
        })
    }

    /// The mock's cards (`Store::cards`): the 09:00 morning card, and for a
    /// timed run its T-1h and T-15m countdowns. All go in unsent:
    /// `notify::seed` posts the due ones through the delivery journal, as
    /// the tick would have. Baldrix's T-1h was retired without posting.
    /// Seren, cancelled below, has none.
    fn cards(&self, plan: &Plan, time: Option<&str>) -> Result<Vec<Reminder>, Fail> {
        if plan.id == "r-seren" {
            return Ok(Vec::new());
        }
        let midnight = self.at(plan.week, plan.day, None)?;
        let mut cards = vec![(DAY_OF.to_owned(), "morning", 9 * 60)];
        if let (Some(time), false) = (time, plan.status == RunStatus::Otot) {
            let start = minute_of(time)?;
            cards.push((countdown_kind(60), "T-1h", start - 60));
            cards.push((countdown_kind(15), "T-15m", start - 15));
        }
        Ok(cards
            .into_iter()
            .map(|(kind, label, minute)| {
                let fire_at = midnight + Duration::minutes(minute);
                let retired = plan.id == "r-baldrix" && label == "T-1h" && fire_at <= self.now;
                Reminder {
                    id: format!("{}-{label}", plan.id),
                    run_id: plan.id.into(),
                    kind,
                    fire_at,
                    sent_at: retired.then_some(fire_at),
                    message_id: None,
                }
            })
            .collect())
    }

    fn rsvp(&self, run: &str, user: &str, state: RsvpState, at: DateTime<Utc>) -> Change {
        Change::PutRsvp(Rsvp {
            run_id: run.into(),
            user_id: user.into(),
            state,
            source: RsvpSource::Reaction,
            at,
        })
    }

    /// A seeded run with `edit` applied, as a put.
    fn edited(&self, id: &str, edit: impl FnOnce(&mut Run)) -> Result<Change, Fail> {
        let plan = RUNS
            .iter()
            .find(|plan| plan.id == id)
            .ok_or("no such seeded run")?;
        let mut run = self.run(plan, plan.time)?;
        edit(&mut run);
        Ok(Change::PutRun(run))
    }
}

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    let hour = |hours: i64| seed.week + Duration::hours(hours);
    // The week as it stood: imported a day before the reset.
    let imported = hour(-24);
    let mut changes = Vec::new();
    for timing in &TIMINGS {
        changes.push(Change::PutFixedRun(FixedRun {
            id: timing.id.into(),
            owner_id: timing.owner.into(),
            channel_id: Some(timing.channel.into()),
            bosses: ids(timing.bosses),
            weekday: timing.weekday,
            time: hhmm(timing.time)?,
            participants: ids(timing.party),
            note: None,
            attendance_default: AttendanceDefault::default(),
            standing: Vec::new(),
            owner_pinned: timing.party.first() != Some(&timing.owner),
        }));
    }
    // Mikan's own weekly timing with Yuzu (member.spec's Weekly timings).
    let mikan = discord::user("eligible").id;
    changes.push(Change::PutFixedRun(FixedRun {
        id: "f-mikan".into(),
        owner_id: mikan.clone(),
        channel_id: None,
        bosses: vec!["HSeren".into()],
        weekday: Weekday::Wed,
        time: hhmm("21:00")?,
        participants: vec![mikan, discord::user("partner").id],
        note: None,
        attendance_default: AttendanceDefault::default(),
        standing: Vec::new(),
        owner_pinned: false,
    }));
    for plan in &RUNS {
        changes.push(Change::PutRun(seed.run(plan, plan.time)?));
        // r-kalos's cards follow the 22:00 it moves to below.
        let time = if plan.id == "r-kalos" {
            Some("22:00")
        } else {
            plan.time
        };
        changes.extend(seed.cards(plan, time)?.into_iter().map(Change::PutReminder));
        for (user, answer) in plan.party {
            if let Some(state) = answer {
                changes.push(seed.rsvp(plan.id, user, *state, imported));
            }
        }
    }
    seed.commit(
        Actor::system("store"),
        Surface::Import,
        imported,
        changes,
        Vec::new(),
    )
    .await?;

    // Proposals and requests closed before the week's first change.
    super::inbox::past(seed).await?;

    let token = || Actor::admin("token");
    let member = Actor::member;
    seed.commit(
        Actor::system("delivery"),
        Surface::DeliveryTick,
        hour(22),
        vec![seed.edited("r-baldrix", |run| run.status = RunStatus::Done)?],
        Vec::new(),
    )
    .await?;
    seed.commit(
        member(TSUBAME),
        Surface::Discord,
        hour(36),
        vec![
            seed.edited("r-kalos", |run| run.status = RunStatus::AtRisk)?,
            seed.rsvp("r-kalos", TSUBAME, RsvpState::No, hour(36)),
        ],
        Vec::new(),
    )
    .await?;
    // The extractor proposed Kalos at 22:00; Asahi (staff, signed in with
    // Discord) approved it from the Inbox.
    super::inbox::kalos_move(seed).await?;
    seed.commit(
        token(),
        Surface::AdminPortal,
        hour(58),
        vec![seed.edited("r-carling", |run| {
            run.participants.push(REN2.into());
            run.source = RunSource::Amend;
        })?],
        Vec::new(),
    )
    .await?;
    for (at, user) in [(61, HOTARU), (62, REN2)] {
        seed.commit(
            member(user),
            Surface::Discord,
            hour(at),
            vec![seed.rsvp("r-carling", user, RsvpState::Maybe, hour(at))],
            Vec::new(),
        )
        .await?;
    }
    seed.commit(
        member(RIN),
        Surface::Discord,
        hour(80),
        vec![seed.edited("r-seren", |run| run.status = RunStatus::Cancelled)?],
        Vec::new(),
    )
    .await?;
    // An admin moved FA by mistake and rolled it back.
    let fa_moved = seed.at(0, 5, Some("20:00"))?;
    let mistake = seed
        .commit(
            token(),
            Surface::AdminPortal,
            hour(90),
            vec![seed.edited("r-fa", |run| {
                run.datetime = fa_moved;
                run.source = RunSource::Amend;
            })?],
            Vec::new(),
        )
        .await?;
    seed.commit(
        token(),
        Surface::Rollback,
        hour(90) + Duration::minutes(5),
        vec![seed.edited("r-fa", |run| run.source = RunSource::Amend)?],
        vec![mistake],
    )
    .await?;
    Ok(())
}
