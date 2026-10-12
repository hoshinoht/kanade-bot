//! Lookup jobs, one at a time (the sidecar serves one lookup and takes up to
//! 120 s), and their in-memory cache: 24 hours, keyed by the sorted set of
//! IGNs, lost on restart and never written anywhere. Jobs are kept for 24
//! hours too, one per target (a new lookup of the same characters or run
//! replaces the old job), so an admin who closed the page finds the result
//! (or the error) when they come back. Memory is bounded by [`MAX_JOBS`]
//! targets and [`MAX_CACHED`] IGN sets; only past those is the oldest
//! dropped early. The running lookup is joined on shutdown ([`Estimates::stop`]).
//!
//! One character is one `ign=` lookup. A party (a run, or several
//! characters) is looked up one character at a time first, so the job can
//! report progress and every finished character stays cached if a later one
//! fails, then once as `igns=` for MapleScouter's party total, which its own
//! short cache answers without rendering again. No total is made from a
//! partial lookup.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use chrono::{DateTime, Utc};
use tokio::{task::JoinHandle, time::Instant};

use super::{
    LookupError, RosterIgn, ScouterClient,
    wire::{Combined, Member},
};
use crate::{domain::history::Actor, infrastructure::character_data::Clock};

/// How long a result is kept in memory.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Retained jobs (one per target); past this the oldest finished one goes
/// before its 24 hours.
pub const MAX_JOBS: usize = 512;
/// Cached IGN sets; past this the oldest goes before its 24 hours.
pub const MAX_CACHED: usize = 1024;
/// Health probes `GET /healthz` at most this often.
const PROBE_EVERY: Duration = Duration::from_secs(60);

/// A roster character to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    pub character_id: String,
    pub member_id: String,
    pub ign: RosterIgn,
}

/// Why a party member is not looked up. They are listed as missing, never
/// counted as 0%.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingReason {
    /// No active character on the roster.
    NoCharacters,
    /// Several active characters and none picked for the run.
    NoCharacterPicked,
    /// The character is not on MapleSEA (MapleScouter covers only it).
    OtherRegion,
    /// The roster IGN is outside MapleScouter's rule.
    UnsupportedIgn,
}

impl MissingReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoCharacters => "no_characters",
            Self::NoCharacterPicked => "no_character_picked",
            Self::OtherRegion => "other_region",
            Self::UnsupportedIgn => "unsupported_ign",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub member_id: String,
    /// The character they bring, when there is one.
    pub character_id: Option<String>,
    pub reason: MissingReason,
}

/// What a job looks up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Roster characters (sorted ids); one is the roster strip's lookup.
    Characters(Vec<String>),
    /// A run's party, with the run's boss tokens.
    Run { run_id: String, bosses: Vec<String> },
}

impl Target {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Characters(a), Self::Characters(b)) => a == b,
            (Self::Run { run_id: a, .. }, Self::Run { run_id: b, .. }) => a == b,
            _ => false,
        }
    }

    /// A party lookup, with MapleScouter's total.
    fn party(&self) -> bool {
        !matches!(self, Self::Characters(ids) if ids.len() == 1)
    }
}

pub struct JobRequest {
    pub target: Target,
    /// 1 to `MAX_BATCH`, unique by IGN.
    pub subjects: Vec<Subject>,
    pub missing: Vec<Missing>,
    pub started_by: Actor,
    /// Skip the cache ("Look up again").
    pub refresh: bool,
}

/// A lookup's result: each member, and the party total for a party lookup.
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    pub members: Vec<Member>,
    /// Scouter boss key → total; `None` for one character.
    pub combined: Option<BTreeMap<String, Combined>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub estimate: Estimate,
    /// When kanade received it from the sidecar.
    pub fetched_at: DateTime<Utc>,
    /// Answered from kanade's cache without calling the sidecar.
    pub cache_hit: bool,
}

/// The drawn error states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// 429: busy with another lookup, or MapleScouter rate-limited it.
    Busy,
    /// 422: critical rate under 100% in every preset tried.
    CriticalRate,
    /// 503: MapleScouter asked for a verification.
    ManualCheck,
    /// 502, 504, the client timeout, no connection, an unreadable answer.
    Failed,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::CriticalRate => "critical_rate",
            Self::ManualCheck => "manual_check",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    /// The sidecar's HTTP status; `None` without an answer.
    pub status: Option<u16>,
    /// The sidecar's error code, or `timeout` / `unreachable` /
    /// `unreadable`.
    pub detail: String,
    pub retry_after: Option<u32>,
    /// The subject it stopped at, when known.
    pub subject: Option<usize>,
}

impl Failure {
    fn of(error: &LookupError, subjects: &[Subject], at: Option<usize>) -> Self {
        let (kind, status, retry_after, ign) = match error {
            LookupError::Refused {
                status,
                ign,
                retry_after,
                ..
            } => (
                match status {
                    429 => FailureKind::Busy,
                    422 => FailureKind::CriticalRate,
                    503 => FailureKind::ManualCheck,
                    _ => FailureKind::Failed,
                },
                Some(*status),
                *retry_after,
                ign.as_deref(),
            ),
            LookupError::Unreadable { status } => (FailureKind::Failed, *status, None, None),
            LookupError::Timeout | LookupError::Unreachable => {
                (FailureKind::Failed, None, None, None)
            }
        };
        let named = ign.and_then(|ign| {
            let key = ign.to_ascii_lowercase();
            subjects.iter().position(|subject| subject.ign.key() == key)
        });
        Self {
            kind,
            status,
            detail: error.label().to_owned(),
            retry_after,
            subject: at.or(named),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub target: Target,
    pub started_by: Actor,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub subjects: Vec<Subject>,
    pub missing: Vec<Missing>,
    /// Characters looked up so far.
    pub done: usize,
    /// The subject being looked up one by one; `None` for the party total
    /// or a single lookup.
    pub current: Option<usize>,
    /// `None` while running.
    pub outcome: Option<Result<Found, Failure>>,
}

impl Job {
    pub fn state(&self) -> JobState {
        match &self.outcome {
            None => JobState::Running,
            Some(Ok(_)) => JobState::Done,
            Some(Err(_)) => JobState::Failed,
        }
    }
}

pub enum Start {
    /// Started, or answered at once from the cache.
    Started(Job),
    /// Another job is running (this one).
    Running(Job),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScouterState {
    /// No `KANADE_SCOUTER_URL`: estimates are off.
    Off,
    Ok,
    /// The last call or probe got no connection (or no scouter answer).
    Unreachable,
    /// MapleScouter answered 503 `verification_required`; it stays until a
    /// lookup succeeds again.
    VerificationRequired,
}

impl ScouterState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ok => "ok",
            Self::Unreachable => "unreachable",
            Self::VerificationRequired => "verification_required",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScouterHealth {
    pub state: ScouterState,
    pub url: Option<String>,
    pub last_lookup_at: Option<DateTime<Utc>>,
    /// The last error's code (as [`Failure::detail`]) and when.
    pub last_error: Option<(String, DateTime<Utc>)>,
}

impl ScouterHealth {
    pub fn off() -> Self {
        Self {
            state: ScouterState::Off,
            url: None,
            last_lookup_at: None,
            last_error: None,
        }
    }
}

struct Cached {
    until: Instant,
    fetched_at: DateTime<Utc>,
    estimate: Estimate,
}

struct Kept {
    job: Job,
    /// `None` while running.
    until: Option<Instant>,
}

#[derive(Default)]
struct Seen {
    unreachable: bool,
    verification: bool,
    last_lookup_at: Option<DateTime<Utc>>,
    last_error: Option<(String, DateTime<Utc>)>,
    probed: Option<Instant>,
}

#[derive(Default)]
struct Inner {
    jobs: VecDeque<Kept>,
    running: Option<String>,
    cache: HashMap<String, Cached>,
    seen: Seen,
}

impl Inner {
    fn prune(&mut self, now: Instant) {
        self.cache.retain(|_, cached| cached.until > now);
        self.jobs
            .retain(|kept| kept.until.is_none_or(|until| until > now));
        while self.jobs.len() > MAX_JOBS {
            let Some(at) = self.jobs.iter().position(|kept| kept.until.is_some()) else {
                break;
            };
            self.jobs.remove(at);
        }
    }

    fn cached(&self, key: &str, party: bool, now: Instant) -> Option<&Cached> {
        self.cache
            .get(key)
            .filter(|cached| cached.until > now && (!party || cached.estimate.combined.is_some()))
    }

    fn keep(&mut self, key: String, estimate: Estimate, fetched_at: DateTime<Utc>, now: Instant) {
        if self.cache.len() >= MAX_CACHED
            && !self.cache.contains_key(&key)
            && let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, cached)| cached.until)
                .map(|(key, _)| key.clone())
        {
            self.cache.remove(&oldest);
        }
        self.cache.insert(
            key,
            Cached {
                until: now + CACHE_TTL,
                fetched_at,
                estimate,
            },
        );
    }

    fn job_mut(&mut self, id: &str) -> Option<&mut Job> {
        self.jobs
            .iter_mut()
            .find(|kept| kept.job.id == id)
            .map(|kept| &mut kept.job)
    }
}

/// The sorted, case-insensitive IGN set.
fn set_key<'a>(igns: impl Iterator<Item = &'a RosterIgn>) -> String {
    let mut keys: Vec<String> = igns.map(RosterIgn::key).collect();
    keys.sort_unstable();
    keys.join(",")
}

struct Shared {
    client: ScouterClient,
    url: String,
    clock: Clock,
    inner: Mutex<Inner>,
}

impl Shared {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// Notes what a sidecar call says about its health.
    fn saw<T>(&self, result: &Result<T, LookupError>) {
        let now = self.now();
        let mut inner = self.inner();
        let seen = &mut inner.seen;
        match result {
            Ok(_) => {
                seen.unreachable = false;
                seen.verification = false;
            }
            Err(error) => {
                seen.unreachable = matches!(error, LookupError::Unreachable);
                if matches!(error, LookupError::Refused { code, .. } if code == "verification_required")
                {
                    seen.verification = true;
                }
                seen.last_error = Some((error.label().to_owned(), now));
            }
        }
    }

    fn progress(&self, id: &str, done: usize, current: Option<usize>) {
        if let Some(job) = self.inner().job_mut(id) {
            job.done = done;
            job.current = current;
        }
    }

    fn cached_single(&self, ign: &RosterIgn) -> bool {
        self.inner()
            .cached(&ign.key(), false, Instant::now())
            .is_some()
    }

    fn keep_single(&self, member: Member) {
        let key = member.ign.to_ascii_lowercase();
        let now = self.now();
        self.inner().keep(
            key,
            Estimate {
                members: vec![member],
                combined: None,
            },
            now,
            Instant::now(),
        );
    }

    /// The lookup itself; the error carries the subject it stopped at.
    async fn lookup(
        &self,
        id: &str,
        igns: &[RosterIgn],
        party: bool,
        refresh: bool,
    ) -> Result<Estimate, (LookupError, Option<usize>)> {
        if !party {
            let result = self.client.single(&igns[0]).await;
            self.saw(&result);
            let member = result.map_err(|error| (error, Some(0)))?;
            return Ok(Estimate {
                members: vec![member],
                combined: None,
            });
        }
        if igns.len() > 1 {
            for (at, ign) in igns.iter().enumerate() {
                if refresh || !self.cached_single(ign) {
                    self.progress(id, at, Some(at));
                    let result = self.client.single(ign).await;
                    self.saw(&result);
                    self.keep_single(result.map_err(|error| (error, Some(at)))?);
                }
                self.progress(id, at + 1, None);
            }
        }
        let result = self.client.batch(igns).await;
        self.saw(&result);
        let batch = result.map_err(|error| (error, None))?;
        Ok(Estimate {
            members: batch.members,
            combined: Some(batch.combined),
        })
    }

    fn finish(&self, id: &str, result: Result<Estimate, (LookupError, Option<usize>)>) {
        let now = self.now();
        let instant = Instant::now();
        let mut inner = self.inner();
        inner.seen.last_lookup_at = Some(now);
        if inner.running.as_deref() == Some(id) {
            inner.running = None;
        }
        let Some(at) = inner.jobs.iter().position(|kept| kept.job.id == id) else {
            return;
        };
        let subjects = inner.jobs[at].job.subjects.clone();
        let outcome = match result {
            Ok(estimate) => {
                if estimate.members.len() > 1 {
                    for member in &estimate.members {
                        inner.keep(
                            member.ign.to_ascii_lowercase(),
                            Estimate {
                                members: vec![member.clone()],
                                combined: None,
                            },
                            now,
                            instant,
                        );
                    }
                }
                inner.keep(
                    set_key(subjects.iter().map(|subject| &subject.ign)),
                    estimate.clone(),
                    now,
                    instant,
                );
                Ok(Found {
                    estimate,
                    fetched_at: now,
                    cache_hit: false,
                })
            }
            Err((error, stopped)) => Err(Failure::of(&error, &subjects, stopped)),
        };
        log(&outcome, subjects.len());
        let kept = &mut inner.jobs[at];
        kept.until = Some(instant + CACHE_TTL);
        let job = &mut kept.job;
        if outcome.is_ok() {
            job.done = subjects.len();
        }
        job.current = None;
        job.finished_at = Some(now);
        job.outcome = Some(outcome);
    }
}

/// Codes and counts only: never an IGN or a result.
fn log(outcome: &Result<Found, Failure>, characters: usize) {
    let (level, fields) = match outcome {
        Ok(found) => (
            "INFO",
            serde_json::json!({
                "state": "done",
                "characters": characters,
                "cache_hit": found.cache_hit,
            }),
        ),
        Err(failure) => (
            "WARN",
            serde_json::json!({
                "state": "failed",
                "characters": characters,
                "code": failure.kind.as_str(),
                "detail": failure.detail,
                "status": failure.status,
            }),
        ),
    };
    crate::runtime::logging::event(level, "estimate_finished", fields);
}

/// The scouter sidecar's lookups, shared by the admin API.
pub struct Estimates {
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for Estimates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Estimates").finish_non_exhaustive()
    }
}

impl Drop for Estimates {
    fn drop(&mut self) {
        if let Some(task) = self
            .task
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

impl Estimates {
    /// `url` is shown in health (it holds no secret).
    pub fn new(client: ScouterClient, url: String, clock: Clock) -> Self {
        Self {
            shared: Arc::new(Shared {
                client,
                url,
                clock,
                inner: Mutex::default(),
            }),
            task: Mutex::default(),
        }
    }

    /// Start a job (or answer it from the cache at once), unless one is
    /// running. Spawns on the current tokio runtime.
    pub fn start(&self, request: JobRequest) -> Start {
        let now = self.shared.now();
        let instant = Instant::now();
        let party = request.target.party();
        let key = set_key(request.subjects.iter().map(|subject| &subject.ign));
        let mut inner = self.shared.inner();
        inner.prune(instant);
        if let Some(running) = inner
            .running
            .clone()
            .and_then(|id| inner.jobs.iter().find(|kept| kept.job.id == id))
        {
            return Start::Running(running.job.clone());
        }
        let mut job = Job {
            id: uuid::Uuid::new_v4().simple().to_string(),
            target: request.target,
            started_by: request.started_by,
            started_at: now,
            finished_at: None,
            done: 0,
            current: None,
            subjects: request.subjects,
            missing: request.missing,
            outcome: None,
        };
        let hit = (!request.refresh)
            .then(|| inner.cached(&key, party, instant))
            .flatten()
            .map(|cached| Found {
                estimate: cached.estimate.clone(),
                fetched_at: cached.fetched_at,
                cache_hit: true,
            });
        // One retained job per target: the new one replaces the last.
        let target = job.target.clone();
        inner.jobs.retain(|kept| !kept.job.target.same(&target));
        if let Some(found) = hit {
            job.done = job.subjects.len();
            job.finished_at = Some(now);
            let outcome = Ok(found);
            log(&outcome, job.subjects.len());
            job.outcome = Some(outcome);
            inner.jobs.push_back(Kept {
                job: job.clone(),
                until: Some(instant + CACHE_TTL),
            });
            inner.prune(instant);
            return Start::Started(job);
        }
        inner.running = Some(job.id.clone());
        inner.jobs.push_back(Kept {
            job: job.clone(),
            until: None,
        });
        inner.prune(instant);
        drop(inner);
        let shared = Arc::clone(&self.shared);
        let id = job.id.clone();
        let igns: Vec<RosterIgn> = job
            .subjects
            .iter()
            .map(|subject| subject.ign.clone())
            .collect();
        let refresh = request.refresh;
        let handle = tokio::spawn(async move {
            let result = shared.lookup(&id, &igns, party, refresh).await;
            shared.finish(&id, result);
        });
        *self.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        Start::Started(job)
    }

    /// Cancel the running lookup, if any, and wait until its task has ended
    /// (serve calls it before the stores close).
    pub async fn stop(&self) {
        let task = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        let mut inner = self.shared.inner();
        inner.prune(Instant::now());
        inner
            .jobs
            .iter()
            .find(|kept| kept.job.id == id)
            .map(|kept| kept.job.clone())
    }

    /// The newest job (running or kept) for `target`.
    pub fn latest(&self, target: &Target) -> Option<Job> {
        let mut inner = self.shared.inner();
        inner.prune(Instant::now());
        inner
            .jobs
            .iter()
            .rev()
            .find(|kept| kept.job.target.same(target))
            .map(|kept| kept.job.clone())
    }

    /// `ign`'s newest successful result still in memory, from a lookup of
    /// that character alone or of a party that included them (a party
    /// lookup keeps each member under their own IGN too, so this entry is
    /// always the newest), and when it was fetched. Never starts a lookup.
    pub fn cached(&self, ign: &RosterIgn) -> Option<(Member, DateTime<Utc>)> {
        let inner = self.shared.inner();
        let cached = inner.cached(&ign.key(), false, Instant::now())?;
        cached
            .estimate
            .members
            .iter()
            .find(|member| member.ign.eq_ignore_ascii_case(ign.as_str()))
            .map(|member| (member.clone(), cached.fetched_at))
    }

    /// The running job, whatever it looks up.
    pub fn running(&self) -> Option<Job> {
        let inner = self.shared.inner();
        let id = inner.running.as_deref()?;
        inner
            .jobs
            .iter()
            .find(|kept| kept.job.id == id)
            .map(|kept| kept.job.clone())
    }

    /// The health block; probes `GET /healthz` at most once a minute.
    pub async fn health(&self) -> ScouterHealth {
        let probe = self
            .shared
            .inner()
            .seen
            .probed
            .is_none_or(|at| at.elapsed() >= PROBE_EVERY);
        if probe {
            let result = self.shared.client.healthy().await;
            let now = self.shared.now();
            let mut inner = self.shared.inner();
            let seen = &mut inner.seen;
            seen.probed = Some(Instant::now());
            match result {
                Ok(()) => seen.unreachable = false,
                Err(error) => {
                    seen.unreachable = true;
                    seen.last_error = Some((error.label().to_owned(), now));
                }
            }
        }
        let inner = self.shared.inner();
        let seen = &inner.seen;
        ScouterHealth {
            state: if seen.verification {
                ScouterState::VerificationRequired
            } else if seen.unreachable {
                ScouterState::Unreachable
            } else {
                ScouterState::Ok
            },
            url: Some(self.shared.url.clone()),
            last_lookup_at: seen.last_lookup_at,
            last_error: seen.last_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::{
        domain::characters::{Character, CharacterRole},
        infrastructure::scouter::Card,
    };

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 29, hour, 0, 0).unwrap()
    }

    /// Invented roster IGN.
    fn ign(name: &str) -> RosterIgn {
        RosterIgn::of(&Character {
            id: "ch-1".into(),
            member_id: "1001".into(),
            ign: name.into(),
            world: "msea".into(),
            class: None,
            level: None,
            role: CharacterRole::Main,
            retired_at: None,
            created_at: at(0),
            updated_at: at(0),
            updated_by: Actor::system("test"),
            version: 1,
        })
        .unwrap()
    }

    /// Invented answer: one Limbo card worth `percentage`.
    fn member(ign: &str, percentage: f64) -> Member {
        Member {
            ign: ign.into(),
            preset: "00010".into(),
            bosses: BTreeMap::from([(
                "hard_limbo".to_owned(),
                Card {
                    percentage,
                    rating: "Party-able".into(),
                    party: false,
                    effective_percentage: Some(percentage),
                    estimated_minutes: Some(3_000.0 / percentage),
                },
            )]),
        }
    }

    /// The sidecar is never called: results are put in memory directly.
    fn estimates(now: Arc<Mutex<DateTime<Utc>>>) -> Estimates {
        let url = "http://127.0.0.1:9".to_owned();
        Estimates::new(
            ScouterClient::new(&url).unwrap(),
            url,
            Arc::new(move || *now.lock().unwrap_or_else(PoisonError::into_inner)),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn a_cached_reading_is_the_newest_and_gone_after_24_hours() {
        let now = Arc::new(Mutex::new(at(4)));
        let estimates = estimates(Arc::clone(&now));
        let ren = ign("RenShade");
        assert_eq!(estimates.cached(&ren), None, "nothing looked up yet");

        estimates.shared.keep_single(member("renshade", 40.0));
        *now.lock().unwrap_or_else(PoisonError::into_inner) = at(6);
        tokio::time::advance(Duration::from_secs(2 * 3_600)).await;
        estimates.shared.keep_single(member("RENSHADE", 72.4));
        let (newest, fetched_at) = estimates.cached(&ren).unwrap();
        assert_eq!(
            (newest.bosses["hard_limbo"].percentage, fetched_at),
            (72.4, at(6))
        );
        assert_eq!(estimates.cached(&ign("MikaBish")), None);

        tokio::time::advance(CACHE_TTL - Duration::from_secs(1)).await;
        assert!(estimates.cached(&ren).is_some());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(estimates.cached(&ren), None, "expired");
        assert!(estimates.running().is_none(), "reading starts nothing");
    }
}
