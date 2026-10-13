//! Lookup jobs and their in-memory cache. Jobs wait in one FIFO queue (at
//! most [`MAX_QUEUED`]) drained by one worker, because the sidecar is one
//! browser; a request for a target already queued or running gets that job.
//! The cache keeps results 24 hours, keyed by the sorted set of IGNs, lost
//! on restart and never written anywhere. Finished jobs are kept 24 hours
//! too, one per target (a new lookup of the same characters or run replaces
//! the old job), so an admin who closed the page finds the result (or the
//! error) when they come back. Memory is bounded by [`MAX_JOBS`] targets
//! and [`MAX_CACHED`] IGN sets; only past those is the oldest dropped
//! early. The worker is cancelled and joined on shutdown
//! ([`Estimates::stop`]).
//!
//! A job looks up each character on its own (`ign=`) and records each
//! outcome as it arrives: one character's failure never stops the others,
//! and every success is cached under its IGN. A party then asks once as
//! `igns=` with the characters that succeeded, for MapleScouter's party
//! total, which its own short cache answers without rendering again; the
//! total is complete only when every character succeeded. A `429 busy` is
//! asked again after its `Retry-After` while the job's deadline allows.

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
/// Jobs waiting behind the running one; past this a new target is refused.
pub const MAX_QUEUED: usize = 16;
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

/// A cached result: each member, and the party total for a party lookup.
#[derive(Clone, Debug, PartialEq)]
struct Estimate {
    members: Vec<Member>,
    /// Scouter boss key → total; `None` for one character.
    combined: Option<BTreeMap<String, Combined>>,
}

/// A finished job with at least one character found (their results are
/// the job's `results`).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// MapleScouter's party total over the characters that succeeded;
    /// `None` for one character, or when `total_failure` says why.
    pub combined: Option<BTreeMap<String, Combined>>,
    /// The party total's own failure, after characters succeeded.
    pub total_failure: Option<Failure>,
    /// When kanade received it from the sidecar.
    pub fetched_at: DateTime<Utc>,
    /// Answered from kanade's cache without calling the sidecar.
    pub cache_hit: bool,
}

/// The drawn error states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// 429: MapleScouter stayed busy past the job's deadline, or was
    /// rate-limited.
    Busy,
    /// 422: critical rate under 100% and no assumed result.
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
    /// The subject it is about, when known.
    pub subject: Option<usize>,
}

impl Failure {
    fn of(error: &LookupError, igns: &[RosterIgn], at: Option<usize>) -> Self {
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
            igns.iter().position(|subject| subject.key() == key)
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
    Queued,
    Running,
    Done,
    Failed,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// A character's outcome: their result, or why there is none.
pub type MemberOutcome = Result<Member, Failure>;

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub target: Target,
    pub started_by: Actor,
    /// When it was asked for.
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub subjects: Vec<Subject>,
    pub missing: Vec<Missing>,
    /// Jobs ahead of it, the running one included, while it waits.
    pub position: Option<usize>,
    /// Each subject's outcome, in subject order, as it arrives.
    pub results: Vec<Option<MemberOutcome>>,
    /// The subject being looked up now; `None` while queued, between
    /// characters, for the party total and once finished.
    pub current: Option<usize>,
    /// `Ok` once at least one character was found, else the first
    /// character's failure; `None` until finished.
    pub outcome: Option<Result<Found, Failure>>,
}

impl Job {
    pub fn state(&self) -> JobState {
        match (&self.outcome, self.position) {
            (Some(Ok(_)), _) => JobState::Done,
            (Some(Err(_)), _) => JobState::Failed,
            (None, Some(_)) => JobState::Queued,
            (None, None) => JobState::Running,
        }
    }

    /// Characters finished so far, found or failed.
    pub fn done(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.is_some())
            .count()
    }
}

pub enum Start {
    /// Queued (or running at once), or answered at once from the cache.
    Started(Job),
    /// The target's job, already queued or running.
    Joined(Job),
    /// [`MAX_QUEUED`] jobs are waiting already.
    Full,
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
    /// `None` while queued or running.
    until: Option<Instant>,
    /// Skip the cache ("Look up again").
    refresh: bool,
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
    /// Waiting job ids, oldest first. Empty while `running` is `None`: the
    /// worker takes the next one as it finishes a job.
    queue: VecDeque<String>,
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

    /// `ign`'s newest result still in memory, and when it was fetched.
    fn member(&self, ign: &RosterIgn, now: Instant) -> Option<(&Member, DateTime<Utc>)> {
        let cached = self.cached(&ign.key(), false, now)?;
        cached
            .estimate
            .members
            .iter()
            .find(|member| member.ign.eq_ignore_ascii_case(ign.as_str()))
            .map(|member| (member, cached.fetched_at))
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

    /// A copy of `kept`'s job with its place in the queue.
    fn snapshot(&self, kept: &Kept) -> Job {
        let mut job = kept.job.clone();
        job.position = self
            .queue
            .iter()
            .position(|id| *id == job.id)
            .map(|at| at + usize::from(self.running.is_some()));
        job
    }

    fn find(&self, id: &str) -> Option<Job> {
        self.jobs
            .iter()
            .find(|kept| kept.job.id == id)
            .map(|kept| self.snapshot(kept))
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

    /// One sidecar call within `deadline`: `attempt` gets the time left, and
    /// a `429 busy` is asked again after its `Retry-After` while that wait
    /// still ends before the deadline.
    async fn call<T, F, Fut>(&self, deadline: Instant, mut attempt: F) -> Result<T, LookupError>
    where
        F: FnMut(Duration) -> Fut,
        Fut: Future<Output = Result<T, LookupError>>,
    {
        loop {
            let result = attempt(deadline.saturating_duration_since(Instant::now())).await;
            self.saw(&result);
            if let Err(LookupError::Refused {
                status: 429,
                code,
                retry_after: Some(seconds),
                ..
            }) = &result
                && code == "busy"
            {
                let wait = Duration::from_secs(u64::from(*seconds));
                // Never `now + wait`: an absurd Retry-After would overflow.
                if wait < deadline.saturating_duration_since(Instant::now()) {
                    tokio::time::sleep(wait).await;
                    continue;
                }
            }
            return result;
        }
    }

    fn edit(&self, id: &str, edit: impl FnOnce(&mut Job)) {
        if let Some(job) = self.inner().job_mut(id) {
            edit(job);
        }
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

    /// Job `id`'s lookups; `None` when the job is gone.
    async fn run(&self, id: &str) -> Option<Result<Found, Failure>> {
        let (igns, party, refresh) = {
            let inner = self.inner();
            let kept = inner.jobs.iter().find(|kept| kept.job.id == id)?;
            let igns: Vec<RosterIgn> = kept
                .job
                .subjects
                .iter()
                .map(|subject| subject.ign.clone())
                .collect();
            (igns, kept.job.target.party(), kept.refresh)
        };
        // Each call the job makes adds the client's timeout to its budget;
        // a busy wait spends from the same budget.
        let calls = u32::try_from(igns.len() + usize::from(party)).unwrap_or(u32::MAX);
        let deadline = Instant::now() + self.client.timeout().saturating_mul(calls);
        let mut found = Vec::with_capacity(igns.len());
        let mut first_failure = None;
        for (at, ign) in igns.iter().enumerate() {
            // Found meanwhile (another job, or before this one was queued).
            let cached = (!refresh)
                .then(|| {
                    self.inner()
                        .member(ign, Instant::now())
                        .map(|(member, _)| member.clone())
                })
                .flatten();
            let outcome = match cached {
                Some(member) => Ok(member),
                None => {
                    self.edit(id, |job| job.current = Some(at));
                    let result = self
                        .call(deadline, |within| self.client.single(ign, within))
                        .await;
                    if let Ok(member) = &result {
                        self.keep_single(member.clone());
                    }
                    result.map_err(|error| Failure::of(&error, &igns, Some(at)))
                }
            };
            match &outcome {
                Ok(_) => found.push(ign.clone()),
                Err(failure) => {
                    first_failure.get_or_insert_with(|| failure.clone());
                }
            }
            self.edit(id, |job| {
                if let Some(slot) = job.results.get_mut(at) {
                    *slot = Some(outcome);
                }
                job.current = None;
            });
        }
        if found.is_empty() {
            return first_failure.map(Err);
        }
        let (combined, total_failure) = if party {
            let result = self
                .call(deadline, |within| self.client.batch(&found, within))
                .await;
            match result {
                Ok(batch) => (Some(batch.combined), None),
                Err(error) => (None, Some(Failure::of(&error, &igns, None))),
            }
        } else {
            (None, None)
        };
        Some(Ok(Found {
            combined,
            total_failure,
            fetched_at: self.now(),
            cache_hit: false,
        }))
    }

    /// Records job `id`'s outcome and hands the worker the next job.
    fn finish(&self, id: &str, outcome: Option<Result<Found, Failure>>) -> Option<String> {
        let now = self.now();
        let instant = Instant::now();
        let mut inner = self.inner();
        inner.seen.last_lookup_at = Some(now);
        if let Some(outcome) = outcome
            && let Some(at) = inner.jobs.iter().position(|kept| kept.job.id == id)
        {
            let job = &inner.jobs[at].job;
            let failed = job
                .results
                .iter()
                .filter(|result| matches!(result, Some(Err(_))))
                .count();
            log(&outcome, job.subjects.len(), failed);
            // A whole party, total included, is kept under its IGN set.
            let whole = match &outcome {
                Ok(Found {
                    combined: Some(combined),
                    ..
                }) if failed == 0 => job
                    .results
                    .iter()
                    .map(|result| match result {
                        Some(Ok(member)) => Some(member.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<Member>>>()
                    .map(|members| {
                        (
                            set_key(job.subjects.iter().map(|subject| &subject.ign)),
                            Estimate {
                                members,
                                combined: Some(combined.clone()),
                            },
                        )
                    }),
                _ => None,
            };
            if let Some((key, estimate)) = whole {
                inner.keep(key, estimate, now, instant);
            }
            let kept = &mut inner.jobs[at];
            kept.until = Some(instant + CACHE_TTL);
            kept.job.current = None;
            kept.job.finished_at = Some(now);
            kept.job.outcome = Some(outcome);
        }
        inner.running = inner.queue.pop_front();
        inner.running.clone()
    }

    /// The one worker: job `id`, then each next one until the queue is
    /// empty.
    async fn work(self: Arc<Self>, mut id: String) {
        loop {
            let outcome = self.run(&id).await;
            match self.finish(&id, outcome) {
                Some(next) => id = next,
                None => return,
            }
        }
    }
}

/// Codes and counts only: never an IGN or a result.
fn log(outcome: &Result<Found, Failure>, characters: usize, failed: usize) {
    let (level, fields) = match outcome {
        Ok(found) => (
            if failed > 0 || found.total_failure.is_some() {
                "WARN"
            } else {
                "INFO"
            },
            serde_json::json!({
                "state": "done",
                "characters": characters,
                "failed": failed,
                "total_failed": found.total_failure.as_ref().map(|failure| failure.kind.as_str()),
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

    /// Queue a job (or answer it from the cache at once); the target's own
    /// queued or running job is answered instead of a second one. Spawns
    /// the worker on the current tokio runtime when none runs.
    pub fn start(&self, request: JobRequest) -> Start {
        let now = self.shared.now();
        let instant = Instant::now();
        let party = request.target.party();
        let mut inner = self.shared.inner();
        inner.prune(instant);
        if let Some(active) = inner
            .jobs
            .iter()
            .find(|kept| kept.until.is_none() && kept.job.target.same(&request.target))
        {
            return Start::Joined(inner.snapshot(active));
        }
        let key = set_key(request.subjects.iter().map(|subject| &subject.ign));
        let hit = (!request.refresh)
            .then(|| inner.cached(&key, party, instant))
            .flatten()
            .and_then(|cached| {
                let results = request
                    .subjects
                    .iter()
                    .map(|subject| {
                        cached
                            .estimate
                            .members
                            .iter()
                            .find(|member| member.ign.eq_ignore_ascii_case(subject.ign.as_str()))
                            .map(|member| Some(Ok(member.clone())))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some((
                    results,
                    Found {
                        combined: cached.estimate.combined.clone(),
                        total_failure: None,
                        fetched_at: cached.fetched_at,
                        cache_hit: true,
                    },
                ))
            });
        if hit.is_none() && inner.queue.len() >= MAX_QUEUED {
            return Start::Full;
        }
        let mut job = Job {
            id: uuid::Uuid::new_v4().simple().to_string(),
            target: request.target,
            started_by: request.started_by,
            started_at: now,
            finished_at: None,
            position: None,
            results: vec![None; request.subjects.len()],
            current: None,
            subjects: request.subjects,
            missing: request.missing,
            outcome: None,
        };
        // One retained job per target: the new one replaces the last.
        let target = job.target.clone();
        inner.jobs.retain(|kept| !kept.job.target.same(&target));
        if let Some((results, found)) = hit {
            job.results = results;
            job.finished_at = Some(now);
            let outcome = Ok(found);
            log(&outcome, job.subjects.len(), 0);
            job.outcome = Some(outcome);
            inner.jobs.push_back(Kept {
                job: job.clone(),
                until: Some(instant + CACHE_TTL),
                refresh: false,
            });
            inner.prune(instant);
            return Start::Started(job);
        }
        let id = job.id.clone();
        inner.jobs.push_back(Kept {
            job,
            until: None,
            refresh: request.refresh,
        });
        inner.queue.push_back(id.clone());
        // No worker while nothing runs: this job starts one.
        let first = if inner.running.is_none() {
            inner.running = inner.queue.pop_front();
            inner.running.clone()
        } else {
            None
        };
        inner.prune(instant);
        let job = inner.find(&id).expect("a queued job is never pruned");
        drop(inner);
        if let Some(first) = first {
            let handle = tokio::spawn(Arc::clone(&self.shared).work(first));
            *self.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        }
        Start::Started(job)
    }

    /// Cancel the worker, if any, and wait until its task has ended (serve
    /// calls it before the stores close). Queued jobs stay queued.
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
        inner.find(id)
    }

    /// The newest job (queued, running or kept) for `target`.
    pub fn latest(&self, target: &Target) -> Option<Job> {
        let mut inner = self.shared.inner();
        inner.prune(Instant::now());
        inner
            .jobs
            .iter()
            .rev()
            .find(|kept| kept.job.target.same(target))
            .map(|kept| inner.snapshot(kept))
    }

    /// `ign`'s newest successful result still in memory, from a lookup of
    /// that character alone or of a party that included them (each
    /// character is kept under their own IGN as they are found, so this
    /// entry is always the newest), and when it was fetched. Never starts
    /// a lookup.
    pub fn cached(&self, ign: &RosterIgn) -> Option<(Member, DateTime<Utc>)> {
        self.shared
            .inner()
            .member(ign, Instant::now())
            .map(|(member, fetched_at)| (member.clone(), fetched_at))
    }

    /// The running job, whatever it looks up.
    pub fn running(&self) -> Option<Job> {
        let inner = self.shared.inner();
        inner.find(inner.running.as_deref()?)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            critical_rate_assumed: false,
            critical_rate_actual: None,
            source: None,
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

    fn busy(seconds: u32) -> LookupError {
        LookupError::Refused {
            status: 429,
            code: "busy".into(),
            ign: None,
            retry_after: Some(seconds),
        }
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

    /// `429 busy` is asked again after its Retry-After while the wait ends
    /// before the deadline; past it, the busy answer stands.
    #[tokio::test(start_paused = true)]
    async fn busy_is_retried_after_retry_after_within_the_deadline() {
        let estimates = estimates(Arc::new(Mutex::new(at(4))));
        let shared = &estimates.shared;
        let calls = AtomicUsize::new(0);
        let begun = Instant::now();
        let deadline = begun + Duration::from_secs(30);
        let answer = shared
            .call(deadline, |within| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                let left = deadline.saturating_duration_since(Instant::now());
                async move {
                    assert_eq!(within, left, "each try gets the time left");
                    if call < 2 { Err(busy(8)) } else { Ok(call) }
                }
            })
            .await;
        assert_eq!(answer, Ok(2), "the third try answers");
        assert_eq!(
            begun.elapsed(),
            Duration::from_secs(16),
            "8 s after each busy"
        );

        // A wait that would end past the deadline is not taken.
        calls.store(0, Ordering::SeqCst);
        let begun = Instant::now();
        let answer = shared
            .call(begun + Duration::from_secs(10), |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err::<usize, _>(busy(8)) }
            })
            .await;
        assert_eq!(answer, Err(busy(8)));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "one retry fits, the next doesn't"
        );
        assert_eq!(begun.elapsed(), Duration::from_secs(8));

        // Other refusals, a rate limit included, are never retried.
        calls.store(0, Ordering::SeqCst);
        let limited = LookupError::Refused {
            status: 429,
            code: "upstream_rate_limited".into(),
            ign: None,
            retry_after: Some(1),
        };
        let answer = shared
            .call(Instant::now() + Duration::from_secs(60), |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                let limited = limited.clone();
                async move { Err::<usize, _>(limited) }
            })
            .await;
        assert_eq!(answer, Err(limited));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            shared
                .inner()
                .seen
                .last_error
                .as_ref()
                .map(|(code, _)| code.as_str()),
            Some("upstream_rate_limited")
        );
    }
}
