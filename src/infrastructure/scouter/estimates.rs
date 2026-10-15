//! Lookup jobs and their results. Jobs wait in one FIFO queue (at most
//! [`MAX_QUEUED`]) drained by one worker, because the sidecar is one
//! browser; a request for a target already queued or running gets that job.
//!
//! Results are kept in memory up to [`CACHE_TTL`] (the character-data
//! file's 30 days): each character's under their roster IGN, each whole
//! party's total under its sorted IGN set. With a file
//! ([`Estimates::with_store`]) every result is written through as it
//! arrives (keyed by the character, with the IGN key it was looked up
//! under) and the newest [`MAX_CACHED`] of each kind are loaded at startup
//! ([`Estimates::load`]); a failed write only logs, memory keeps the
//! result. Only a result younger than the saved `estimate_days`
//! ([`Estimates::with_fresh_days`]) answers a lookup at once; an older one
//! is looked up again. The synchronous readers ([`Estimates::cached`],
//! [`Estimates::cached_party`]) get a result of any age with its fetch
//! time. The sweeper drops what it deletes from the file
//! ([`Estimates::forget`], [`Estimates::sweep`]).
//!
//! Finished jobs are kept [`JOB_TTL`], one per target (a new lookup of the
//! same characters or run replaces the old job), so an admin who closed the
//! page finds the result (or the error) when they come back. Memory is
//! bounded by [`MAX_JOBS`] targets and [`MAX_CACHED`] results of each kind;
//! only past those is the oldest dropped early. The worker is cancelled and
//! joined on shutdown ([`Estimates::stop`]).
//!
//! A job looks up each character on its own (`ign=`) and records each
//! outcome as it arrives: one character's failure never stops the others,
//! and every success is kept under its IGN. A party then asks once as
//! `igns=` with the characters that succeeded, for the scouter's party
//! total, which its own short cache answers without rendering again; the
//! total is complete only when every character succeeded. A `429 busy` is
//! asked again after its `Retry-After` while the job's deadline allows.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use chrono::{DateTime, TimeDelta, Utc};
use tokio::{task::JoinHandle, time::Instant};

use super::{
    LookupError, RosterIgn, ScouterClient,
    wire::{Combined, Member},
};
use crate::{
    domain::{
        chardata::{CharDataStore, ESTIMATE_TTL, MemberEstimate, PartyEstimate, estimate_kept},
        history::Actor,
        scheduler::StoreError,
        settings::DEFAULT_ESTIMATE_DAYS,
    },
    infrastructure::character_data::{Clock, error_kind},
};

/// How long a result is kept in memory: the character-data file's
/// [`ESTIMATE_TTL`].
pub const CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long a finished job is kept.
pub const JOB_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Retained jobs (one per target); past this the oldest finished one goes
/// before its 24 hours.
pub const MAX_JOBS: usize = 512;
/// Results held per kind (characters, party totals), and how many of each
/// a restart loads; past this the oldest goes before its 30 days.
pub const MAX_CACHED: usize = 1024;
/// Jobs waiting behind the running one; past this a new target is refused.
pub const MAX_QUEUED: usize = 16;
/// Health probes `GET /healthz` at most this often.
const PROBE_EVERY: Duration = Duration::from_secs(60);

/// Days a result answers a lookup at once (the saved `estimate_days`),
/// read at every lookup so a Config save applies at once.
pub type FreshDays = Arc<dyn Fn() -> u32 + Send + Sync>;

/// A roster character to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    pub character_id: String,
    pub member_id: String,
    pub ign: RosterIgn,
}

crate::domain::spelled! {
    /// Why a party member is not looked up. They are listed as missing, never
    /// counted as 0%.
    MissingReason {
        /// No active character on the roster.
        NoCharacters => "no_characters",
        /// Several active characters and none picked for the run.
        NoCharacterPicked => "no_character_picked",
        /// The character is not on MapleSEA (the scouter covers only it).
        OtherRegion => "other_region",
        /// The roster IGN is outside the scouter's rule.
        UnsupportedIgn => "unsupported_ign",
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

    /// A party lookup, with the scouter's total.
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

/// A finished job with at least one character found (their results are
/// the job's `results`).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The scouter's party total over the characters that succeeded;
    /// `None` for one character, or when `total_failure` says why.
    pub combined: Option<BTreeMap<String, Combined>>,
    /// The party total's own failure, after characters succeeded.
    pub total_failure: Option<Failure>,
    /// When kanade received it from the sidecar (answered from memory: the
    /// oldest result it used).
    pub fetched_at: DateTime<Utc>,
    /// Answered from kanade's cache without calling the sidecar.
    pub cache_hit: bool,
}

crate::domain::spelled! {
    /// The drawn error states.
    FailureKind {
        /// 429: the scouter stayed busy past the job's deadline, or was
        /// rate-limited.
        Busy => "busy",
        /// 422: critical rate under 100% and no assumed result.
        CriticalRate => "critical_rate",
        /// 503: the scouter asked for a verification.
        ManualCheck => "manual_check",
        /// 502, 504, the client timeout, no connection, an unreadable answer.
        Failed => "failed",
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

crate::domain::spelled! {
    JobState {
        Queued => "queued",
        Running => "running",
        Done => "done",
        Failed => "failed",
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

crate::domain::spelled! {
    ScouterState {
        /// No `KANADE_SCOUTER_URL`: estimates are off.
        Off => "off",
        Ok => "ok",
        /// The last call or probe got no connection (or no scouter answer).
        Unreachable => "unreachable",
        /// The scouter answered 503 `verification_required`; it stays until a
        /// lookup succeeds again.
        VerificationRequired => "verification_required",
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

/// A result in memory.
struct Held<T> {
    /// Dropped then (monotonic, so a wall-clock step never keeps it).
    until: Instant,
    fetched_at: DateTime<Utc>,
    /// The roster characters it is about: one for a character's result,
    /// ascending and distinct for a party total.
    characters: Vec<String>,
    value: T,
}

/// The monotonic end of a result fetched at `fetched_at`, seen at wall
/// clock `now` and monotonic `instant`: what is left of its 30 days.
fn held_until(fetched_at: DateTime<Utc>, now: DateTime<Utc>, instant: Instant) -> Instant {
    let left = (fetched_at + ESTIMATE_TTL - now)
        .to_std()
        .unwrap_or(Duration::ZERO);
    instant + left.min(CACHE_TTL)
}

/// Insert `held` under `key`, dropping the oldest first when full.
fn hold<T>(map: &mut HashMap<String, Held<T>>, key: String, held: Held<T>) {
    if map.len() >= MAX_CACHED
        && !map.contains_key(&key)
        && let Some(oldest) = map
            .iter()
            .min_by_key(|(_, held)| held.until)
            .map(|(key, _)| key.clone())
    {
        map.remove(&oldest);
    }
    map.insert(key, held);
}

/// The ascending, distinct characters of `subjects`.
fn characters(subjects: &[Subject]) -> Vec<String> {
    let mut ids: Vec<String> = subjects
        .iter()
        .map(|subject| subject.character_id.clone())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
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
    /// IGN key → the character's newest result.
    members: HashMap<String, Held<Member>>,
    /// Sorted IGN set → the party's newest total.
    parties: HashMap<String, Held<BTreeMap<String, Combined>>>,
    seen: Seen,
}

impl Inner {
    fn prune(&mut self, now: Instant) {
        self.members.retain(|_, held| held.until > now);
        self.parties.retain(|_, held| held.until > now);
        self.jobs
            .retain(|kept| kept.until.is_none_or(|until| until > now));
        while self.jobs.len() > MAX_JOBS {
            let Some(at) = self.jobs.iter().position(|kept| kept.until.is_some()) else {
                break;
            };
            self.jobs.remove(at);
        }
    }

    /// `ign`'s newest result still in memory, of any age.
    fn member(&self, ign: &RosterIgn, now: Instant) -> Option<&Held<Member>> {
        self.members.get(&ign.key()).filter(|held| held.until > now)
    }

    /// The newest total of exactly the IGN set `key`, of any age.
    fn party(&self, key: &str, now: Instant) -> Option<&Held<BTreeMap<String, Combined>>> {
        self.parties.get(key).filter(|held| held.until > now)
    }

    fn keep_member(
        &mut self,
        ign_key: String,
        character_id: String,
        member: Member,
        fetched_at: DateTime<Utc>,
        until: Instant,
    ) {
        let held = Held {
            until,
            fetched_at,
            characters: vec![character_id],
            value: member,
        };
        hold(&mut self.members, ign_key, held);
    }

    fn keep_party(
        &mut self,
        set_key: String,
        characters: Vec<String>,
        combined: BTreeMap<String, Combined>,
        fetched_at: DateTime<Utc>,
        until: Instant,
    ) {
        let held = Held {
            until,
            fetched_at,
            characters,
            value: combined,
        };
        hold(&mut self.parties, set_key, held);
    }

    /// What memory answers for a lookup of `subjects` at once: each one's
    /// result (and the total, for a party), every one about these very
    /// characters and `fresh`; `None` unless all are.
    fn answer(
        &self,
        subjects: &[Subject],
        party: bool,
        now: Instant,
        fresh: impl Fn(DateTime<Utc>) -> bool,
    ) -> Option<(Vec<Option<MemberOutcome>>, Found)> {
        let mut oldest: Option<DateTime<Utc>> = None;
        let mut seen = |at: DateTime<Utc>| oldest = Some(oldest.map_or(at, |was| was.min(at)));
        let mut results = Vec::with_capacity(subjects.len());
        for subject in subjects {
            let held = self.member(&subject.ign, now).filter(|held| {
                held.characters.as_slice() == std::slice::from_ref(&subject.character_id)
                    && fresh(held.fetched_at)
            })?;
            seen(held.fetched_at);
            results.push(Some(Ok(held.value.clone())));
        }
        let combined = if party {
            let ids = characters(subjects);
            let held = self
                .party(&set_key(subjects.iter().map(|subject| &subject.ign)), now)
                .filter(|held| held.characters == ids && fresh(held.fetched_at))?;
            seen(held.fetched_at);
            Some(held.value.clone())
        } else {
            None
        };
        Some((
            results,
            Found {
                combined,
                total_failure: None,
                fetched_at: oldest?,
                cache_hit: true,
            },
        ))
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
    /// The character-data file results are written through to.
    store: Option<Arc<dyn CharDataStore>>,
    fresh_days: FreshDays,
    inner: Mutex<Inner>,
}

impl Shared {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// A result fetched at `fetched_at` may answer a lookup at `now`: it is
    /// younger than the saved `estimate_days`.
    fn fresh(&self, fetched_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        now - fetched_at < TimeDelta::days(i64::from((self.fresh_days)()))
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

    /// Keep `subject`'s new result in memory, then write it through.
    async fn keep_member(&self, subject: &Subject, member: &Member) {
        let fetched_at = self.now();
        self.inner().keep_member(
            subject.ign.key(),
            subject.character_id.clone(),
            member.clone(),
            fetched_at,
            Instant::now() + CACHE_TTL,
        );
        let Some(store) = &self.store else {
            return;
        };
        let estimate = MemberEstimate {
            character_id: subject.character_id.clone(),
            ign_key: subject.ign.key(),
            result: member.into(),
            fetched_at,
        };
        if let Err(error) = store.put_member_estimate(estimate).await {
            unsaved("member", &error);
        }
    }

    /// Keep the whole party's new total in memory, then write it through.
    async fn keep_party(
        &self,
        subjects: &[Subject],
        combined: &BTreeMap<String, Combined>,
        fetched_at: DateTime<Utc>,
    ) {
        let key = set_key(subjects.iter().map(|subject| &subject.ign));
        let ids = characters(subjects);
        self.inner().keep_party(
            key.clone(),
            ids.clone(),
            combined.clone(),
            fetched_at,
            Instant::now() + CACHE_TTL,
        );
        let Some(store) = &self.store else {
            return;
        };
        let estimate = PartyEstimate {
            set_key: key,
            character_ids: ids,
            combined: combined
                .iter()
                .map(|(key, total)| (key.clone(), total.into()))
                .collect(),
            fetched_at,
        };
        if let Err(error) = store.put_party_estimate(estimate).await {
            unsaved("party", &error);
        }
    }

    /// Job `id`'s lookups; `None` when the job is gone.
    async fn run(&self, id: &str) -> Option<Result<Found, Failure>> {
        let (subjects, party, refresh) = {
            let inner = self.inner();
            let kept = inner.jobs.iter().find(|kept| kept.job.id == id)?;
            (
                kept.job.subjects.clone(),
                kept.job.target.party(),
                kept.refresh,
            )
        };
        let igns: Vec<RosterIgn> = subjects.iter().map(|subject| subject.ign.clone()).collect();
        // Each call the job makes adds the client's timeout to its budget;
        // a busy wait spends from the same budget.
        let calls = u32::try_from(igns.len() + usize::from(party)).unwrap_or(u32::MAX);
        let deadline = Instant::now() + self.client.timeout().saturating_mul(calls);
        let mut found = Vec::with_capacity(igns.len());
        let mut first_failure = None;
        for (at, subject) in subjects.iter().enumerate() {
            // Found meanwhile (another job, or before this one was queued),
            // and still fresh.
            let cached = (!refresh)
                .then(|| {
                    let now = self.now();
                    self.inner()
                        .member(&subject.ign, Instant::now())
                        .filter(|held| {
                            held.characters.as_slice()
                                == std::slice::from_ref(&subject.character_id)
                                && self.fresh(held.fetched_at, now)
                        })
                        .map(|held| held.value.clone())
                })
                .flatten();
            let outcome = match cached {
                Some(member) => Ok(member),
                None => {
                    self.edit(id, |job| job.current = Some(at));
                    let result = self
                        .call(deadline, |within| self.client.single(&subject.ign, within))
                        .await;
                    if let Ok(member) = &result {
                        self.keep_member(subject, member).await;
                    }
                    result.map_err(|error| Failure::of(&error, &igns, Some(at)))
                }
            };
            match &outcome {
                Ok(_) => found.push(subject.ign.clone()),
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
        let fetched_at = self.now();
        let (combined, total_failure) = if party {
            let result = self
                .call(deadline, |within| self.client.batch(&found, within))
                .await;
            match result {
                Ok(batch) => {
                    // A whole party, every character found, is kept under
                    // its IGN set.
                    if first_failure.is_none() {
                        self.keep_party(&subjects, &batch.combined, fetched_at)
                            .await;
                    }
                    (Some(batch.combined), None)
                }
                Err(error) => (None, Some(Failure::of(&error, &igns, None))),
            }
        } else {
            (None, None)
        };
        Some(Ok(Found {
            combined,
            total_failure,
            fetched_at,
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
            let kept = &mut inner.jobs[at];
            kept.until = Some(instant + JOB_TTL);
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

/// A result kept in memory that the file did not take.
fn unsaved(kind: &'static str, error: &StoreError) {
    crate::runtime::logging::log!(warn, "estimate_unsaved", kind, error = error_kind(error));
}

/// Codes and counts only: never an IGN or a result.
fn log(outcome: &Result<Found, Failure>, characters: usize, failed: usize) {
    match outcome {
        Ok(found) => crate::runtime::logging::log!(
            if failed > 0 || found.total_failure.is_some() {
                "WARN"
            } else {
                "INFO"
            },
            "estimate_finished",
            state = "done",
            characters,
            failed,
            total_failed = found
                .total_failure
                .as_ref()
                .map(|failure| failure.kind.as_str()),
            cache_hit = found.cache_hit,
        ),
        Err(failure) => crate::runtime::logging::log!(
            warn,
            "estimate_finished",
            state = "failed",
            characters,
            code = failure.kind.as_str(),
            detail = failure.detail,
            status = failure.status,
        ),
    }
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
    /// `url` is shown in health (it holds no secret). Without
    /// [`Self::with_store`] results live in memory only; without
    /// [`Self::with_fresh_days`] they answer lookups for the default
    /// `estimate_days`.
    pub fn new(client: ScouterClient, url: String, clock: Clock) -> Self {
        Self {
            shared: Arc::new(Shared {
                client,
                url,
                clock,
                store: None,
                fresh_days: Arc::new(|| DEFAULT_ESTIMATE_DAYS),
                inner: Mutex::default(),
            }),
            task: Mutex::default(),
        }
    }

    /// Set up before any job starts, while nothing else holds the state.
    fn configure(&mut self) -> &mut Shared {
        Arc::get_mut(&mut self.shared).expect("configured before any job starts")
    }

    /// Write every result through to the character-data file.
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn CharDataStore>) -> Self {
        self.configure().store = Some(store);
        self
    }

    /// Read the saved `estimate_days` from `days` at every lookup.
    #[must_use]
    pub fn with_fresh_days(mut self, days: FreshDays) -> Self {
        self.configure().fresh_days = days;
        self
    }

    /// Load the file's newest [`MAX_CACHED`] results and totals still kept
    /// into memory (serve, at startup, before any lookup). A failed read
    /// starts with nothing loaded. Logs `estimates_loaded`.
    pub async fn load(&self) {
        let Some(store) = &self.shared.store else {
            return;
        };
        let now = self.shared.now();
        let limit = u32::try_from(MAX_CACHED).unwrap_or(u32::MAX);
        let stored = match store.estimates(now, limit).await {
            Ok(stored) => stored,
            Err(error) => {
                crate::runtime::logging::log!(
                    warn,
                    "estimates_loaded",
                    members = 0,
                    parties = 0,
                    error = error_kind(&error),
                );
                return;
            }
        };
        let (members, parties) = (stored.members.len(), stored.parties.len());
        let instant = Instant::now();
        let mut inner = self.shared.inner();
        // Oldest first: the newest of a key wins, and past MAX_CACHED the
        // oldest goes.
        for row in stored.members.into_iter().rev() {
            let until = held_until(row.fetched_at, now, instant);
            inner.keep_member(
                row.ign_key,
                row.character_id,
                row.result.into(),
                row.fetched_at,
                until,
            );
        }
        for row in stored.parties.into_iter().rev() {
            let until = held_until(row.fetched_at, now, instant);
            let combined = row
                .combined
                .into_iter()
                .map(|(key, total)| (key, total.into()))
                .collect();
            inner.keep_party(
                row.set_key,
                row.character_ids,
                combined,
                row.fetched_at,
                until,
            );
        }
        drop(inner);
        crate::runtime::logging::log!(info, "estimates_loaded", members, parties);
    }

    /// Queue a job (or answer it from memory at once while every result it
    /// needs is fresh); the target's own queued or running job is answered
    /// instead of a second one. Spawns the worker on the current tokio
    /// runtime when none runs.
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
        let hit = (!request.refresh)
            .then(|| {
                inner.answer(&request.subjects, party, instant, |fetched_at| {
                    self.shared.fresh(fetched_at, now)
                })
            })
            .flatten();
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
                until: Some(instant + JOB_TTL),
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

    /// `ign`'s newest successful result still in memory, of any age, from a
    /// lookup of that character alone or of a party that included them
    /// (each character is kept under their own IGN as they are found, so
    /// this entry is always the newest), and when it was fetched. Never
    /// starts a lookup.
    pub fn cached(&self, ign: &RosterIgn) -> Option<(Member, DateTime<Utc>)> {
        self.shared
            .inner()
            .member(ign, Instant::now())
            .map(|held| (held.value.clone(), held.fetched_at))
    }

    /// The party total (scouter boss key → total) of the last whole-party
    /// lookup of exactly `igns` still in memory, of any age, and when it
    /// was fetched. Never starts a lookup.
    pub fn cached_party(
        &self,
        igns: &[RosterIgn],
    ) -> Option<(BTreeMap<String, Combined>, DateTime<Utc>)> {
        self.shared
            .inner()
            .party(&set_key(igns.iter()), Instant::now())
            .map(|held| (held.value.clone(), held.fetched_at))
    }

    /// Whether a queued or running job looks up any of `igns`: their
    /// readings are still coming in, so what is cached is only part of
    /// the answer. Never starts a lookup.
    pub fn looking_up(&self, igns: &[RosterIgn]) -> bool {
        self.shared
            .inner()
            .jobs
            .iter()
            .filter(|kept| kept.until.is_none())
            .flat_map(|kept| &kept.job.subjects)
            .any(|subject| {
                igns.iter()
                    .any(|ign| ign.as_str().eq_ignore_ascii_case(subject.ign.as_str()))
            })
    }

    /// Every character a result or total in memory is about, with the IGN
    /// set it is held under (a character's own result: their IGN key), for
    /// the sweeper.
    pub fn held(&self) -> Vec<(String, String)> {
        let inner = self.shared.inner();
        let members = inner
            .members
            .iter()
            .map(|(key, held)| (key, &held.characters));
        let parties = inner
            .parties
            .iter()
            .map(|(key, held)| (key, &held.characters));
        members
            .chain(parties)
            .flat_map(|(key, ids)| ids.iter().map(|id| (id.clone(), key.clone())))
            .collect()
    }

    /// Drop every result and total about `character_id` (the sweeper
    /// deleted them from the file).
    pub fn forget(&self, character_id: &str) {
        let mut inner = self.shared.inner();
        let about = |ids: &Vec<String>| ids.iter().any(|id| id == character_id);
        inner.members.retain(|_, held| !about(&held.characters));
        inner.parties.retain(|_, held| !about(&held.characters));
    }

    /// Drop what is no longer kept at wall-clock `now` (30 days old, or
    /// stamped ahead), as the sweeper does in the file.
    pub fn sweep(&self, now: DateTime<Utc>) {
        let mut inner = self.shared.inner();
        inner
            .members
            .retain(|_, held| estimate_kept(held.fetched_at, now));
        inner
            .parties
            .retain(|_, held| estimate_kept(held.fetched_at, now));
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
mod tests;
