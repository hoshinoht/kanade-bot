//! Synthetic boss weeks (fake names only) and their in-memory mutation rules.

pub mod catalog;
mod chat;
pub mod clock;
mod config;
pub mod dto;
pub mod extractions;
mod fixed;
pub mod history;
pub mod inbox;
pub mod knowledge;
mod limits;
pub mod logfilter;
mod model_context;
mod people;
mod reminders;
mod seed;

use catalog::{BossRef, Catalog};
use clock::{DOW, clock, countdown, iso_date, iso_now, local_now, minutes, valid_time, week_start};
use dto::*;
use seed::{Fixed, MemberSeed, Rec};

#[derive(Clone)]
pub enum MoveError {
    NotFound,
    Stale,
    Invalid(String),
    /// A typed refusal the backend contract names: HTTP status, error code, message.
    Coded(u16, &'static str, String),
}

impl MoveError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::Stale => write!(f, "stale"),
            Self::Invalid(message) | Self::Coded(_, _, message) => write!(f, "{message}"),
        }
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const LIVE: [&str; 3] = ["planned", "confirmed", "at_risk"];

/// Mutable per-member state layered over the seed.
pub struct MemberState {
    seed: MemberSeed,
    ping_level: &'static str,
    persona: Option<&'static str>,
    aliases: Vec<String>,
}

pub struct Store {
    runs: Vec<Rec>,
    fixed: Vec<Fixed>,
    members: Vec<MemberState>,
    history: Vec<history::Record>,
    proposals: Vec<inbox::Proposal>,
    /// Closed inbox items, so a repeated decision answers as the first did.
    decided: Vec<inbox::Decided>,
    /// How the mock's admin signed in: `discord`, `token` or `tailscale`.
    session: &'static str,
    /// Signed out: every admin route but sign-in answers `401 unauthenticated`.
    signed_in: bool,
    /// The next Discord sign-in fails with this `login_error` code.
    discord_error: Option<&'static str>,
    jobs: Vec<extractions::Job>,
    limit_resets: Vec<&'static str>,
    /// The newest manually posted digest week (`false` this, `true` next).
    digest_week: Option<bool>,
    config: config::Config,
    version: u64,
    next_id: u32,
    catalog: Catalog,
}

impl Store {
    pub fn new(catalog: Catalog) -> Self {
        let mut store = Self {
            runs: Vec::new(),
            fixed: Vec::new(),
            members: Vec::new(),
            history: Vec::new(),
            proposals: Vec::new(),
            decided: Vec::new(),
            session: "discord",
            signed_in: true,
            discord_error: None,
            jobs: Vec::new(),
            limit_resets: Vec::new(),
            digest_week: None,
            config: config::defaults(),
            version: 1,
            next_id: 1,
            catalog,
        };
        store.reset();
        store
    }

    pub fn reset(&mut self) {
        self.runs = seed::runs();
        self.fixed = seed::fixed_runs();
        self.members = seed::members()
            .into_iter()
            .map(|seed| MemberState {
                ping_level: seed.ping_level,
                persona: seed.persona,
                aliases: seed.aliases.iter().map(|a| (*a).to_owned()).collect(),
                seed,
            })
            .collect();
        self.version = 1;
        self.next_id = 1;
        self.proposals = inbox::seed();
        self.decided.clear();
        self.session = "discord";
        self.signed_in = true;
        self.discord_error = None;
        self.jobs.clear();
        self.limit_resets.clear();
        self.digest_week = None;
        self.config = config::defaults();
        self.seed_history();
    }

    fn fresh_id(&mut self, prefix: &str) -> (String, String) {
        self.next_id += 1;
        (
            format!("{prefix}-new{}", self.next_id),
            format!("{:08x}", 0xbeef_0000_u32 + self.next_id),
        )
    }

    fn start(next: bool) -> i64 {
        week_start(local_now().0) + if next { 7 } else { 0 }
    }

    /// Absolute local minute a run starts at (own-time runs count from midnight).
    fn start_minute(rec: &Rec) -> i64 {
        (Self::start(rec.next_week) + i64::from(rec.day)) * 1440
            + rec.time.as_deref().map_or(0, minutes)
    }

    fn now_minute() -> i64 {
        let (today, minute) = local_now();
        today * 1440 + minute
    }

    /// "Tue 29 Sep 21:00" for an absolute local minute.
    fn when(minute: i64) -> String {
        let day = minute.div_euclid(1440);
        let date = iso_date(day);
        let month = MONTHS[date[5..7].parse::<usize>().unwrap_or(1) - 1];
        format!(
            "{} {} {month} {}",
            DOW[day.rem_euclid(7) as usize],
            &date[8..10],
            clock(minute.rem_euclid(1440))
        )
    }

    fn cards(rec: &Rec) -> Vec<Card> {
        if rec.status == "cancelled" {
            return Vec::new();
        }
        let day = (Self::start(rec.next_week) + i64::from(rec.day)) * 1440;
        let now = Self::now_minute();
        let mut plan = vec![("morning", 9 * 60)];
        if let (Some(t), false) = (rec.time.as_deref(), rec.status == "otot") {
            plan.push(("T-1h", minutes(t) - 60));
            plan.push(("T-15m", minutes(t) - 15));
        }
        plan.into_iter()
            .map(|(label, at)| {
                let due = day + at;
                // One retired-without-posting card, so the stale state is represented.
                let state = match (due <= now, rec.id == "r-baldrix" && label == "T-1h") {
                    (true, true) => "skipped",
                    (true, false) => "posted",
                    _ => "queued",
                };
                Card {
                    label,
                    state,
                    at: clock(at),
                    url: (state == "posted").then(|| {
                        format!("https://discord.com/channels/0/0/{}{}", rec.short_id, at)
                    }),
                }
            })
            .collect()
    }

    fn fixed_of(&self, rec: &Rec) -> Option<&Fixed> {
        let id = rec.fixed_id.as_deref()?;
        self.fixed.iter().find(|f| f.id == id)
    }

    fn roster_change(&self, rec: &Rec) -> Option<RosterChange> {
        let fixed = self.fixed_of(rec)?;
        let named = |id: &str| {
            seed::member_name(id).map(|(id, name)| Named {
                id: id.into(),
                name: name.into(),
            })
        };
        let out = fixed
            .participants
            .iter()
            .filter(|id| !rec.participants.iter().any(|p| p.id == **id))
            .filter_map(|id| named(id))
            .collect();
        let added = rec
            .participants
            .iter()
            .filter(|p| !fixed.participants.contains(&p.id))
            .filter_map(|p| named(p.id))
            .collect();
        Some(RosterChange { out, added })
    }

    fn amended(&self, rec: &Rec) -> bool {
        let Some(fixed) = self.fixed_of(rec) else {
            return false;
        };
        let moved = rec.day != seed::day_of(fixed.weekday)
            || rec.time.as_deref() != Some(fixed.time.as_str());
        let roster = self
            .roster_change(rec)
            .is_some_and(|c| !c.out.is_empty() || !c.added.is_empty());
        moved || roster
    }

    fn bosses(&self, list: &[BossRef]) -> Vec<Boss> {
        list.iter()
            .map(|b| self.catalog.boss(&b.token, b.key, b.difficulty))
            .collect()
    }

    fn minutes(&self, list: &[BossRef]) -> u32 {
        list.iter()
            .map(|boss| {
                self.config
                    .run_lengths
                    .overrides
                    .iter()
                    .find(|override_| {
                        override_.boss == boss.key && override_.difficulty == boss.difficulty
                    })
                    .map_or(self.config.run_lengths.default_minutes, |override_| {
                        override_.minutes
                    })
            })
            .sum()
    }

    fn dto(&self, rec: &Rec) -> Run {
        Run {
            id: rec.id.clone(),
            short_id: rec.short_id.clone(),
            day: rec.day,
            time: rec.time.clone(),
            minutes: self.minutes(&rec.bosses),
            status: rec.status,
            bosses: self.bosses(&rec.bosses),
            tally: Tally {
                on: rec
                    .participants
                    .iter()
                    .filter(|p| p.answer == "yes")
                    .count(),
                total: rec.participants.len(),
            },
            participants: rec.participants.clone(),
            party: rec.channel,
            channel_id: rec.channel,
            channel: seed::channel(rec.channel).map_or(rec.channel, |c| c.1),
            cards: Self::cards(rec),
            fixed_id: rec.fixed_id.clone(),
            amended: self.amended(rec),
            roster_change: self
                .roster_change(rec)
                .filter(|c| !c.out.is_empty() || !c.added.is_empty()),
        }
    }

    fn days(next: bool) -> Vec<WeekDay> {
        let start = Self::start(next);
        let today = local_now().0;
        (0..7u8)
            .map(|i| WeekDay {
                index: i,
                date: iso_date(start + i64::from(i)),
                dow: DOW[usize::from(i)],
                is_reset: i == 0,
                is_today: start + i64::from(i) == today,
            })
            .collect()
    }

    pub fn week(&self, next: bool) -> Week {
        Week {
            starts: iso_date(Self::start(next)),
            timezone: "Asia/Kuala_Lumpur",
            reset: "Thu 00:00",
            days: Self::days(next),
            runs: self
                .runs
                .iter()
                .filter(|r| r.next_week == next)
                .map(|r| self.dto(r))
                .collect(),
            generated_at: iso_now(),
            version: self.version,
        }
    }

    pub fn public_week(&self, next: bool) -> PublicWeek {
        let week = self.week(next);
        PublicWeek {
            starts: week.starts,
            timezone: week.timezone,
            reset: week.reset,
            days: week.days,
            runs: week
                .runs
                .into_iter()
                .map(|r| PublicRun {
                    id: r.id,
                    day: r.day,
                    time: r.time,
                    status: r.status,
                    bosses: r.bosses,
                    tally: r.tally,
                })
                .collect(),
            generated_at: week.generated_at,
        }
    }

    pub fn stats(&self, next: bool) -> Stats {
        let per_day = (0..7u8)
            .map(|day| {
                let people = self
                    .runs
                    .iter()
                    .filter(|r| r.next_week == next && r.day == day)
                    .flat_map(|r| &r.participants);
                let (mut answered, mut waiting) = (0, 0);
                for p in people {
                    if p.answer == "waiting" {
                        waiting += 1
                    } else {
                        answered += 1
                    }
                }
                DayStat {
                    day,
                    answered,
                    waiting,
                }
            })
            .collect();
        Stats { per_day }
    }

    pub fn summary(&self) -> Summary {
        let now = Self::now_minute();
        let ahead = || {
            self.runs
                .iter()
                .filter(|r| LIVE.contains(&r.status) && Self::start_minute(r) > now)
        };
        let next = ahead()
            .filter(|r| r.time.is_some())
            .min_by_key(|r| Self::start_minute(r))
            .map(|r| {
                let run = self.dto(r);
                NextRun {
                    run_id: run.id,
                    bosses: run
                        .bosses
                        .iter()
                        .map(|b| b.token.as_str())
                        .collect::<Vec<_>>()
                        .join(" + "),
                    when: Self::when(Self::start_minute(r)),
                    countdown: countdown(Self::start_minute(r) - now),
                    on: run.tally.on,
                    total: run.tally.total,
                }
            });
        Summary {
            next,
            unanswered: ahead()
                .flat_map(|r| &r.participants)
                .filter(|p| p.answer == "waiting")
                .count(),
            inbox: self.proposals.len(),
            model: Model {
                busy: true,
                holder: Some("extractor"),
            },
        }
    }

    pub fn channels() -> Vec<Named> {
        seed::CHANNELS
            .iter()
            .map(|&(id, name, _)| Named {
                id: id.into(),
                name: name.into(),
            })
            .collect()
    }

    /// Finds a run, checks the week version, applies `change`, bumps the version.
    fn mutate(
        &mut self,
        id: &str,
        version: u64,
        change: impl FnOnce(&mut Rec, &[Fixed]) -> Result<(), MoveError>,
    ) -> Result<RunResult, MoveError> {
        if version != self.version {
            return Err(MoveError::Stale);
        }
        let index = self
            .runs
            .iter()
            .position(|r| r.id == id)
            .ok_or(MoveError::NotFound)?;
        change(&mut self.runs[index], &self.fixed)?;
        self.version += 1;
        Ok(RunResult {
            run: self.dto(&self.runs[index]),
            version: self.version,
        })
    }

    pub fn move_run(&mut self, id: &str, req: MoveRequest) -> Result<MoveResult, MoveError> {
        if req.day > 6 {
            return Err(MoveError::invalid("A boss week has seven days."));
        }
        let mut previous = Previous { day: 0, time: None };
        let result = self.mutate(id, req.version, |run, _| {
            if matches!(run.status, "done" | "cancelled") {
                return Err(MoveError::invalid(
                    "Finished and cancelled runs stay where they were.",
                ));
            }
            match (&req.time, run.status) {
                (None, s) if s != "otot" => {
                    return Err(MoveError::invalid("A scheduled run needs a time."));
                }
                (Some(t), _) if !valid_time(t) => {
                    return Err(MoveError::invalid("Times are HH:MM, 00:00 to 23:59."));
                }
                _ => {}
            }
            previous = Previous {
                day: run.day,
                time: run.time.clone(),
            };
            run.day = req.day;
            if run.status != "otot" {
                run.time = req.time;
            }
            Ok(())
        })?;
        Ok(MoveResult {
            run: result.run,
            previous,
            version: result.version,
        })
    }

    /// Planner slot exchange: both rows change under one version bump, or neither.
    pub fn swap_runs(&mut self, id: &str, req: SwapRequest) -> Result<SwapResult, MoveError> {
        if id == req.with {
            return Err(MoveError::invalid("A run cannot be swapped with itself."));
        }
        if req.version != self.version {
            return Err(MoveError::Stale);
        }
        let first = self
            .runs
            .iter()
            .position(|run| run.id == id)
            .ok_or(MoveError::NotFound)?;
        let second = self
            .runs
            .iter()
            .position(|run| run.id == req.with)
            .ok_or(MoveError::NotFound)?;
        let (left, right) = (&self.runs[first], &self.runs[second]);
        if left.next_week != right.next_week {
            return Err(MoveError::invalid(
                "Runs can only swap within the same boss week.",
            ));
        }
        if matches!(left.status, "done" | "cancelled")
            || matches!(right.status, "done" | "cancelled")
        {
            return Err(MoveError::invalid(
                "Finished and cancelled runs stay where they were.",
            ));
        }
        let (left_day, left_time) = (left.day, left.time.clone());
        let (right_day, right_time) = (right.day, right.time.clone());
        self.runs[first].day = right_day;
        self.runs[second].day = left_day;
        if self.runs[first].status != "otot" && self.runs[second].status != "otot" {
            self.runs[first].time = right_time;
        }
        if self.runs[first].status != "otot" && self.runs[second].status != "otot" {
            self.runs[second].time = left_time;
        }
        self.version += 1;
        Ok(SwapResult {
            runs: [self.dto(&self.runs[first]), self.dto(&self.runs[second])],
            version: self.version,
        })
    }

    pub fn set_status(&mut self, id: &str, req: StatusRequest) -> Result<RunResult, MoveError> {
        let status = match req.status.as_str() {
            "planned" => "planned",
            "confirmed" => "confirmed",
            "otot" => "otot",
            "done" => "done",
            "cancelled" => "cancelled",
            _ => {
                return Err(MoveError::invalid(
                    "Pick one of planned, confirmed, own time, done or cancelled.",
                ));
            }
        };
        self.mutate(id, req.version, |run, _| {
            run.status = status;
            Ok(())
        })
    }

    pub fn rsvp(&mut self, id: &str, req: RsvpRequest) -> Result<RunResult, MoveError> {
        let answer = match req.answer.as_str() {
            "yes" => "yes",
            "no" => "no",
            "clear" => "waiting",
            _ => return Err(MoveError::invalid("An answer is on, out or clear.")),
        };
        self.mutate(id, req.version, |run, _| {
            let person = run.participants.iter_mut().find(|p| p.id == req.member_id);
            let person = person.ok_or(MoveError::invalid("That member is not on this run."))?;
            person.answer = answer;
            // v4: a no on a live run puts it at risk until someone settles it.
            if answer == "no" && matches!(run.status, "planned" | "confirmed") {
                run.status = "at_risk";
            }
            Ok(())
        })
    }

    pub fn participants(
        &mut self,
        id: &str,
        req: ParticipantsRequest,
    ) -> Result<RunResult, MoveError> {
        self.mutate(id, req.version, |run, _| {
            if let Some(remove) = req.remove.as_deref() {
                run.participants.retain(|p| p.id != remove);
            }
            if let Some(add) = req.add.as_deref() {
                let (id, name) =
                    seed::member_name(add).ok_or(MoveError::invalid("No such member."))?;
                if !run.participants.iter().any(|p| p.id == id) {
                    run.participants.push(Participant {
                        id,
                        name,
                        answer: "waiting",
                    });
                }
            }
            Ok(())
        })
    }

    /// v5: put an amended run back on its weekly timing (day, time and roster).
    pub fn reset_to_fixed(&mut self, id: &str, version: u64) -> Result<RunResult, MoveError> {
        self.mutate(id, version, |run, fixed| {
            let id = run.fixed_id.as_deref().ok_or(MoveError::invalid(
                "This run has no weekly timing to go back to.",
            ))?;
            let timing = fixed
                .iter()
                .find(|f| f.id == id && !f.retired)
                .ok_or(MoveError::invalid("Its weekly timing was retired."))?;
            if matches!(run.status, "done" | "cancelled") {
                return Err(MoveError::invalid(
                    "Finished and cancelled runs stay as they were.",
                ));
            }
            fixed::apply_timing(run, timing);
            Ok(())
        })
    }

    pub fn ping(&self, id: &str) -> Result<String, MoveError> {
        let run = self
            .runs
            .iter()
            .find(|r| r.id == id)
            .ok_or(MoveError::NotFound)?;
        let tokens: Vec<&str> = run.bosses.iter().map(|b| b.token.as_str()).collect();
        let channel = seed::channel(run.channel).map_or(run.channel, |c| c.1);
        Ok(format!(
            "Posted the morning card for {} in {channel} as a TEST message.",
            tokens.join(" + ")
        ))
    }

    pub fn public_portal(&self) -> bool {
        self.config.public_portal
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

#[cfg(test)]
mod tests {
    use super::{Store, catalog::Catalog};
    use std::path::PathBuf;

    pub fn store() -> Store {
        Store::new(Catalog::new(PathBuf::from("/nonexistent")))
    }

    #[test]
    fn public_projection_carries_no_people_party_or_version() {
        let json = serde_json::to_string(&store().public_week(false)).unwrap();
        for field in [
            "participants",
            "party",
            "version",
            "answer",
            "Asahi",
            "1001",
            "cards",
            "short_id",
            "fixed_id",
            "roster",
            "minutes",
        ] {
            assert!(!json.contains(field), "{field} leaked into the public week");
        }
        assert!(json.contains("\"tally\""));
    }

    #[test]
    fn duplicate_display_names_have_distinct_member_ids() {
        let json = serde_json::to_string(&store().week(false)).unwrap();
        assert!(json.contains(r#"{"id":"1002","name":"Ren""#));
        assert!(json.contains(r#"{"id":"1013","name":"Ren""#));
    }

    #[test]
    fn absent_art_is_null_not_a_url() {
        let json = serde_json::to_string(&store().week(false)).unwrap();
        assert!(json.contains(r#""portrait":null"#));
        assert!(!json.contains("/art/"));
    }

    #[test]
    fn present_art_is_a_same_origin_url() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/e2e/fixtures/boss");
        let json = serde_json::to_string(&Store::new(Catalog::new(root)).week(false)).unwrap();
        assert!(json.contains(r#""art":"/art/entry/Carling""#));
        assert!(json.contains(r#""portrait":"/art/portraits/Limbo""#));
    }

    #[test]
    fn a_no_puts_a_live_run_at_risk() {
        let mut s = store();
        let v = s.version;
        let req = super::RsvpRequest {
            member_id: "1008".into(),
            answer: "no".into(),
            version: v,
        };
        let result = s.rsvp("r-limbo", req).ok().unwrap();
        assert_eq!(result.run.status, "at_risk");
        assert_eq!(result.version, v + 1);
    }

    #[test]
    fn amended_runs_and_roster_changes_are_derived_from_the_timing() {
        let week = store().week(false);
        let run = |id: &str| week.runs.iter().find(|r| r.id == id).unwrap();
        assert!(run("r-kalos").amended, "22:00 against a 21:30 timing");
        assert!(run("r-carling").amended, "one extra member");
        let change = run("r-carling").roster_change.as_ref().unwrap();
        assert_eq!(change.added.len(), 1);
        assert_eq!(change.added[0].id, "1013");
        assert!(!run("r-limbo").amended);
        assert!(!run("r-bellona").amended, "no timing at all");
    }

    #[test]
    fn reset_to_fixed_restores_day_time_and_roster() {
        let mut s = store();
        let v = s.version;
        let result = s.reset_to_fixed("r-kalos", v).ok().unwrap();
        assert_eq!(result.run.time.as_deref(), Some("21:30"));
        assert!(!result.run.amended);
        let carling = s.reset_to_fixed("r-carling", v + 1).ok().unwrap();
        assert!(carling.run.participants.iter().all(|p| p.id != "1013"));
        assert!(s.reset_to_fixed("r-bellona", v + 2).is_err());
    }
}
