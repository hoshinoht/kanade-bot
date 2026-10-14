//! The Inbox (mock `inbox::seed`, `past::seeded`, `requests::seed`,
//! `ownership::seed`, `drafts::seed`): Kanade's proposals with their cards
//! and the channel messages they cite, member requests, their closed
//! predecessors, ownership requests and administrator drafts, each written
//! through the scheduler as the extractor, the chatbot, the member and the
//! deciding admin wrote it, at the mock's times.

use chrono::{DateTime, Duration, Utc, Weekday};
use kanade::domain::{
    drafts::{DraftOp, ProposalSource, Target},
    history::Actor,
    model_log::{ModelLogStore, WatchedMessage},
    notify::{DeliveryTarget, EffectKind, IntentContent, NotificationIntent},
    ownership::{OwnerRequest, OwnerRequestStore},
    proposals::{
        Approver, CardDetails, CardPayload, ChangeKind, ProposalCardStore, ProposedChange,
    },
    requests::{NoFreezes, RequestSpec, Subject},
    schedule::{FixedEdit, NewFixedRun},
    scheduler::{ProposalRequest, Supersede, SupersedeScope},
};

use super::{Seed, hhmm, ids::*};
use crate::Fail;

/// History record 3: the extractor's Kalos move Asahi approved.
pub const KALOS_MOVE: &str = "c-kalos-move";

fn failed(id: &str) -> impl FnOnce(String) -> Fail + '_ {
    move |error| format!("seeding {id}: {error}").into()
}

/// A channel line around a proposal: author, text, hour (as
/// [`Seed::hour`]), and whether the proposal cites it. Empty text is a
/// cited message since deleted.
type Line = (&'static str, &'static str, i64, bool);

struct Proposal {
    id: &'static str,
    source: ProposalSource,
    source_id: &'static str,
    kind: ChangeKind,
    run: Option<&'static str>,
    channel: &'static str,
    bosses: &'static [&'static str],
    participants: &'static [&'static str],
    /// Day of this boss week and `HH:MM` the change names.
    to: Option<(i64, &'static str)>,
    confidence: f64,
    is_question: bool,
    summary: &'static str,
    /// The channel thread, oldest first.
    thread: &'static [Line],
    /// When the extractor (or chatbot) read it, as [`Seed::hour`].
    read_at: i64,
    card_posted: bool,
}

const fn proposal(id: &'static str, kind: ChangeKind, channel: &'static str) -> Proposal {
    Proposal {
        id,
        source: ProposalSource::Extraction,
        source_id: "",
        kind,
        run: None,
        channel,
        bosses: &[],
        participants: &[],
        to: None,
        confidence: 0.8,
        is_question: false,
        summary: "",
        thread: &[],
        read_at: 0,
        card_posted: true,
    }
}

/// The live proposals. The mock's third, `p-jupiter-chat`, asks to move
/// Monday's Jupiter after it ended; the real scheduler refuses that.
const LIVE: [Proposal; 2] = [
    Proposal {
        source_id: "x-bm",
        run: Some("r-bm"),
        bosses: &["XBM"],
        participants: &[MINATO, KAITO, SORA],
        to: Some((6, "23:30")),
        confidence: 0.86,
        summary: "Minato asks to push Black Mage to Wednesday; Kaito agrees.",
        // Sora's cited message was deleted since.
        thread: &[
            (KAITO, "bm still tue this week?", 124, false),
            (SORA, "should be", 124, false),
            (MINATO, "tue cannot, wed same time ok?", 125, true),
            (SORA, "brb dinner", 125, false),
            (KAITO, "wed ok for me", 126, true),
            (SORA, "", 126, true),
            (MINATO, "ty, will ask kanade", 126, false),
        ],
        read_at: 126,
        ..proposal("p-bm-move", ChangeKind::Move, BM_TRIO)
    },
    Proposal {
        source_id: "x-limbo",
        bosses: &["NLimbo"],
        participants: &[MIKA, NAGI],
        to: Some((2, "21:00")),
        confidence: 0.52,
        is_question: true,
        summary: "Mika floats a Normal Limbo run on Saturday; nobody has confirmed.",
        thread: &[
            (NAGI, "finally cleared hlimbo prequest", 122, false),
            (MIKA, "nlimbo sat 9pm anyone?", 123, true),
            (NAGI, "maybe, will check", 124, false),
        ],
        read_at: 124,
        ..proposal("p-limbo-add", ChangeKind::Add, LIMBO_TRIO)
    },
];

/// History record 3's proposal: read at 39 h into the boss week (the past
/// tab's clock, [`Seed::week`]-based; 47 on [`Seed::hour`]'s).
const KALOS: Proposal = Proposal {
    source_id: "x-kalos",
    run: Some("r-kalos"),
    bosses: &["XKalos"],
    to: Some((1, "22:00")),
    confidence: 0.93,
    summary: "Tsubame asks to start Kalos at 22:00",
    thread: &[
        (TSUBAME, "kalos at 10 instead? 9:30 is tight", 46, true),
        (MIKA, "", 46, true),
    ],
    read_at: 47,
    ..proposal(KALOS_MOVE, ChangeKind::Move, KALOS_FOUR)
};

/// Closed proposals for the Past tab (hours on [`Seed::hour`]'s clock).
const PAST: [Proposal; 3] = [
    Proposal {
        source: ProposalSource::Chat,
        source_id: "c-lotus",
        bosses: &["HLotus"],
        participants: &[MIKA],
        to: Some((2, "21:00")),
        summary: "Mika asks the chatbot for a Saturday Lotus run",
        thread: &[(MIKA, "can we add a lotus run sat?", 37, true)],
        read_at: 38,
        ..proposal("c-lotus-add", ChangeKind::Add, HSTAR_PARTY)
    },
    Proposal {
        source_id: "x-limbo-early",
        run: Some("r-limbo"),
        bosses: &["HLimbo"],
        to: Some((1, "21:00")),
        summary: "Hinata suggests Limbo at 21:00",
        thread: &[(HINATA, "limbo 9pm?", 33, true)],
        read_at: 34,
        ..proposal("c-limbo-move", ChangeKind::Move, LIMBO_TRIO)
    },
    Proposal {
        source_id: "x-seren-early",
        run: Some("r-seren"),
        bosses: &["HSeren"],
        summary: "Seren might be off this week",
        read_at: 10,
        card_posted: false,
        ..proposal("c-seren-cancel", ChangeKind::Cancel, SEREN_TRIO)
    },
];

impl Seed<'_> {
    /// Store `lines` as the watched channel's messages; their Discord ids
    /// in order. A deleted one gets an id, never a row.
    async fn channel_lines(&self, channel: &str, lines: &[Line]) -> Result<Vec<String>, Fail> {
        let mut ids = Vec::with_capacity(lines.len());
        for (n, (author, text, hour, _)) in (0_u64..).zip(lines) {
            // A minute apart within the hour, in thread order.
            let at = self.hour(*hour) - Duration::minutes(30) + Duration::minutes(n as i64);
            let id = Seed::snowflake(at, 0x2000 + n);
            if !text.is_empty() {
                self.store
                    .upsert_message(WatchedMessage {
                        id: id.clone(),
                        channel_id: channel.into(),
                        author_id: (*author).into(),
                        created_at: at,
                        edited_at: None,
                        content: (*text).into(),
                        processed_at: Some(at),
                    })
                    .await?;
            }
            ids.push(id);
        }
        Ok(ids)
    }

    /// Stage `p` as its source did at its read time, with its card (posted
    /// at once unless `card_posted` is false).
    async fn propose(&self, p: &Proposal) -> Result<(), Fail> {
        let read_at = self.hour(p.read_at);
        let ids = self.channel_lines(p.channel, p.thread).await?;
        let cited: Vec<String> = p
            .thread
            .iter()
            .zip(ids)
            .filter(|(line, _)| line.3)
            .map(|(_, id)| id)
            .collect();
        let new_datetime =
            p.to.map(|(day, time)| self.at(0, day, Some(time)))
                .transpose()?;
        let to_strings = |list: &[&str]| list.iter().map(|id| (*id).to_owned()).collect();
        let change = ProposedChange {
            run_id: p.run.map(str::to_owned),
            channel_id: Some(p.channel.into()),
            bosses: to_strings(p.bosses),
            participants: to_strings(p.participants),
            new_datetime,
            ..ProposedChange::new(p.kind)
        };
        let directory = self.directory().await?;
        self.scheduler(read_at, &[p.id])
            .propose(
                ProposalRequest {
                    change,
                    source: p.source,
                    source_id: p.source_id.into(),
                    supersede: Supersede::Keep,
                },
                &self.state.policy,
                &directory,
            )
            .await
            .map_err(|error| failed(p.id)(format!("{error:?}")))?;
        let details = CardDetails {
            kind: p.kind,
            run_id: p.run.map(str::to_owned),
            bosses: to_strings(p.bosses),
            participants: to_strings(p.participants),
            new_datetime,
            day_ref: None,
            time_ref: None,
            rsvp: None,
            is_question: p.is_question,
            summary: Some(p.summary.into()),
            also_mentioned: Vec::new(),
            confidence: p.confidence,
            payload: CardPayload::default(),
            evidence_message_ids: cited,
            self_service: None,
        };
        self.store
            .save_card(p.id, p.channel, &details, read_at)
            .await?;
        if p.card_posted {
            let posted = read_at + Duration::seconds(5);
            super::notify::deliver(
                self,
                NotificationIntent {
                    effect: EffectKind::Card,
                    effect_context: Vec::new(),
                    channel_id: p.channel.into(),
                    targets: vec![DeliveryTarget::Card(p.id.into())],
                    mentions: Vec::new(),
                    content: IntentContent::ProposalCard {
                        proposal_ids: vec![p.id.into()],
                    },
                    warnings: Vec::new(),
                },
                Seed::snowflake(posted, 0x3000),
                None,
                posted,
            )
            .await?;
        }
        Ok(())
    }

    fn approver(user_id: &str, is_admin: bool) -> Approver {
        Approver {
            user_id: user_id.into(),
            has_role: true,
            is_admin,
            via_portal: is_admin,
        }
    }
}

/// History record 3: the extractor proposes Kalos at 22:00 and Asahi
/// approves it from the Inbox two hours later.
pub async fn kalos_move(seed: &Seed<'_>) -> Result<(), Fail> {
    seed.propose(&KALOS).await?;
    let directory = seed.directory().await?;
    seed.scheduler(seed.hour(KALOS.read_at + 2), &[])
        .approve_proposal(
            KALOS_MOVE,
            &Seed::approver(ASAHI, true),
            &seed.state.policy,
            &directory,
        )
        .await
        .map_err(|error| failed(KALOS_MOVE)(format!("{error:?}")))?;
    Ok(())
}

/// A member request: its id, requester, title, what it asks, and when
/// (an instant).
struct Ask {
    id: &'static str,
    member: &'static str,
    title: &'static str,
    spec: RequestSpec,
    at: DateTime<Utc>,
}

/// How a seeded request closed: by whom, how and when.
enum Close {
    Withdrawn(DateTime<Utc>),
    Rejected(Actor, &'static str, DateTime<Utc>),
}

impl Seed<'_> {
    async fn ask(&self, ask: Ask) -> Result<u64, Fail> {
        let directory = self.directory().await?;
        let draft = self
            .scheduler(ask.at, &[ask.id])
            .submit_request(
                ask.member,
                ask.title,
                ask.spec,
                None,
                &self.state.policy,
                &directory,
                &NoFreezes,
            )
            .await
            .map_err(|error| failed(ask.id)(format!("{error:?}")))?;
        Ok(draft.version)
    }

    async fn close(&self, id: &str, member: &str, version: u64, close: Close) -> Result<(), Fail> {
        let closed = match close {
            Close::Withdrawn(at) => self
                .scheduler(at, &[])
                .withdraw_request(member, id, version)
                .await
                .map(drop),
            Close::Rejected(actor, reason, at) => self
                .scheduler(at, &[])
                .reject_request(&actor, id, version, reason)
                .await
                .map(drop),
        };
        closed.map_err(|error| failed(id)(format!("{error:?}")))
    }
}

fn run(id: &str) -> Subject {
    Subject::Run(id.into())
}

fn fixed(id: &str) -> Subject {
    Subject::Fixed(id.into())
}

fn new_fixed(
    owner: &str,
    bosses: &[&str],
    weekday: Weekday,
    time: &str,
    channel: &str,
    party: &[&str],
) -> Result<RequestSpec, Fail> {
    Ok(RequestSpec::NewFixed(NewFixedRun {
        owner_id: owner.into(),
        channel_id: Some(channel.into()),
        bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
        weekday,
        time: hhmm(time)?,
        participants: party.iter().map(|id| (*id).to_owned()).collect(),
        note: None,
        owner_pinned: false,
    }))
}

fn admin(id: &str) -> Actor {
    Actor::admin(id.to_owned())
}

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    for p in &LIVE {
        seed.propose(p).await?;
    }
    requests(seed).await?;
    mine(seed).await?;
    ownership(seed).await?;
    drafts(seed).await?;
    Ok(())
}

/// The Past tab: closed proposals and requests, oldest first, decided
/// before the boss week's first change (each is dry-run on the week as it
/// then stood).
pub async fn past(seed: &Seed<'_>) -> Result<(), Fail> {
    let week = |hours: i64| seed.week + Duration::hours(hours);
    // Requests before the boss week began.
    let nagi = seed
        .ask(Ask {
            id: "c-nagi-join",
            member: NAGI,
            title: "can I come to Kalos?",
            spec: RequestSpec::Join(fixed("f-kalos")),
            at: week(-70),
        })
        .await?;
    let tsubame = seed
        .ask(Ask {
            id: "c-tsu-change",
            member: TSUBAME,
            title: "Wednesdays suit us",
            spec: RequestSpec::ChangeFixed {
                fixed_id: "f-kalos".into(),
                edit: FixedEdit {
                    weekday: Some(Weekday::Wed),
                    ..FixedEdit::default()
                },
            },
            at: week(-60),
        })
        .await?;
    seed.close(
        "c-nagi-join",
        NAGI,
        nagi,
        Close::Rejected(
            admin("discord:100000000000009999"),
            "Asked twice.",
            week(-50),
        ),
    )
    .await?;
    seed.close(
        "c-tsu-change",
        TSUBAME,
        tsubame,
        Close::Rejected(
            admin(&format!("discord:{MIKA}")),
            "Wednesday clashes with Kalos.",
            week(-40),
        ),
    )
    .await?;
    // The extractor's Seren cancel nobody answered: it expired.
    let [lotus, limbo, seren] = &PAST;
    seed.propose(seren).await?;
    seed.scheduler(seed.hour(seren.read_at) + Duration::hours(24), &[])
        .expire_due_proposals()
        .await
        .map_err(|error| failed(seren.id)(format!("{error:?}")))?;
    let yuzu = seed
        .ask(Ask {
            id: "c-yuzu-fixed",
            member: YUZU,
            title: "weekly star on Saturdays",
            spec: new_fixed(
                YUZU,
                &["HMaleficStar"],
                Weekday::Sat,
                "21:00",
                HSTAR_PARTY,
                &[YUZU],
            )?,
            at: week(4),
        })
        .await?;
    seed.close(
        "c-yuzu-fixed",
        YUZU,
        yuzu,
        Close::Rejected(admin("tailscale:ops"), "One weekly run per party.", week(9)),
    )
    .await?;
    let ren = seed
        .ask(Ask {
            id: "c-ren-leave",
            member: REN,
            title: "busy on Tuesday",
            spec: RequestSpec::Leave(run("r-carling")),
            at: week(10),
        })
        .await?;
    seed.close("c-ren-leave", REN, ren, Close::Withdrawn(week(14)))
        .await?;
    // A newer extraction about Limbo retired this one.
    seed.propose(limbo).await?;
    seed.scheduler(seed.hour(limbo.read_at) + Duration::hours(2), &[])
        .supersede_proposals(SupersedeScope {
            run_id: limbo.run,
            channel_id: Some(limbo.channel),
            bosses: &[],
            keep: None,
            from_channel: Some(limbo.channel),
            by: ProposalSource::Extraction,
        })
        .await
        .map_err(|error| failed(limbo.id)(format!("{error:?}")))?;
    // Mika asked the chatbot, then turned the card down herself.
    seed.propose(lotus).await?;
    seed.scheduler(seed.hour(lotus.read_at) + Duration::hours(3), &[])
        .reject_proposal(lotus.id, &Seed::approver(MIKA, false))
        .await
        .map_err(|error| failed(lotus.id)(format!("{error:?}")))?;
    Ok(())
}

/// The Inbox's member requests.
async fn requests(seed: &Seed<'_>) -> Result<(), Fail> {
    let asks = [
        Ask {
            id: "p-carling-link",
            member: NAGI,
            title: "Can I fill in this week?",
            spec: RequestSpec::Join(run("r-carling")),
            at: seed.hour(110),
        },
        Ask {
            id: "p-kalos-fixed",
            member: TSUBAME,
            title: "Saturdays suit everyone better.",
            spec: RequestSpec::ChangeFixed {
                fixed_id: "f-kalos".into(),
                edit: FixedEdit {
                    weekday: Some(Weekday::Sat),
                    time: Some(hhmm("22:30")?),
                    participants: Some(vec![
                        REN.into(),
                        ASAHI.into(),
                        TSUBAME.into(),
                        HINATA.into(),
                        YUZU.into(),
                    ]),
                    ..FixedEdit::default()
                },
            },
            at: seed.hour(111),
        },
        Ask {
            id: "p-fa-request",
            member: HOTARU,
            title: "Something came up on Monday.",
            spec: RequestSpec::Leave(run("r-fa")),
            at: seed.hour(112),
        },
        Ask {
            id: "p-limbo-new",
            member: MIKA,
            title: "A Sunday Normal Limbo for the newer players.",
            spec: new_fixed(
                MIKA,
                &["NLimbo"],
                Weekday::Sun,
                "21:00",
                LIMBO_TRIO,
                &[MIKA, NAGI],
            )?,
            at: seed.hour(114),
        },
    ];
    for ask in asks {
        seed.ask(ask).await?;
    }
    Ok(())
}

/// Asahi's own requests (the portal's Requests page).
async fn mine(seed: &Seed<'_>) -> Result<(), Fail> {
    let hours = |n: i64| seed.ago(n * 3_600);
    let new = seed
        .ask(Ask {
            id: "req-jupiter-new",
            member: ASAHI,
            title: "A Monday Jupiter for the two of us.",
            spec: new_fixed(
                ASAHI,
                &["HJupiter"],
                Weekday::Mon,
                "21:00",
                JUPITER_TRIO,
                &[ASAHI, SORA],
            )?,
            at: hours(120),
        })
        .await?;
    seed.close("req-jupiter-new", ASAHI, new, Close::Withdrawn(hours(110)))
        .await?;
    let bellona = seed
        .ask(Ask {
            id: "req-bellona-join",
            member: ASAHI,
            title: "Room for one more on Bellona?",
            spec: RequestSpec::Join(run("r-bellona")),
            at: hours(100),
        })
        .await?;
    seed.close(
        "req-bellona-join",
        ASAHI,
        bellona,
        Close::Rejected(admin(&format!("discord:{REN}")), "party is full", hours(90)),
    )
    .await?;
    seed.ask(Ask {
        id: "req-kalos-weekly",
        member: ASAHI,
        title: "21:00 suits Ren and Tsubame better on Fridays.",
        spec: RequestSpec::ChangeFixed {
            fixed_id: "f-kalos".into(),
            edit: FixedEdit {
                time: Some(hhmm("21:00")?),
                ..FixedEdit::default()
            },
        },
        at: seed.ago(58 * 60),
    })
    .await?;
    Ok(())
}

/// Two open ownership requests: Mika wants Carling (nearly expired),
/// Tsubame wants Kalos (pinned to Ren).
async fn ownership(seed: &Seed<'_>) -> Result<(), Fail> {
    for (id, timing, channel, requester, hours) in [
        ("own-carling", "f-carling", HSTAR_PARTY, MIKA, 20),
        ("own-kalos", "f-kalos", KALOS_FOUR, TSUBAME, 5),
    ] {
        seed.store
            .create_owner_request(OwnerRequest::open(
                id.into(),
                timing.into(),
                requester.into(),
                Some(channel.into()),
                seed.ago(hours * 3_600),
            ))
            .await?;
    }
    Ok(())
}

/// A draft: id, title, created and each op's hour, from the week's start.
type DraftPlan = (&'static str, &'static str, i64, Vec<(i64, DraftOp)>);

/// Two open administrator drafts. Drafts are their author's own, so they
/// are Asahi's, the admin the template signs in with Discord (as the mock).
async fn drafts(seed: &Seed<'_>) -> Result<(), Fail> {
    let me = &format!("discord:{ASAHI}");
    let directory = seed.directory().await?;
    // The mock's drafts count hours from the boss week's start.
    let at = |hours: i64| seed.week + Duration::hours(hours);
    let target = |id: &str| Target::Existing(id.into());
    let plans: [DraftPlan; 2] = [
        (
            "d-carling",
            "Carling retime",
            99,
            vec![(
                100,
                DraftOp::AmendRun {
                    run: target("r-carling"),
                    to: seed.at(0, 3, Some("21:00"))?,
                },
            )],
        ),
        (
            "d-weekend",
            "Weekend reshuffle",
            109,
            // The mock moves Friday's Kalos, which has ended by Monday; the
            // real scheduler refuses staging that, so the reshuffle takes
            // Tuesday's Black Mage instead.
            vec![
                (
                    110,
                    DraftOp::AmendRun {
                        run: target("r-bm"),
                        to: seed.at(0, 6, Some("21:30"))?,
                    },
                ),
                (
                    111,
                    DraftOp::SwapParticipants {
                        run: target("r-bm"),
                        remove: Vec::new(),
                        add: vec![HINATA.into()],
                        via_portal: true,
                    },
                ),
            ],
        ),
    ];
    for (id, title, created, ops) in plans {
        let mut version = seed
            .scheduler(at(created), &[id])
            .create_draft(me, title, None)
            .await
            .map_err(|error| failed(id)(format!("{error:?}")))?
            .version;
        for (hour, op) in ops {
            version = seed
                .scheduler(at(hour), &[])
                .add_draft_op(me, id, version, op, &seed.state.policy, &directory)
                .await
                .map_err(|error| failed(id)(format!("{error:?}")))?
                .draft
                .version;
        }
    }
    Ok(())
}
