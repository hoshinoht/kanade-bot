//! The Inbox (v4 /inbox): proposed changes read from party chat, plus v5
//! self-service requests members confirm, signed in with Discord, from a
//! pre-filled link (run id and proposed time only; no secret in the link).

use super::dto::{Boss, Named, Participant};
use super::history::Actor;
use super::seed::{self, Rec};
use super::{MoveError, Store};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
pub struct Evidence {
    pub id: String,
    pub author: String,
    pub at: String,
    pub content: Option<String>,
    pub url: Option<String>,
    /// The message is no longer stored (deleted or pruned).
    pub missing: bool,
}

#[derive(Clone, Serialize)]
pub struct SelfService {
    pub member: Named,
    /// Where the member opened the link from.
    pub via: &'static str,
    pub note: Option<String>,
}

/// A member request (v5 "requests as PRs"): who sent it, from where, and the
/// state the backend reports about it.
#[derive(Clone)]
pub struct Request {
    pub member: &'static str,
    pub via: &'static str,
    pub note: &'static str,
    /// The requester is frozen (their requests are held for an admin).
    pub frozen: bool,
    /// Hour (from the boss week's start) the request expires at.
    pub expires_hour: Option<i64>,
    /// The run's day and time the member saw when they asked; a different
    /// value now is a conflict the admin has to acknowledge.
    pub base: Option<(u8, Option<&'static str>)>,
    /// The generated one-line summary the public app shows the member.
    pub public_summary: &'static str,
}

#[derive(Clone)]
pub struct Proposal {
    pub id: &'static str,
    pub short_id: &'static str,
    pub kind: &'static str,
    pub run_id: Option<&'static str>,
    /// For moves and adds: target boss-week day and time.
    pub day: u8,
    pub time: Option<&'static str>,
    pub answer: Option<(&'static str, &'static str)>,
    pub bosses: Vec<&'static str>,
    pub participants: Vec<&'static str>,
    pub confidence: Option<f32>,
    pub is_question: bool,
    pub channel: &'static str,
    pub read_at_hour: i64,
    pub summary: &'static str,
    pub evidence: Vec<(&'static str, &'static str, i64, Option<&'static str>)>,
    pub request: Option<Request>,
    /// For `fix`: the weekly timing the request changes, and its new weekday
    /// (0 = Monday) and time.
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
    /// Badges: `conflict`, `expired`, `requester_frozen`, `no_effect`.
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
    pub card_url: Option<String>,
    pub self_service: Option<SelfService>,
}

#[derive(Deserialize, Default)]
pub struct ApproveRequest {
    /// Edit, then approve: a new boss-week day and time for a move.
    pub day: Option<u8>,
    pub time: Option<String>,
    /// The version the admin reviewed; a different one is 409 stale.
    pub version: Option<u32>,
    /// Weekly-timing changes: run id -> `update` | `keep`.
    pub choices: Option<std::collections::BTreeMap<String, String>>,
    /// Approve over reported conflicts (the admin reviewed them).
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize, Default)]
pub struct RejectRequest {
    pub version: Option<u32>,
    /// Required (1-500 characters) for member requests; the member is told.
    pub reason: Option<String>,
}

const fn request(
    member: &'static str,
    via: &'static str,
    note: &'static str,
    public_summary: &'static str,
) -> Request {
    Request {
        member,
        via,
        note,
        frozen: false,
        expires_hour: None,
        base: None,
        public_summary,
    }
}

pub fn seed() -> Vec<Proposal> {
    let base = Proposal {
        id: "",
        short_id: "",
        kind: "move",
        run_id: None,
        day: 0,
        time: None,
        answer: None,
        bosses: vec![],
        participants: vec![],
        confidence: None,
        is_question: false,
        channel: "",
        read_at_hour: 100,
        summary: "",
        evidence: vec![],
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
            read_at_hour: 108,
            summary: "Minato asks to push Black Mage to Wednesday; Kaito agrees.",
            evidence: vec![
                ("1012", "tue cannot, wed same time ok?", 107, Some("m1")),
                ("1009", "wed ok for me", 108, Some("m2")),
                ("1008", "", 108, None),
            ],
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
            summary: "Mika floats a Normal Limbo run on Saturday; nobody has confirmed.",
            evidence: vec![("1003", "nlimbo sat 9pm anyone?", 99, Some("m3"))],
            ..base.clone()
        },
        // The redirect feature (case a): Ren confirmed a move of a run they
        // are on from the chatbot's pre-filled link; others on the run
        // approve it through an admin.
        Proposal {
            id: "p-carling-link",
            short_id: "b3d5f7a9",
            run_id: Some("r-carling"),
            day: 6,
            time: Some("22:00"),
            bosses: vec!["HCarling", "HStar"],
            participants: vec!["1013"],
            channel: "hstar-party",
            read_at_hour: 110,
            summary: "Ren confirmed moving HCarling + HStar to Wednesday 22:00 from a pre-filled link.",
            request: Some(Request {
                base: Some((5, Some("22:00"))),
                expires_hour: Some(24 * 7),
                ..request(
                    "1013",
                    "the chatbot's reply",
                    "Others are on this run, so it waits for approval.",
                    "Move HCarling + HStar to Wed 22:00",
                )
            }),
            ..base.clone()
        },
        // Case c: a weekly-timing change through the pre-filled request form.
        Proposal {
            id: "p-limbo-fixed",
            short_id: "d4e6f8a0",
            kind: "fix",
            run_id: Some("r-limbo"),
            day: 2,
            time: Some("22:30"),
            bosses: vec!["HLimbo"],
            participants: vec!["1003"],
            channel: "limbo-trio",
            read_at_hour: 111,
            summary: "Mika asks to move the weekly HLimbo timing to Saturdays 22:30.",
            request: Some(request(
                "1003",
                "the extractor's request-form link",
                "Weekly timings always need an admin.",
                "Weekly HLimbo: Fridays 23:30 to Saturdays 22:30",
            )),
            timing: Some(("f-limbo", 5, "22:30")),
            ..base.clone()
        },
        // Asked while FA was on Monday; the run has moved since: a conflict.
        Proposal {
            id: "p-fa-request",
            short_id: "e5f7a9b1",
            run_id: Some("r-fa"),
            day: 5,
            time: Some("20:30"),
            bosses: vec!["HFA"],
            participants: vec!["1011"],
            channel: "fa-night",
            read_at_hour: 112,
            summary: "Hotaru asks to move HFA to Tuesday 20:30.",
            request: Some(Request {
                base: Some((4, Some("19:30"))),
                ..request(
                    "1011",
                    "the chatbot's reply",
                    "Asked before the run last moved.",
                    "Move HFA to Tue 20:30",
                )
            }),
            ..base.clone()
        },
        // Expired before anyone looked: it can only be rejected.
        Proposal {
            id: "p-kalos-expired",
            short_id: "f6a8b0c2",
            run_id: Some("r-kalos"),
            day: 1,
            time: Some("23:00"),
            bosses: vec!["XKalos"],
            participants: vec!["1002"],
            channel: "kalos-four",
            read_at_hour: 20,
            summary: "Ren asked to push XKalos to Friday 23:00.",
            request: Some(Request {
                expires_hour: Some(-12),
                ..request(
                    "1002",
                    "the public week page",
                    "Nobody answered before it expired.",
                    "Move XKalos to Fri 23:00",
                )
            }),
            ..base.clone()
        },
        // Already where the member asked for it, and sent by a frozen member.
        Proposal {
            id: "p-jupiter-same",
            short_id: "a9b1c3d5",
            run_id: Some("r-jupiter"),
            day: 4,
            time: Some("21:00"),
            bosses: vec!["HJupiter"],
            participants: vec!["1010"],
            channel: "jupiter-trio",
            read_at_hour: 113,
            summary: "Rin asks for HJupiter on Monday 21:00, where it already is.",
            request: Some(Request {
                frozen: true,
                ..request(
                    "1010",
                    "the public week page",
                    "Rin's requests are frozen after repeated no-shows.",
                    "Move HJupiter to Mon 21:00",
                )
            }),
            ..base
        },
    ]
}

fn label(kind: &str) -> &'static str {
    match kind {
        "move" => "Move",
        "add" => "New run",
        "cancel" => "Cancel",
        "rsvp" => "Answer",
        "sub" => "Swap",
        "otot" => "Own time",
        "split" => "Split",
        _ => "Weekly timing",
    }
}

fn coded(status: u16, code: &'static str, message: impl Into<String>) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

const WEEKDAY_NAMES: [&str; 7] = [
    "Mondays",
    "Tuesdays",
    "Wednesdays",
    "Thursdays",
    "Fridays",
    "Saturdays",
    "Sundays",
];

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
        match (p.kind, run) {
            ("move", Some(r)) => {
                no_effect = r.day == p.day && r.time.as_deref() == p.time;
                changes.push(Change {
                    field: "when",
                    from: Self::when(Self::start_minute(r)),
                    to: Self::when(Self::target_minute(p)),
                });
                if let Some((day, time)) = p.request.as_ref().and_then(|q| q.base)
                    && (day != r.day || time != r.time.as_deref())
                {
                    conflicts.push(Conflict {
                        field: "when",
                        expected: Self::slot_when(day, time),
                        found: Self::when(Self::start_minute(r)),
                    });
                }
            }
            ("fix", _) => {
                if let Some((fixed_id, weekday, time)) = p.timing
                    && let Some(f) = self.fixed.iter().find(|f| f.id == fixed_id)
                {
                    no_effect = f.weekday == weekday && f.time == time;
                    changes.push(Change {
                        field: "weekly timing",
                        from: format!("{} {}", WEEKDAY_NAMES[usize::from(f.weekday)], f.time),
                        to: format!("{} {time}", WEEKDAY_NAMES[usize::from(weekday)]),
                    });
                    choices = Some(
                        self.runs
                            .iter()
                            .filter(|r| {
                                r.fixed_id.as_deref() == Some(fixed_id)
                                    && matches!(r.status, "planned" | "confirmed" | "at_risk")
                            })
                            .map(|r| Choice {
                                run_id: r.id.clone(),
                                label: if r.next_week {
                                    "Next week"
                                } else {
                                    "This week"
                                }
                                .into(),
                                when: Self::when(Self::start_minute(r)),
                                amended: seed::day_of(f.weekday) != r.day
                                    || r.time.as_deref() != Some(f.time.as_str()),
                            })
                            .collect(),
                    );
                }
            }
            ("add", _) => changes.push(Change {
                field: "new run",
                from: "—".into(),
                to: Self::when(Self::target_minute(p)),
            }),
            _ => {}
        }
        if !conflicts.is_empty() {
            flags.push("conflict");
        }
        if let Some(q) = &p.request {
            if q.expires_hour
                .is_some_and(|h| Self::at_hour(h) <= Self::now_minute())
            {
                flags.push("expired");
            }
            if q.frozen {
                flags.push("requester_frozen");
            }
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
                let from_when = run
                    .filter(|_| matches!(p.kind, "move" | "fix"))
                    .map(|r| Self::when(Self::start_minute(r)));
                let when = match (p.kind, run) {
                    ("rsvp", Some(r)) => Self::when(Self::start_minute(r)),
                    _ => Self::when(Self::target_minute(p)),
                };
                let (flags, preview, choices) = self.review(p);
                let member = p.request.as_ref().is_some();
                ProposalDto {
                    id: p.id,
                    short_id: p.short_id,
                    kind: p.kind,
                    kind_label: label(p.kind),
                    source: if member { "self_service" } else { "extraction" },
                    tab: if member { "self_service" } else { "extractor" },
                    version: p.version,
                    flags,
                    preview,
                    expires_at: p
                        .request
                        .as_ref()
                        .and_then(|q| q.expires_hour)
                        .map(|h| Self::when(Self::at_hour(h))),
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
                        .map(|(i, (who, text, hour, link))| Evidence {
                            id: format!("{}-{i}", p.short_id),
                            author: seed::member_name(who).map_or("someone", |m| m.1).into(),
                            at: Self::when(Self::at_hour(*hour)),
                            content: (!text.is_empty()).then(|| (*text).to_owned()),
                            url: link.map(|l| format!("https://discord.com/channels/0/0/{l}")),
                            missing: text.is_empty(),
                        })
                        .collect(),
                    card_url: (!member)
                        .then(|| format!("https://discord.com/channels/0/0/card-{}", p.short_id)),
                    self_service: p.request.as_ref().map(|q| SelfService {
                        member: named(q.member).unwrap_or(Named {
                            id: q.member.into(),
                            name: q.member.into(),
                        }),
                        via: q.via,
                        note: Some(q.note.into()),
                    }),
                }
            })
            .collect()
    }

    pub fn approve(
        &mut self,
        id: &str,
        req: ApproveRequest,
    ) -> Result<(String, &'static str), MoveError> {
        let index = self
            .proposals
            .iter()
            .position(|p| p.id == id)
            .ok_or(MoveError::NotFound)?;
        let p = self.proposals[index].clone();
        let (flags, _, choices) = self.review(&p);
        if flags.contains(&"expired") {
            return Err(coded(
                410,
                "expired",
                "This request expired; it can only be rejected.",
            ));
        }
        if req.version.is_some_and(|v| v != p.version) {
            return Err(coded(
                409,
                "stale",
                "The request changed since you opened it; review it again.",
            ));
        }
        let edited = req.day.is_some() || req.time.is_some();
        if flags.contains(&"no_effect") && !edited {
            return Err(coded(
                409,
                "no_effect",
                "It is already like that; nothing would change. Reject it instead.",
            ));
        }
        if flags.contains(&"conflict") && !req.force {
            return Err(coded(
                409,
                "conflicts",
                "The run moved since the member asked; review the conflict, then approve anyway.",
            ));
        }
        match (&choices, &req.choices) {
            (None, Some(_)) => {
                return Err(coded(
                    422,
                    "choices_not_applicable",
                    "Only weekly-timing changes take per-run choices.",
                ));
            }
            (Some(runs), given) if !runs.is_empty() => {
                let given = given.as_ref();
                let complete = runs.iter().all(|c| {
                    given
                        .and_then(|g| g.get(&c.run_id))
                        .is_some_and(|v| v == "update" || v == "keep")
                });
                if !complete {
                    return Err(coded(
                        422,
                        "choices_required",
                        "Choose update or keep for every run of this timing.",
                    ));
                }
            }
            _ => {}
        }
        let run_index = p
            .run_id
            .and_then(|rid| self.runs.iter().position(|r| r.id == rid));
        let surface = if p.request.is_some() {
            "admin_portal"
        } else {
            "extraction_approval"
        };
        match (p.kind, run_index) {
            ("move", Some(i)) => {
                let day = req.day.unwrap_or(p.day);
                if day > 6 {
                    return Err(MoveError::invalid("A boss week has seven days."));
                }
                let time = req.time.clone().or(p.time.map(Into::into));
                if let Some(t) = time.as_deref().filter(|t| !super::clock::valid_time(t)) {
                    return Err(MoveError::Invalid(format!("“{t}” is not HH:MM.")));
                }
                let run = &mut self.runs[i];
                run.day = day;
                run.time = time;
            }
            ("rsvp", Some(i)) => {
                let (member, answer) =
                    p.answer.ok_or(MoveError::invalid("No answer to record."))?;
                let run = &mut self.runs[i];
                let person = run
                    .participants
                    .iter_mut()
                    .find(|x| x.id == member)
                    .ok_or(MoveError::invalid("They are no longer on the run."))?;
                person.answer = answer;
                if answer == "no" && matches!(run.status, "planned" | "confirmed") {
                    run.status = "at_risk";
                }
            }
            ("fix", _) => {
                let (fixed_id, weekday, time) = p
                    .timing
                    .ok_or(MoveError::invalid("No weekly timing to change."))?;
                let fi = self
                    .fixed
                    .iter()
                    .position(|f| f.id == fixed_id && !f.retired)
                    .ok_or(MoveError::invalid("That weekly timing is retired."))?;
                self.fixed[fi].weekday = weekday;
                self.fixed[fi].time = time.into();
                for (run_id, choice) in req.choices.iter().flatten() {
                    if choice == "update"
                        && let Some(run) = self.runs.iter_mut().find(|r| &r.id == run_id)
                    {
                        super::fixed::apply_timing(run, &self.fixed[fi]);
                    }
                }
            }
            ("add", _) => {
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
                    day: p.day,
                    time: p.time.map(Into::into),
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
            _ => return Err(MoveError::invalid("That run is no longer on the board.")),
        }
        self.proposals.remove(index);
        self.version += 1;
        Ok((
            format!(
                "Approved: {} {}.",
                label(p.kind).to_lowercase(),
                p.bosses.join(" + ")
            ),
            surface,
        ))
    }

    pub fn reject(&mut self, id: &str, req: RejectRequest) -> Result<String, MoveError> {
        let index = self
            .proposals
            .iter()
            .position(|p| p.id == id)
            .ok_or(MoveError::NotFound)?;
        let p = &self.proposals[index];
        if req.version.is_some_and(|v| v != p.version) {
            return Err(coded(
                409,
                "stale",
                "The request changed since you opened it; review it again.",
            ));
        }
        let reason = req.reason.as_deref().map(str::trim).unwrap_or_default();
        if p.request.is_some() && reason.is_empty() {
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
        let p = self.proposals.remove(index);
        Ok(format!(
            "Rejected: {} {}.",
            label(p.kind).to_lowercase(),
            p.bosses.join(" + ")
        ))
    }

    pub fn approve_tracked(&mut self, id: &str, req: ApproveRequest) -> Result<String, MoveError> {
        let surface = if self
            .proposals
            .iter()
            .any(|p| p.id == id && p.request.is_some())
        {
            "admin_portal"
        } else {
            "extraction_approval"
        };
        self.tracked(Actor::admin(), surface, |s| {
            s.approve(id, req).map(|(message, _)| message)
        })
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

    #[test]
    fn member_requests_carry_flags_preview_and_choices() {
        let s = store();
        let inbox = s.inbox();
        assert!(inbox.iter().all(|p| p.kind != "rsvp"));
        let by = |id: &str| inbox.iter().find(|p| p.id == id).unwrap();
        assert_eq!(by("p-bm-move").tab, "extractor");
        assert_eq!(by("p-carling-link").tab, "self_service");
        assert!(by("p-carling-link").flags.is_empty());
        assert_eq!(by("p-fa-request").flags, vec!["conflict"]);
        assert_eq!(by("p-kalos-expired").flags, vec!["expired"]);
        assert_eq!(
            by("p-jupiter-same").flags,
            vec!["requester_frozen", "no_effect"]
        );
        assert!(by("p-jupiter-same").preview.no_effect);
        assert!(
            by("p-limbo-fixed")
                .choices
                .as_ref()
                .is_some_and(|c| !c.is_empty())
        );
        assert!(by("p-carling-link").public_summary.is_some());
    }

    #[test]
    fn approve_enforces_the_request_contract() {
        let mut s = store();
        let none = ApproveRequest::default;
        assert_eq!(
            code(s.approve("p-kalos-expired", none()).unwrap_err()),
            (410, "expired")
        );
        assert_eq!(
            code(s.approve("p-jupiter-same", none()).unwrap_err()),
            (409, "no_effect")
        );
        assert_eq!(
            code(s.approve("p-fa-request", none()).unwrap_err()),
            (409, "conflicts")
        );
        let stale = ApproveRequest {
            version: Some(9),
            ..none()
        };
        assert_eq!(
            code(s.approve("p-carling-link", stale).unwrap_err()),
            (409, "stale")
        );
        let stray = ApproveRequest {
            choices: Some(BTreeMap::new()),
            ..none()
        };
        assert_eq!(
            code(s.approve("p-carling-link", stray).unwrap_err()),
            (422, "choices_not_applicable")
        );
        assert_eq!(
            code(s.approve("p-limbo-fixed", none()).unwrap_err()),
            (422, "choices_required")
        );

        let runs: Vec<String> = s
            .inbox()
            .into_iter()
            .find(|p| p.id == "p-limbo-fixed")
            .unwrap()
            .choices
            .unwrap()
            .into_iter()
            .map(|c| c.run_id)
            .collect();
        let choices = runs
            .iter()
            .map(|r| (r.clone(), "keep".to_owned()))
            .collect();
        s.approve(
            "p-limbo-fixed",
            ApproveRequest {
                choices: Some(choices),
                ..none()
            },
        )
        .ok()
        .unwrap();
        let timing = s.fixed.iter().find(|f| f.id == "f-limbo").unwrap();
        assert_eq!((timing.weekday, timing.time.as_str()), (5, "22:30"));

        s.approve(
            "p-fa-request",
            ApproveRequest {
                force: true,
                ..none()
            },
        )
        .ok()
        .unwrap();
        s.approve(
            "p-carling-link",
            ApproveRequest {
                version: Some(1),
                ..none()
            },
        )
        .ok()
        .unwrap();
        let run = s.runs.iter().find(|r| r.id == "r-carling").unwrap();
        assert_eq!((run.day, run.time.as_deref()), (6, Some("22:00")));
    }

    #[test]
    fn member_rejections_need_a_reason() {
        let mut s = store();
        let none = RejectRequest::default;
        assert_eq!(
            code(s.reject("p-kalos-expired", none()).unwrap_err()),
            (422, "reason_required")
        );
        let long = RejectRequest {
            reason: Some("x".repeat(501)),
            ..none()
        };
        assert_eq!(
            code(s.reject("p-kalos-expired", long).unwrap_err()),
            (422, "reason_invalid")
        );
        let ok = RejectRequest {
            reason: Some("It expired.".into()),
            ..none()
        };
        assert!(s.reject("p-kalos-expired", ok).is_ok());
        // Extractor proposals keep v4's reason-free reject.
        assert!(s.reject("p-limbo-add", none()).is_ok());
    }
}
