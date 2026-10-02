//! The Inbox (admin-api "Inbox (A6)"): Kanade's proposals from party chat
//! (`extraction`) and from the chatbot (`chat`), plus members' requests
//! (`new_fixed`, `change_fixed`, `join`, `leave`, `swap`). Proposals are
//! decided only by a Discord-signed-in admin; requests by any admin session.

use super::dto::{Boss, Named, Participant};
use super::history::Actor;
use super::seed::{self, Rec};
use super::{MoveError, Store};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
pub struct Evidence {
    pub id: String,
    pub author: String,
    /// The author's Discord id; null once the message is gone.
    pub author_id: Option<String>,
    pub at: String,
    pub content: Option<String>,
    pub url: Option<String>,
    /// The message is no longer stored (deleted or pruned).
    pub missing: bool,
}

/// A message of the thread around the evidence; `used` when it is cited.
#[derive(Clone, Serialize)]
pub struct ThreadMessage {
    #[serde(flatten)]
    pub message: Evidence,
    pub used: bool,
}

/// A thread fixture line: the evidence at this index (cited), or another
/// stored message `(author, text, hour)`.
#[derive(Clone, Copy)]
pub enum Line {
    Said(usize),
    Other(&'static str, &'static str, i64),
}

#[derive(Clone, Serialize)]
pub struct SelfService {
    pub member: Named,
    /// Where the member opened the link from.
    pub via: &'static str,
    pub note: Option<String>,
}

/// A member request (v5 "requests as PRs"): who sent it and the state the
/// backend reports about it.
#[derive(Clone)]
pub struct Request {
    pub member: &'static str,
    /// The member's own title (admin-only).
    pub note: &'static str,
    /// The requester may no longer have it approved.
    pub unauthorised: bool,
    /// Hour (from the boss week's start) the request expires at.
    pub expires_hour: Option<i64>,
    /// The run's day and time the member saw when they asked; a different
    /// value now is a conflict, and conflicts always block.
    pub base: Option<(u8, Option<&'static str>)>,
    /// The generated `member request: <type> <subject>` line.
    pub public_summary: &'static str,
}

#[derive(Clone)]
pub struct Proposal {
    pub id: &'static str,
    pub short_id: &'static str,
    pub kind: &'static str,
    /// `extraction` or `chat` for proposals; requests are `self_service`.
    pub source: &'static str,
    pub run_id: Option<&'static str>,
    /// For moves and adds: target boss-week day and time.
    pub day: u8,
    pub time: Option<&'static str>,
    pub bosses: Vec<&'static str>,
    pub participants: Vec<&'static str>,
    pub confidence: Option<f32>,
    pub is_question: bool,
    pub channel: &'static str,
    pub read_at_hour: i64,
    pub summary: &'static str,
    pub evidence: Vec<(&'static str, &'static str, i64, Option<&'static str>)>,
    /// The channel thread, oldest first; `None` serves `thread: null`.
    pub thread: Option<Vec<Line>>,
    pub request: Option<Request>,
    /// `change_fixed`: the weekly timing and its new weekday (0 = Monday) and
    /// time; `new_fixed`: an empty id and the new timing's slot. `swap`
    /// names the member leaving, then the one joining, in `participants`.
    pub timing: Option<(&'static str, u8, &'static str)>,
    /// Bumped by every edit; approve and reject may name the version they saw.
    pub version: u32,
}

#[derive(Serialize)]
pub struct Change {
    pub field: &'static str,
    pub from: String,
    pub to: String,
}

#[derive(Serialize)]
pub struct Conflict {
    pub field: &'static str,
    pub expected: String,
    pub found: String,
}

#[derive(Serialize)]
pub struct Preview {
    pub no_effect: bool,
    pub changes: Vec<Change>,
    pub conflicts: Vec<Conflict>,
}

#[derive(Serialize)]
pub struct Choice {
    pub run_id: String,
    pub label: String,
    pub when: String,
    /// The run already differs from its weekly timing (someone moved it).
    pub amended: bool,
}

#[derive(Serialize)]
pub struct ProposalDto {
    pub id: &'static str,
    pub short_id: &'static str,
    pub kind: &'static str,
    pub kind_label: &'static str,
    pub source: &'static str,
    /// The inbox tab it belongs to.
    pub tab: &'static str,
    pub version: u32,
    /// Badges: `conflict`, `expired`, `requester_unauthorised`, `no_effect`.
    pub flags: Vec<&'static str>,
    pub preview: Preview,
    pub expires_at: Option<String>,
    /// Weekly-timing changes: each open run of the timing needs update or keep.
    pub choices: Option<Vec<Choice>>,
    pub public_summary: Option<&'static str>,
    pub bosses: Vec<Boss>,
    pub run_id: Option<&'static str>,
    pub from_when: Option<String>,
    pub when: String,
    pub participants: Vec<Named>,
    pub confidence: Option<f32>,
    pub is_question: bool,
    pub channel: Option<&'static str>,
    pub read_at: String,
    pub summary: &'static str,
    pub evidence: Vec<Evidence>,
    /// The stored channel messages around `evidence` (deleted ones absent).
    pub thread: Option<Vec<ThreadMessage>>,
    pub card_url: Option<String>,
    pub self_service: Option<SelfService>,
}

#[derive(Deserialize, Default)]
pub struct ApproveRequest {
    /// Edit, then approve (proposals with a time): day of the proposed
    /// instant's boss week and `HH:MM`.
    pub day: Option<u8>,
    pub time: Option<String>,
    /// Required for requests; a different one is 409 stale.
    pub version: Option<u32>,
    /// `change_fixed`: amended run id -> `update` | `keep`.
    pub choices: Option<std::collections::BTreeMap<String, String>>,
    /// Refused: conflicts always block (`force_unsupported`).
    pub force: Option<bool>,
}

/// A closed item, kept so a repeated decision answers as the first did.
#[derive(Clone)]
pub struct Decided {
    pub proposal: Proposal,
    pub approved: bool,
    /// The approval's choices or edit, or the rejection's reason.
    pub digest: String,
    pub actor: Actor,
    pub message: String,
}

#[derive(Deserialize, Default)]
pub struct RejectRequest {
    pub version: Option<u32>,
    /// Required (1-500 characters) for member requests; the member is told.
    pub reason: Option<String>,
}

/// A boss week's reset, as an hour of `at_hour`.
const RESET_HOUR: i64 = 7 * 24 + 8;

const fn request(
    member: &'static str,
    note: &'static str,
    public_summary: &'static str,
) -> Request {
    Request {
        member,
        note,
        unauthorised: false,
        expires_hour: Some(RESET_HOUR),
        base: None,
        public_summary,
    }
}

pub fn seed() -> Vec<Proposal> {
    let base = Proposal {
        id: "",
        short_id: "",
        kind: "move",
        source: "extraction",
        run_id: None,
        day: 0,
        time: None,
        bosses: vec![],
        participants: vec![],
        confidence: None,
        is_question: false,
        channel: "",
        read_at_hour: 126,
        summary: "",
        evidence: vec![],
        thread: None,
        request: None,
        timing: None,
        version: 1,
    };
    vec![
        Proposal {
            id: "p-bm-move",
            short_id: "a7c1e9d2",
            run_id: Some("r-bm"),
            day: 6,
            time: Some("23:30"),
            bosses: vec!["XBM"],
            participants: vec!["1012", "1009", "1008"],
            confidence: Some(0.86),
            channel: "bm-trio",
            summary: "Minato asks to push Black Mage to Wednesday; Kaito agrees.",
            evidence: vec![
                ("1012", "tue cannot, wed same time ok?", 125, Some("m1")),
                ("1009", "wed ok for me", 126, Some("m2")),
                ("1008", "", 126, None),
            ],
            // The missing evidence (index 2) is gone from the thread too.
            thread: Some(vec![
                Line::Other("1009", "bm still tue this week?", 124),
                Line::Other("1008", "should be", 124),
                Line::Said(0),
                Line::Other("1008", "brb dinner", 125),
                Line::Said(1),
                Line::Other("1012", "ty, will ask kanade", 126),
            ]),
            ..base.clone()
        },
        Proposal {
            id: "p-limbo-add",
            short_id: "c8e0a2b4",
            kind: "add",
            day: 2,
            time: Some("21:00"),
            bosses: vec!["NLimbo"],
            participants: vec!["1003", "1007"],
            confidence: Some(0.52),
            is_question: true,
            channel: "limbo-trio",
            read_at_hour: 124,
            summary: "Mika floats a Normal Limbo run on Saturday; nobody has confirmed.",
            evidence: vec![("1003", "nlimbo sat 9pm anyone?", 123, Some("m3"))],
            thread: Some(vec![
                Line::Other("1007", "finally cleared hlimbo prequest", 122),
                Line::Said(0),
                Line::Other("1007", "maybe, will check", 124),
            ]),
            ..base.clone()
        },
        // Asked of the chatbot rather than read from the party channel.
        Proposal {
            id: "p-jupiter-chat",
            short_id: "b2c4d6e8",
            source: "chat",
            run_id: Some("r-jupiter"),
            day: 6,
            time: Some("21:00"),
            bosses: vec!["HJupiter"],
            participants: vec!["1012"],
            channel: "jupiter-trio",
            read_at_hour: 128,
            summary: "Minato asked Kanade to move HJupiter to Wednesday 21:00.",
            evidence: vec![(
                "1012",
                "@Kanade can jupiter be wed 9pm instead?",
                128,
                Some("m9"),
            )],
            thread: Some(vec![
                Line::Other("1012", "wed works better for me this week", 127),
                Line::Said(0),
            ]),
            ..base.clone()
        },
        Proposal {
            id: "p-carling-link",
            short_id: "b3d5f7a9",
            kind: "join",
            run_id: Some("r-carling"),
            bosses: vec!["HCarling", "HStar"],
            participants: vec!["1007"],
            channel: "hstar-party",
            read_at_hour: 110,
            summary: "Can I fill in this week?",
            request: Some(request(
                "1007",
                "Can I fill in this week?",
                "member request: join HCarling + HStar Tue 29 Sep 22:00",
            )),
            ..base.clone()
        },
        // Asked while FA was at 19:30; the run has moved since: a conflict.
        Proposal {
            id: "p-fa-request",
            short_id: "e5f7a9b1",
            kind: "leave",
            run_id: Some("r-fa"),
            bosses: vec!["HFA"],
            participants: vec!["1011"],
            channel: "fa-night",
            read_at_hour: 112,
            summary: "Something came up on Monday.",
            request: Some(Request {
                base: Some((4, Some("19:30"))),
                ..request(
                    "1011",
                    "Something came up on Monday.",
                    "member request: leave HFA Mon 28 Sep 19:30",
                )
            }),
            ..base.clone()
        },
        // Expired before anyone looked: it can only be rejected.
        Proposal {
            id: "p-kalos-expired",
            short_id: "f6a8b0c2",
            kind: "swap",
            run_id: Some("r-kalos"),
            bosses: vec!["XKalos"],
            participants: vec!["1002", "1004"],
            channel: "kalos-four",
            read_at_hour: 20,
            summary: "Yuzu takes my XKalos spot.",
            request: Some(Request {
                expires_hour: Some(-12),
                ..request(
                    "1002",
                    "Yuzu takes my XKalos spot.",
                    "member request: swap XKalos Fri 25 Sep 22:00",
                )
            }),
            ..base.clone()
        },
        // Already on the run, and no longer allowed to have it approved.
        Proposal {
            id: "p-jupiter-same",
            short_id: "a9b1c3d5",
            kind: "join",
            run_id: Some("r-jupiter"),
            bosses: vec!["HJupiter"],
            participants: vec!["1008"],
            channel: "jupiter-trio",
            read_at_hour: 113,
            summary: "Put me on Jupiter.",
            request: Some(Request {
                unauthorised: true,
                ..request(
                    "1008",
                    "Put me on Jupiter.",
                    "member request: join HJupiter Mon 28 Sep 21:00",
                )
            }),
            ..base.clone()
        },
        // A weekly-timing change: only the amended run (r-kalos) needs a choice.
        Proposal {
            id: "p-kalos-fixed",
            short_id: "d4e6f8a0",
            kind: "change_fixed",
            bosses: vec!["XKalos"],
            participants: vec!["1005"],
            channel: "kalos-four",
            read_at_hour: 111,
            summary: "Saturdays suit everyone better.",
            request: Some(Request {
                expires_hour: None,
                ..request(
                    "1005",
                    "Saturdays suit everyone better.",
                    "member request: change_fixed XKalos Fri 21:30",
                )
            }),
            timing: Some(("f-kalos", 5, "22:30")),
            ..base.clone()
        },
        Proposal {
            id: "p-limbo-new",
            short_id: "c1d3e5f7",
            kind: "new_fixed",
            bosses: vec!["NLimbo"],
            participants: vec!["1003", "1007"],
            channel: "limbo-trio",
            read_at_hour: 114,
            summary: "A Sunday Normal Limbo for the newer players.",
            request: Some(Request {
                expires_hour: None,
                ..request(
                    "1003",
                    "A Sunday Normal Limbo for the newer players.",
                    "member request: new_fixed NLimbo Sun 21:00",
                )
            }),
            timing: Some(("", 6, "21:00")),
            ..base
        },
    ]
}

/// A message as the server shows it; empty `text` is a gone message.
fn evidence(id: String, who: &str, text: &str, hour: i64, link: Option<&str>) -> Evidence {
    Evidence {
        author: seed::member_name(who).map_or("someone", |m| m.1).into(),
        author_id: (!text.is_empty()).then(|| who.to_owned()),
        at: Store::when(Store::at_hour(hour)),
        content: (!text.is_empty()).then(|| text.to_owned()),
        url: link
            .map(str::to_owned)
            .or_else(|| (!text.is_empty()).then(|| id.clone()))
            .map(|l| format!("https://discord.com/channels/0/0/{l}")),
        missing: text.is_empty(),
        id,
    }
}

/// As the server's `kind_label`.
fn label(kind: &str) -> &'static str {
    match kind {
        "move" => "Move",
        "add" => "New run",
        "cancel" => "Cancel",
        "split" => "Split",
        "otot" => "Own time",
        "sub" | "swap" => "Swap",
        "rsvp" => "Answer",
        "fix" => "Weekly timing",
        "new_fixed" => "New weekly run",
        "change_fixed" => "Weekly timing change",
        "join" => "Join",
        "leave" => "Leave",
        _ => "Change",
    }
}

fn coded(status: u16, code: &'static str, message: impl Into<String>) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

const DOW_MON: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// `Tue 22:00`, a weekly slot (0 = Monday), as the server writes it.
fn weekly(weekday: u8, time: &str) -> String {
    format!("{} {time}", DOW_MON[usize::from(weekday) % 7])
}

fn stale() -> MoveError {
    coded(
        409,
        "stale",
        "It changed or was decided since you opened it; review it again.",
    )
}

fn discord_required() -> MoveError {
    coded(
        403,
        "discord_session_required",
        "Sign in with Discord to approve or reject Kanade's proposals.",
    )
}

fn version_required() -> MoveError {
    coded(
        422,
        "version_required",
        "Name the version you reviewed (`version`).",
    )
}

fn mismatch() -> MoveError {
    coded(
        422,
        "idempotency_mismatch",
        "That was already decided with different details.",
    )
}

/// Only these proposals have a time an edit can replace.
fn timed(kind: &str) -> bool {
    matches!(kind, "move" | "add" | "split")
}

impl Store {
    fn at_hour(h: i64) -> i64 {
        Self::start(false) * 1440 - 8 * 60 + h * 60
    }

    fn target_minute(p: &Proposal) -> i64 {
        Self::start(false) * 1440
            + i64::from(p.day) * 1440
            + p.time.map_or(0, super::clock::minutes)
    }

    fn slot_when(day: u8, time: Option<&str>) -> String {
        Self::when(
            Self::start(false) * 1440
                + i64::from(day) * 1440
                + time.map_or(0, super::clock::minutes),
        )
    }

    fn names_of(run: &Rec) -> String {
        run.participants
            .iter()
            .map(|p| p.name)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The amended runs a timing change would move: each needs a choice.
    fn amended_runs(&self, fixed_id: &str) -> Vec<&Rec> {
        self.runs
            .iter()
            .filter(|r| {
                r.fixed_id.as_deref() == Some(fixed_id)
                    && matches!(r.status, "planned" | "confirmed" | "at_risk")
                    && self.amended(r)
            })
            .collect()
    }

    /// Badges, preview and per-run choices, derived from the live week.
    fn review(&self, p: &Proposal) -> (Vec<&'static str>, Preview, Option<Vec<Choice>>) {
        let run = p
            .run_id
            .and_then(|id| self.runs.iter().find(|r| r.id == id));
        let mut flags = Vec::new();
        let mut changes = Vec::new();
        let mut conflicts = Vec::new();
        let mut no_effect = false;
        let mut choices = None;
        let on = |r: &Rec, id: &str| r.participants.iter().any(|x| x.id == id);
        let name = |id: &str| seed::member_name(id).map_or(id.to_owned(), |m| m.1.to_owned());
        match (p.kind, run) {
            ("move", Some(r)) => {
                no_effect = r.day == p.day && r.time.as_deref() == p.time;
                changes.push(Change {
                    field: "slot",
                    from: Self::when(Self::start_minute(r)),
                    to: Self::when(Self::target_minute(p)),
                });
            }
            ("join" | "leave" | "swap", Some(r)) => {
                let who = p.participants.first().copied().unwrap_or_default();
                let joining = p.participants.get(1).copied();
                let (effect, to) = match p.kind {
                    "join" => (!on(r, who), format!("{}, {}", Self::names_of(r), name(who))),
                    "leave" => (
                        on(r, who),
                        r.participants
                            .iter()
                            .filter(|x| x.id != who)
                            .map(|x| x.name)
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    _ => {
                        let incoming = joining.unwrap_or_default();
                        (
                            on(r, who) && !on(r, incoming),
                            Self::names_of(r).replace(&name(who), &name(incoming)),
                        )
                    }
                };
                no_effect = !effect;
                changes.push(Change {
                    field: "participants",
                    from: Self::names_of(r),
                    to,
                });
            }
            ("change_fixed", _) => {
                if let Some((fixed_id, weekday, time)) = p.timing
                    && let Some(f) = self.fixed.iter().find(|f| f.id == fixed_id)
                {
                    no_effect = f.weekday == weekday && f.time == time;
                    changes.push(Change {
                        field: "day_time",
                        from: weekly(f.weekday, &f.time),
                        to: weekly(weekday, time),
                    });
                    // Unamended runs follow the timing; only amended ones are asked about.
                    choices = Some(
                        self.amended_runs(fixed_id)
                            .into_iter()
                            .map(|r| Choice {
                                run_id: r.id.clone(),
                                label: if r.next_week {
                                    "Next week"
                                } else {
                                    "This week"
                                }
                                .into(),
                                when: Self::when(Self::start_minute(r)),
                                amended: true,
                            })
                            .collect(),
                    );
                }
            }
            ("new_fixed", _) => {
                if let Some((_, weekday, time)) = p.timing {
                    changes.push(Change {
                        field: "new_fixed",
                        from: "—".into(),
                        to: format!("{} {}", weekly(weekday, time), p.bosses.join(" + ")),
                    });
                }
            }
            ("add", _) => changes.push(Change {
                field: "new_run",
                from: "—".into(),
                to: Self::when(Self::target_minute(p)),
            }),
            _ => {}
        }
        if let (Some(r), Some((day, time))) = (run, p.request.as_ref().and_then(|q| q.base))
            && (day != r.day || time != r.time.as_deref())
        {
            conflicts.push(Conflict {
                field: "slot",
                expected: Self::slot_when(day, time),
                found: Self::when(Self::start_minute(r)),
            });
        }
        if !conflicts.is_empty() {
            flags.push("conflict");
        }
        if self
            .expires_hour(p)
            .is_some_and(|h| Self::at_hour(h) <= Self::now_minute())
        {
            flags.push("expired");
        }
        if p.request.as_ref().is_some_and(|q| q.unauthorised) {
            flags.push("requester_unauthorised");
        }
        if no_effect {
            flags.push("no_effect");
        }
        (
            flags,
            Preview {
                no_effect,
                changes,
                conflicts,
            },
            choices,
        )
    }

    /// A proposal's 24 h TTL or its week's reset, whichever is first; a
    /// request's week reset (none for weekly-only requests).
    fn expires_hour(&self, p: &Proposal) -> Option<i64> {
        match &p.request {
            Some(q) => q.expires_hour,
            None => Some((p.read_at_hour + 24).min(RESET_HOUR)),
        }
    }

    pub fn inbox(&self) -> Vec<ProposalDto> {
        self.proposals
            .iter()
            .map(|p| {
                let run = p
                    .run_id
                    .and_then(|id| self.runs.iter().find(|r| r.id == id));
                let named = |id: &str| {
                    seed::member_name(id).map(|(id, name)| Named {
                        id: id.into(),
                        name: name.into(),
                    })
                };
                let timing = p
                    .timing
                    .and_then(|(id, _, _)| self.fixed.iter().find(|f| f.id == id));
                let from_when = match (p.kind, run) {
                    ("move", Some(r)) => Some(Self::when(Self::start_minute(r))),
                    ("change_fixed", _) => timing.map(|f| weekly(f.weekday, &f.time)),
                    _ => None,
                };
                let when = match (p.kind, run, p.timing) {
                    ("change_fixed" | "new_fixed", _, Some((_, weekday, time))) => {
                        weekly(weekday, time)
                    }
                    ("move" | "add" | "split", _, _) => Self::when(Self::target_minute(p)),
                    (_, Some(r), _) => Self::when(Self::start_minute(r)),
                    _ => "—".into(),
                };
                let (flags, preview, choices) = self.review(p);
                let member = p.request.is_some();
                ProposalDto {
                    id: p.id,
                    short_id: p.short_id,
                    kind: p.kind,
                    kind_label: label(p.kind),
                    source: if member { "self_service" } else { p.source },
                    tab: if member { "self_service" } else { "extractor" },
                    version: p.version,
                    flags,
                    preview,
                    expires_at: self.expires_hour(p).map(|h| Self::when(Self::at_hour(h))),
                    choices,
                    public_summary: p.request.as_ref().map(|q| q.public_summary),
                    bosses: p
                        .bosses
                        .iter()
                        .filter_map(|t| super::catalog::boss_ref(t))
                        .map(|b| self.catalog.boss(&b.token, b.key, b.difficulty))
                        .collect(),
                    run_id: p.run_id,
                    from_when,
                    when,
                    participants: p.participants.iter().filter_map(|id| named(id)).collect(),
                    confidence: p.confidence,
                    is_question: p.is_question,
                    channel: seed::channel(p.channel).map(|c| c.1),
                    read_at: Self::when(Self::at_hour(p.read_at_hour)),
                    summary: p.summary,
                    evidence: p
                        .evidence
                        .iter()
                        .enumerate()
                        .map(|(i, &(who, text, hour, link))| {
                            evidence(format!("{}-{i}", p.short_id), who, text, hour, link)
                        })
                        .collect(),
                    thread: p.thread.as_ref().map(|lines| {
                        lines
                            .iter()
                            .enumerate()
                            .map(|(j, line)| match *line {
                                Line::Said(i) => {
                                    let (who, text, hour, link) = p.evidence[i];
                                    ThreadMessage {
                                        message: evidence(
                                            format!("{}-{i}", p.short_id),
                                            who,
                                            text,
                                            hour,
                                            link,
                                        ),
                                        used: true,
                                    }
                                }
                                Line::Other(who, text, hour) => ThreadMessage {
                                    message: evidence(
                                        format!("{}-t{j}", p.short_id),
                                        who,
                                        text,
                                        hour,
                                        None,
                                    ),
                                    used: false,
                                },
                            })
                            .collect()
                    }),
                    card_url: (!member)
                        .then(|| format!("https://discord.com/channels/0/0/card-{}", p.short_id)),
                    self_service: p.request.as_ref().map(|q| SelfService {
                        member: named(q.member).unwrap_or(Named {
                            id: q.member.into(),
                            name: q.member.into(),
                        }),
                        via: "request",
                        note: Some(q.note.into()),
                    }),
                }
            })
            .collect()
    }

    fn decided_message(verb: &str, p: &Proposal) -> String {
        format!("{verb}: {} #{}.", label(p.kind).to_lowercase(), p.short_id)
    }

    /// The live item, or the closed one a repeated decision refers to.
    fn item(&self, id: &str) -> Option<(Proposal, Option<Decided>)> {
        if let Some(p) = self.proposals.iter().find(|p| p.id == id) {
            return Some((p.clone(), None));
        }
        self.decided
            .iter()
            .rev()
            .find(|d| d.proposal.id == id)
            .map(|d| (d.proposal.clone(), Some(d.clone())))
    }

    /// A repeat answers the first decision when it is the same one by the same
    /// admin; another decision on a closed item is stale.
    fn replay(
        d: &Decided,
        approve: bool,
        digest: &str,
        actor: &Actor,
    ) -> Result<String, MoveError> {
        if d.approved != approve || &d.actor != actor {
            return Err(stale());
        }
        if d.digest != digest {
            return Err(mismatch());
        }
        Ok(d.message.clone())
    }

    /// Who decides this item: the admin's Discord user for proposals (as their
    /// ✅ on the card), the session's admin for requests.
    pub fn decider(&self, p: &Proposal) -> Result<Actor, MoveError> {
        if p.request.is_some() {
            return Ok(self.session_actor());
        }
        self.discord_user()
            .map(|id| Actor::new("member", id))
            .ok_or_else(discord_required)
    }

    /// The edited instant's day and time, or None for a plain approval.
    fn edit(p: &Proposal, req: &ApproveRequest) -> Result<Option<(u8, String)>, MoveError> {
        let (day, time) = match (req.day, req.time.as_deref()) {
            (None, None) => return Ok(None),
            (Some(day), Some(time)) => (day, time),
            _ => return Err(MoveError::invalid("An edit needs a day and a time.")),
        };
        if !timed(p.kind) {
            return Err(coded(
                422,
                "edit_not_applicable",
                "Only a change with a time (a move or a new run) can be edited before approving.",
            ));
        }
        if day > 6 {
            return Err(MoveError::invalid("A boss week has seven days."));
        }
        if !super::clock::valid_time(time) {
            return Err(MoveError::invalid("Times are HH:MM, 00:00 to 23:59."));
        }
        // An edit equal to the proposed time is a plain approval.
        Ok((day != p.day || Some(time) != p.time).then(|| (day, time.to_owned())))
    }

    pub fn approve(&mut self, id: &str, req: ApproveRequest) -> Result<String, MoveError> {
        if req.force == Some(true) {
            return Err(coded(
                422,
                "force_unsupported",
                "Conflicts cannot be approved over; reject it or change the schedule first.",
            ));
        }
        let (p, closed) = self.item(id).ok_or(MoveError::NotFound)?;
        let actor = self.decider(&p);
        let (edit, digest) = if p.request.is_some() {
            if req.day.is_some() || req.time.is_some() {
                return Err(coded(
                    422,
                    "edit_not_applicable",
                    "Member requests are approved as asked; reject and ask for a new one to change them.",
                ));
            }
            let version = req.version.ok_or_else(version_required)?;
            let digest = format!("{:?}", req.choices);
            if let Some(d) = &closed {
                return Self::replay(d, true, &digest, &actor?);
            }
            if version != p.version {
                return Err(stale());
            }
            (None, digest)
        } else {
            let actor = actor.as_ref().map_err(Clone::clone)?;
            if req.choices.is_some() {
                return Err(coded(
                    422,
                    "choices_not_applicable",
                    "Only weekly-timing change requests take per-run choices.",
                ));
            }
            if closed.is_none() && req.version.is_some_and(|v| v != p.version) {
                return Err(stale());
            }
            let edit = Self::edit(&p, &req)?;
            let digest = format!("{edit:?}");
            if let Some(d) = &closed {
                return Self::replay(d, true, &digest, actor);
            }
            (edit, digest)
        };
        let actor = actor?;
        let (flags, _, choices) = self.review(&p);
        if flags.contains(&"expired") {
            // The attempt closes it, as the tick would have.
            self.close(&p, false, "expired".into(), actor, String::new());
            return Err(coded(
                410,
                "expired",
                "It expired and is now closed; nothing was applied.",
            ));
        }
        if flags.contains(&"requester_unauthorised") {
            return Err(coded(
                409,
                "requester_unauthorised",
                "The member may no longer have this approved; reject it.",
            ));
        }
        if flags.contains(&"conflict") {
            return Err(coded(
                409,
                "conflicts",
                "It changed since the member asked; reject it.",
            ));
        }
        if flags.contains(&"no_effect") && edit.is_none() {
            return Err(coded(
                409,
                "no_effect",
                "It is already like that; nothing would change. Reject it instead.",
            ));
        }
        match (&choices, &req.choices) {
            (None, Some(_)) => {
                return Err(coded(
                    422,
                    "choices_not_applicable",
                    "Only weekly-timing change requests take per-run choices.",
                ));
            }
            (Some(_), None) => {
                return Err(coded(
                    422,
                    "choices_required",
                    "Choose update or keep for every amended run (send {} when none are listed).",
                ));
            }
            (Some(runs), Some(given)) => {
                if given.keys().any(|k| runs.iter().all(|c| &c.run_id != k)) {
                    return Err(coded(
                        422,
                        "choices_not_applicable",
                        "A choice names a run this change does not move.",
                    ));
                }
                if runs.iter().any(|c| {
                    !given
                        .get(&c.run_id)
                        .is_some_and(|v| v == "update" || v == "keep")
                }) {
                    return Err(coded(
                        422,
                        "choices_required",
                        "Choose update or keep for every amended run.",
                    ));
                }
            }
            (None, None) => {}
        }
        self.apply(&p, edit, req.choices.as_ref())?;
        let message = Self::decided_message("Approved", &p);
        self.close(&p, true, digest, actor, message.clone());
        self.version += 1;
        Ok(message)
    }

    fn close(
        &mut self,
        p: &Proposal,
        approved: bool,
        digest: String,
        actor: Actor,
        message: String,
    ) {
        self.proposals.retain(|x| x.id != p.id);
        self.decided.push(Decided {
            proposal: p.clone(),
            approved,
            digest,
            actor,
            message,
        });
    }

    fn apply(
        &mut self,
        p: &Proposal,
        edit: Option<(u8, String)>,
        choices: Option<&std::collections::BTreeMap<String, String>>,
    ) -> Result<(), MoveError> {
        let run_index = p
            .run_id
            .and_then(|rid| self.runs.iter().position(|r| r.id == rid));
        let (day, time) = edit.map_or((p.day, p.time.map(Into::into)), |(d, t)| (d, Some(t)));
        let gone = || MoveError::invalid("That run is no longer on the board.");
        let person = |id: &str| {
            seed::member_name(id).map(|(id, name)| Participant {
                id,
                name,
                answer: "waiting",
            })
        };
        match p.kind {
            "move" => {
                let run = &mut self.runs[run_index.ok_or_else(gone)?];
                run.day = day;
                run.time = time;
            }
            "add" => {
                let (id, short_id) = self.fresh_id("r");
                let bosses = p
                    .bosses
                    .iter()
                    .filter_map(|t| super::catalog::boss_ref(t))
                    .collect();
                self.runs.push(Rec {
                    id,
                    short_id,
                    next_week: false,
                    day,
                    time,
                    status: "planned",
                    bosses,
                    participants: p
                        .participants
                        .iter()
                        .filter_map(|m| seed::member_name(m))
                        .map(|(id, name)| Participant {
                            id,
                            name,
                            answer: "yes",
                        })
                        .collect(),
                    channel: p.channel,
                    fixed_id: None,
                });
            }
            "join" | "leave" | "swap" => {
                let run = &mut self.runs[run_index.ok_or_else(gone)?];
                let who = p.participants.first().copied().unwrap_or_default();
                if p.kind != "join" {
                    run.participants.retain(|x| x.id != who);
                }
                let joining = if p.kind == "join" {
                    Some(who)
                } else {
                    p.participants.get(1).copied()
                };
                if let Some(joiner) = joining.filter(|_| p.kind != "leave").and_then(person) {
                    run.participants.push(joiner);
                }
            }
            "change_fixed" => {
                let (fixed_id, weekday, time) = p
                    .timing
                    .ok_or(MoveError::invalid("No weekly timing to change."))?;
                let amended: Vec<String> = self
                    .amended_runs(fixed_id)
                    .into_iter()
                    .map(|r| r.id.clone())
                    .collect();
                let fi = self
                    .fixed
                    .iter()
                    .position(|f| f.id == fixed_id && !f.retired)
                    .ok_or(MoveError::invalid("That weekly timing is retired."))?;
                self.fixed[fi].weekday = weekday;
                self.fixed[fi].time = time.into();
                let timing = &self.fixed[fi];
                for run in self.runs.iter_mut().filter(|r| {
                    r.fixed_id.as_deref() == Some(fixed_id)
                        && matches!(r.status, "planned" | "confirmed" | "at_risk")
                }) {
                    let keep = amended.contains(&run.id)
                        && choices.and_then(|c| c.get(&run.id)).map(String::as_str) == Some("keep");
                    if !keep {
                        super::fixed::apply_timing(run, timing);
                    }
                }
            }
            "new_fixed" => {
                let (_, weekday, time) = p
                    .timing
                    .ok_or(MoveError::invalid("No weekly timing to add."))?;
                self.create_fixed(super::dto::FixedRequest {
                    weekday,
                    time: time.into(),
                    bosses: p.bosses.join(" "),
                    participants: p.participants.iter().map(|m| (*m).to_owned()).collect(),
                    channel_id: p.channel.into(),
                    note: None,
                    owner_id: None,
                    decisions: Default::default(),
                    version: None,
                })?;
            }
            _ => return Err(MoveError::invalid("The mock cannot apply that change.")),
        }
        Ok(())
    }

    pub fn reject(&mut self, id: &str, req: RejectRequest) -> Result<String, MoveError> {
        let (p, closed) = self.item(id).ok_or(MoveError::NotFound)?;
        let actor = self.decider(&p)?;
        let reason = req.reason.as_deref().map(str::trim).unwrap_or_default();
        if p.request.is_some() {
            let version = req.version.ok_or_else(version_required)?;
            if reason.is_empty() {
                return Err(coded(
                    422,
                    "reason_required",
                    "Say why, in a sentence: the member is told.",
                ));
            }
            if reason.chars().count() > 500 {
                return Err(coded(
                    422,
                    "reason_invalid",
                    "A reason is at most 500 characters.",
                ));
            }
            if let Some(d) = &closed {
                return Self::replay(d, false, reason, &actor);
            }
            if version != p.version {
                return Err(stale());
            }
        } else {
            if !reason.is_empty() {
                return Err(coded(
                    422,
                    "reason_not_applicable",
                    "Kanade's proposals are rejected without a reason; nothing would keep it.",
                ));
            }
            if closed.is_none() && req.version.is_some_and(|v| v != p.version) {
                return Err(stale());
            }
            if let Some(d) = &closed {
                return Self::replay(d, false, "", &actor);
            }
        }
        let message = Self::decided_message("Rejected", &p);
        self.close(&p, false, reason.to_owned(), actor, message.clone());
        Ok(message)
    }

    /// Recorded as the server records it: a proposal as the approving
    /// member (`extraction_approval` / `chat_approval`), a request as the
    /// session's admin (`request_merge`).
    pub fn approve_tracked(&mut self, id: &str, req: ApproveRequest) -> Result<String, MoveError> {
        let Some((p, _)) = self.item(id) else {
            return Err(MoveError::NotFound);
        };
        let surface = match (p.request.is_some(), p.source) {
            (true, _) => "request_merge",
            (false, "chat") => "chat_approval",
            _ => "extraction_approval",
        };
        let actor = self.decider(&p).unwrap_or_else(|_| self.session_actor());
        self.tracked(actor, surface, |s| s.approve(id, req))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use super::{ApproveRequest, RejectRequest};
    use crate::mock::MoveError;
    use std::collections::BTreeMap;

    fn code(e: MoveError) -> (u16, &'static str) {
        match e {
            MoveError::Coded(status, code, _) => (status, code),
            other => panic!("not a coded refusal: {other}"),
        }
    }

    fn v1() -> ApproveRequest {
        ApproveRequest {
            version: Some(1),
            ..ApproveRequest::default()
        }
    }

    fn keep(run: &str) -> Option<BTreeMap<String, String>> {
        Some([(run.to_owned(), "keep".to_owned())].into())
    }

    #[test]
    fn the_inbox_lists_request_types_chat_proposals_and_amended_choices() {
        let s = store();
        let inbox = s.inbox();
        let by = |id: &str| inbox.iter().find(|p| p.id == id).unwrap();
        assert_eq!(
            (by("p-bm-move").source, by("p-bm-move").tab),
            ("extraction", "extractor")
        );
        assert_eq!(
            (by("p-jupiter-chat").source, by("p-jupiter-chat").tab),
            ("chat", "extractor")
        );
        let kinds: Vec<&str> = inbox
            .iter()
            .filter(|p| p.tab == "self_service")
            .map(|p| p.kind)
            .collect();
        for kind in ["new_fixed", "change_fixed", "join", "leave", "swap"] {
            assert!(kinds.contains(&kind), "{kind}");
        }
        assert!(
            inbox.iter().filter(|p| p.self_service.is_some()).all(|p| p
                .self_service
                .as_ref()
                .unwrap()
                .via
                == "request")
        );
        assert!(by("p-carling-link").flags.is_empty());
        assert_eq!(by("p-fa-request").flags, vec!["conflict"]);
        assert_eq!(by("p-kalos-expired").flags, vec!["expired"]);
        assert_eq!(
            by("p-jupiter-same").flags,
            vec!["requester_unauthorised", "no_effect"]
        );
        // Only the amended run is listed; the other follows the timing.
        let choices = by("p-kalos-fixed").choices.as_ref().unwrap();
        assert_eq!(
            choices
                .iter()
                .map(|c| c.run_id.as_str())
                .collect::<Vec<_>>(),
            ["r-kalos"]
        );
        assert!(choices.iter().all(|c| c.amended));
        assert!(by("p-bm-move").choices.is_none() && by("p-limbo-new").choices.is_none());
        assert!(by("p-bm-move").expires_at.is_some());
    }

    #[test]
    fn threads_mark_cited_messages_oldest_first_without_gone_ones() {
        let s = store();
        let inbox = s.inbox();
        let by = |id: &str| inbox.iter().find(|p| p.id == id).unwrap();
        for id in ["p-bm-move", "p-limbo-add", "p-jupiter-chat"] {
            let p = by(id);
            let thread = p.thread.as_ref().unwrap();
            assert!(thread.iter().any(|m| m.used) && thread.iter().any(|m| !m.used));
            assert!(thread.iter().all(|m| !m.message.missing), "{id}");
            for m in thread.iter().filter(|m| m.used) {
                assert!(p.evidence.iter().any(|e| e.id == m.message.id), "{id}");
            }
        }
        // Oldest first: the cited Tue message sits between Monday chatter.
        let bm: Vec<_> = by("p-bm-move")
            .thread
            .as_ref()
            .unwrap()
            .iter()
            .map(|m| m.used)
            .collect();
        assert_eq!(bm, [false, false, true, false, true, false]);
        // The deleted evidence stays listed as evidence only.
        assert!(by("p-bm-move").evidence[2].missing);
        // Member requests have no channel thread, as on the server.
        for p in inbox.iter().filter(|p| p.self_service.is_some()) {
            assert!(p.thread.is_none() && p.evidence.is_empty(), "{}", p.id);
        }
    }

    #[test]
    fn approving_follows_the_server_rules_and_replays() {
        let mut s = store();
        let forced = ApproveRequest {
            force: Some(true),
            ..v1()
        };
        assert_eq!(
            code(s.approve("p-carling-link", forced).unwrap_err()),
            (422, "force_unsupported")
        );
        assert_eq!(
            code(
                s.approve("p-carling-link", ApproveRequest::default())
                    .unwrap_err()
            ),
            (422, "version_required")
        );
        let edited = ApproveRequest {
            day: Some(1),
            time: Some("21:00".into()),
            ..v1()
        };
        assert_eq!(
            code(s.approve("p-carling-link", edited).unwrap_err()),
            (422, "edit_not_applicable")
        );
        assert_eq!(
            code(
                s.approve(
                    "p-carling-link",
                    ApproveRequest {
                        version: Some(9),
                        ..v1()
                    }
                )
                .unwrap_err()
            ),
            (409, "stale")
        );
        assert_eq!(
            code(s.approve("p-kalos-expired", v1()).unwrap_err()),
            (410, "expired")
        );
        assert_eq!(
            code(s.approve("p-jupiter-same", v1()).unwrap_err()),
            (409, "requester_unauthorised")
        );
        assert_eq!(
            code(s.approve("p-fa-request", v1()).unwrap_err()),
            (409, "conflicts")
        );
        assert_eq!(
            code(s.approve("p-kalos-fixed", v1()).unwrap_err()),
            (422, "choices_required")
        );
        let none = ApproveRequest {
            choices: Some(BTreeMap::new()),
            ..v1()
        };
        assert_eq!(
            code(s.approve("p-kalos-fixed", none).unwrap_err()),
            (422, "choices_required")
        );
        let stray = ApproveRequest {
            choices: keep("n-kalos"),
            ..v1()
        };
        assert_eq!(
            code(s.approve("p-kalos-fixed", stray).unwrap_err()),
            (422, "choices_not_applicable")
        );
        let chosen = || ApproveRequest {
            choices: keep("r-kalos"),
            ..v1()
        };
        let message = s.approve("p-kalos-fixed", chosen()).ok().unwrap();
        assert_eq!(message, "Approved: weekly timing change #d4e6f8a0.");
        let timing = s.fixed.iter().find(|f| f.id == "f-kalos").unwrap();
        assert_eq!((timing.weekday, timing.time.as_str()), (5, "22:30"));
        // The kept run stays; the unamended one follows the timing.
        let run = |id: &str| s.runs.iter().find(|r| r.id == id).unwrap();
        assert_eq!(run("r-kalos").time.as_deref(), Some("22:00"));
        assert_eq!(
            (run("n-kalos").day, run("n-kalos").time.as_deref()),
            (2, Some("22:30"))
        );
        // A repeat answers the first result; other choices are a different decision.
        assert_eq!(s.approve("p-kalos-fixed", chosen()).ok().unwrap(), message);
        let other = ApproveRequest {
            choices: Some([("r-kalos".to_owned(), "update".to_owned())].into()),
            ..v1()
        };
        assert_eq!(
            code(s.approve("p-kalos-fixed", other).unwrap_err()),
            (422, "idempotency_mismatch")
        );

        s.approve("p-carling-link", v1()).ok().unwrap();
        assert!(run_has(&s, "r-carling", "1007"));
        s.approve("p-limbo-new", v1()).ok().unwrap();
        assert!(s.fixed.iter().any(|f| f.weekday == 6 && f.time == "21:00"));
    }

    fn run_has(s: &crate::mock::Store, run: &str, member: &str) -> bool {
        s.runs
            .iter()
            .find(|r| r.id == run)
            .unwrap()
            .participants
            .iter()
            .any(|p| p.id == member)
    }

    #[test]
    fn proposals_need_a_discord_session_and_take_one_atomic_edit() {
        let mut s = store();
        s.set_session("token");
        assert_eq!(
            code(s.approve("p-bm-move", v1()).unwrap_err()),
            (403, "discord_session_required")
        );
        assert_eq!(
            code(s.reject("p-bm-move", RejectRequest::default()).unwrap_err()),
            (403, "discord_session_required")
        );
        // Requests work for every admin session.
        let reason = || RejectRequest {
            version: Some(1),
            reason: Some("Not this week.".into()),
        };
        assert!(s.reject("p-fa-request", reason()).is_ok());
        s.set_session("discord");

        let choices = ApproveRequest {
            choices: Some(BTreeMap::new()),
            ..v1()
        };
        assert_eq!(
            code(s.approve("p-bm-move", choices).unwrap_err()),
            (422, "choices_not_applicable")
        );
        let half = ApproveRequest {
            day: Some(2),
            ..v1()
        };
        assert!(matches!(
            s.approve("p-bm-move", half),
            Err(MoveError::Invalid(_))
        ));
        let bad = ApproveRequest {
            day: Some(7),
            time: Some("21:00".into()),
            ..v1()
        };
        assert!(matches!(
            s.approve("p-bm-move", bad),
            Err(MoveError::Invalid(_))
        ));
        let edit = || ApproveRequest {
            day: Some(6),
            time: Some("22:30".into()),
            ..v1()
        };
        let message = s.approve("p-bm-move", edit()).ok().unwrap();
        let bm = s.runs.iter().find(|r| r.id == "r-bm").unwrap();
        assert_eq!((bm.day, bm.time.as_deref()), (6, Some("22:30")));
        assert_eq!(s.approve("p-bm-move", edit()).ok().unwrap(), message);
        assert_eq!(
            code(s.approve("p-bm-move", v1()).unwrap_err()),
            (422, "idempotency_mismatch")
        );
        assert_eq!(
            code(s.reject("p-bm-move", RejectRequest::default()).unwrap_err()),
            (409, "stale")
        );

        let noted = RejectRequest {
            reason: Some("no".into()),
            ..RejectRequest::default()
        };
        assert_eq!(
            code(s.reject("p-limbo-add", noted).unwrap_err()),
            (422, "reason_not_applicable")
        );
        let rejected = s
            .reject("p-limbo-add", RejectRequest::default())
            .ok()
            .unwrap();
        assert_eq!(rejected, "Rejected: new run #c8e0a2b4.");
        assert_eq!(
            s.reject("p-limbo-add", RejectRequest::default())
                .ok()
                .unwrap(),
            rejected
        );
    }

    #[test]
    fn member_rejections_need_a_version_and_a_reason() {
        let mut s = store();
        let none = RejectRequest::default;
        assert_eq!(
            code(s.reject("p-kalos-expired", none()).unwrap_err()),
            (422, "version_required")
        );
        let v = || RejectRequest {
            version: Some(1),
            ..none()
        };
        assert_eq!(
            code(s.reject("p-kalos-expired", v()).unwrap_err()),
            (422, "reason_required")
        );
        let long = RejectRequest {
            reason: Some("x".repeat(501)),
            ..v()
        };
        assert_eq!(
            code(s.reject("p-kalos-expired", long).unwrap_err()),
            (422, "reason_invalid")
        );
        let ok = || RejectRequest {
            reason: Some("It expired.".into()),
            ..v()
        };
        assert!(s.reject("p-kalos-expired", ok()).is_ok());
        assert!(
            s.reject("p-kalos-expired", ok()).is_ok(),
            "a repeat answers 200"
        );
        let other = RejectRequest {
            reason: Some("Other.".into()),
            ..v()
        };
        assert_eq!(
            code(s.reject("p-kalos-expired", other).unwrap_err()),
            (422, "idempotency_mismatch")
        );
    }
}
