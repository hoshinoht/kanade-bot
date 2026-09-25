//! The in-memory delivery journal, sharing tables (and the revision) with the
//! in-memory schedule store. Each write works on a copy and swaps it in.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use twilight_model::id::Id;
use twilight_model::id::marker::MessageMarker;

use super::{MemoryScheduleStore, Tables};
use crate::bot::events::{CardIndex, LookupError};
use crate::domain::notify::{
    ActiveClaims, AttemptId, AttemptRecord, AttemptState, Claim, DIGEST_REPLACEMENT_ACTOR,
    DIGEST_REPLACEMENT_REASON, DedupeKey, DeliveryJournal, DeliveryTarget, DigestLog, EffectKind,
    JournalError, Lease, NOT_SENT_ACTOR, NotificationIntent, REJECTED_ACTOR, Receipt, Recovery,
    WeeklyDigest, check_resolution, effect_ordinal,
};
use crate::domain::time::{from_iso, to_iso};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    Live,
    Orphaned,
    Retired,
}

#[derive(Clone, Debug)]
struct LeaseRow {
    token_hash: String,
    lifecycle: Lifecycle,
}

#[derive(Clone, Debug)]
struct TargetRow {
    target: DeliveryTarget,
    released: bool,
}

#[derive(Clone, Debug)]
struct AttemptRow {
    operation_id: String,
    ordinal: i64,
    effect: String,
    dedupe_key: DedupeKey,
    dedupe_active: bool,
    state: AttemptState,
    channel_id: String,
    message_id: Option<String>,
    resolved_by: Option<String>,
    reason: String,
    targets: Vec<TargetRow>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct JournalTables {
    leases: BTreeMap<String, LeaseRow>,
    attempts: BTreeMap<String, AttemptRow>,
    digests: BTreeMap<DateTime<Utc>, WeeklyDigest>,
    marker: Option<String>,
    /// `(message_id, run_id)` written at bind, as SQLite `delivery_card_runs`.
    card_runs: BTreeSet<(String, String)>,
}

impl JournalTables {
    /// v4 `unproven_retirement_exists` for one reminder.
    pub(super) fn retired_unproven(&self, reminder_id: &str) -> bool {
        self.attempts.values().any(|row| {
            row.state == AttemptState::Retired
                && row.message_id.is_none()
                && row.resolved_by.as_deref() != Some(NOT_SENT_ACTOR)
                && row.targets.iter().any(|target| {
                    target.released
                        && matches!(&target.target, DeliveryTarget::Reminder(id) if id == reminder_id)
                })
        })
    }

    fn holds(&self, target: &DeliveryTarget) -> bool {
        self.attempts.values().any(|row| {
            row.dedupe_active
                && row
                    .targets
                    .iter()
                    .any(|held| !held.released && &held.target == target)
        })
    }

    fn check_live(&self, lease: &Lease) -> Result<(), JournalError> {
        match self.leases.get(&lease.operation_id) {
            Some(row)
                if row.lifecycle == Lifecycle::Live && row.token_hash == lease.token_hash() =>
            {
                Ok(())
            }
            _ => Err(JournalError::LeaseNotLive),
        }
    }

    fn attempt(&mut self, id: &AttemptId) -> Result<&mut AttemptRow, JournalError> {
        self.attempts
            .get_mut(&id.0)
            .ok_or_else(|| JournalError::StateChanged(format!("attempt {id} does not exist")))
    }

    /// v4 `raise_digest_marker`: never backwards, never to a future week.
    fn raise_marker(&mut self, week: DateTime<Utc>, at: DateTime<Utc>) -> Result<(), JournalError> {
        if week > at {
            return Ok(());
        }
        if let Some(marker) = &self.marker
            && from_iso(marker).is_ok_and(|marker| marker >= week)
        {
            return Ok(());
        }
        self.marker = Some(to_iso(&week)?);
        Ok(())
    }
}

fn state_changed(detail: impl Into<String>) -> JournalError {
    JournalError::StateChanged(detail.into())
}

/// Mark each unsent reminder handled without a message (v4's skip shape) and
/// raise the digest marker, so a retired send is never retried.
fn suppress_natives(
    tables: &mut Tables,
    targets: &[TargetRow],
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    for target in targets {
        match &target.target {
            DeliveryTarget::Reminder(id) => {
                if let Some(row) = tables.reminders.get_mut(id) {
                    if row.message_id.is_some() {
                        return Err(state_changed(format!(
                            "reminder {id} is bound to another message"
                        )));
                    }
                    row.sent_at = row.sent_at.or(Some(at));
                }
            }
            DeliveryTarget::Digest(week) => tables.journal.raise_marker(*week, at)?,
        }
    }
    Ok(())
}

fn retire(row: &mut AttemptRow, actor: &str, reason: &str) {
    for target in &mut row.targets {
        target.released = true;
    }
    row.state = AttemptState::Retired;
    row.dedupe_active = false;
    row.resolved_by = Some(actor.to_owned());
    reason.clone_into(&mut row.reason);
}

impl MemoryScheduleStore {
    /// Apply `write` to a copy of the tables; keep it only on success.
    fn journal_write<T>(
        &self,
        write: impl FnOnce(&mut Tables) -> Result<T, JournalError>,
    ) -> Result<T, JournalError> {
        let mut tables = self.tables();
        let mut next = tables.clone();
        let value = write(&mut next)?;
        *tables = next;
        Ok(value)
    }
}

fn claim_in(
    tables: &mut Tables,
    lease: &Lease,
    intent: &NotificationIntent,
    requested: Option<i64>,
) -> Result<Claim, JournalError> {
    tables.journal.check_live(lease)?;
    let next = tables
        .journal
        .attempts
        .values()
        .filter(|row| row.operation_id == lease.operation_id)
        .map(|row| row.ordinal + 1)
        .max()
        .unwrap_or(0);
    let ordinal = effect_ordinal(intent, requested, next)?;
    if ordinal < next {
        return Ok(Claim::Held);
    }
    let key = if intent.targets.is_empty() {
        DedupeKey::operation(&lease.operation_id, ordinal)
    } else {
        DedupeKey::native(&intent.targets)?
    };
    let journal = &tables.journal;
    if journal
        .attempts
        .values()
        .any(|row| row.dedupe_active && row.dedupe_key == key)
        || intent.targets.iter().any(|target| journal.holds(target))
    {
        return Ok(Claim::Held);
    }
    for target in &intent.targets {
        match target {
            DeliveryTarget::Reminder(id) => {
                let row = tables.reminders.get(id).ok_or_else(|| {
                    JournalError::TargetUnavailable(format!("reminder {id} does not exist"))
                })?;
                if row.sent_at.is_some() || row.message_id.is_some() {
                    return Err(JournalError::TargetUnavailable(format!(
                        "reminder {id} was already sent"
                    )));
                }
                if journal.retired_unproven(id) {
                    return Err(JournalError::TargetUnavailable(format!(
                        "reminder {id} was retired without proof of delivery"
                    )));
                }
            }
            DeliveryTarget::Digest(week) => {
                if journal
                    .digests
                    .get(week)
                    .is_some_and(|row| row.retired_at.is_none())
                {
                    return Err(JournalError::TargetUnavailable(format!(
                        "digest for {} already has an active card",
                        to_iso(week)?
                    )));
                }
            }
        }
    }
    let mut targets: Vec<DeliveryTarget> = intent.targets.clone();
    targets.sort();
    targets.dedup();
    let id = uuid::Uuid::new_v4().to_string();
    tables.journal.attempts.insert(
        id.clone(),
        AttemptRow {
            operation_id: lease.operation_id.clone(),
            ordinal,
            effect: intent.effect.as_str().to_owned(),
            dedupe_key: key,
            dedupe_active: true,
            state: AttemptState::Intent,
            channel_id: intent.channel_id.clone(),
            message_id: None,
            resolved_by: None,
            reason: String::new(),
            targets: targets
                .into_iter()
                .map(|target| TargetRow {
                    target,
                    released: false,
                })
                .collect(),
        },
    );
    Ok(Claim::Fresh(AttemptId(id)))
}

fn bind_in(
    tables: &mut Tables,
    lease: &Lease,
    attempt: &AttemptId,
    receipt: &Receipt,
    record_week: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    tables.journal.check_live(lease)?;
    let row = tables.journal.attempt(attempt)?;
    if row.state != AttemptState::Intent {
        return Err(state_changed(format!(
            "attempt {attempt} is {}",
            row.state.as_str()
        )));
    }
    if row.channel_id != receipt.channel_id {
        return Err(state_changed(format!(
            "receipt channel {} is not the claimed channel {}",
            receipt.channel_id, row.channel_id
        )));
    }
    row.state = AttemptState::Bound;
    row.message_id = Some(receipt.message_id.clone());
    let targets = row.targets.clone();
    for target in &targets {
        match &target.target {
            DeliveryTarget::Reminder(id) => {
                let reminder = tables
                    .reminders
                    .get_mut(id)
                    .filter(|row| row.sent_at.is_none() && row.message_id.is_none())
                    .ok_or_else(|| state_changed(format!("reminder {id} is gone or sent")))?;
                reminder.sent_at = Some(at);
                reminder.message_id = Some(receipt.message_id.clone());
                let run_id = reminder.run_id.clone();
                tables
                    .journal
                    .card_runs
                    .insert((receipt.message_id.clone(), run_id));
            }
            DeliveryTarget::Digest(week) => {
                if tables
                    .journal
                    .digests
                    .get(week)
                    .is_some_and(|row| row.retired_at.is_none())
                {
                    return Err(state_changed("the week already has an active digest"));
                }
                tables.journal.digests.insert(
                    *week,
                    WeeklyDigest {
                        week_start: *week,
                        channel_id: receipt.channel_id.clone(),
                        message_id: receipt.message_id.clone(),
                        posted_at: at,
                        retired_at: None,
                    },
                );
            }
        }
    }
    if let Some(week) = record_week {
        tables.journal.raise_marker(week, at)?;
    }
    tables.revision += 1;
    Ok(())
}

fn replace_in(
    tables: &mut Tables,
    lease: &Lease,
    digest: &WeeklyDigest,
    at: DateTime<Utc>,
) -> Result<(), JournalError> {
    tables.journal.check_live(lease)?;
    let target = DeliveryTarget::Digest(digest.week_start);
    let active = tables
        .journal
        .digests
        .get(&digest.week_start)
        .is_some_and(|row| {
            row.retired_at.is_none()
                && row.channel_id == digest.channel_id
                && row.message_id == digest.message_id
        });
    let claim = tables.journal.attempts.values_mut().find(|row| {
        row.state == AttemptState::Bound
            && row.dedupe_active
            && row.effect == EffectKind::Digest.as_str()
            && row.channel_id == digest.channel_id
            && row.message_id.as_deref() == Some(digest.message_id.as_str())
            && row
                .targets
                .iter()
                .any(|held| !held.released && held.target == target)
    });
    let (true, Some(claim)) = (active, claim) else {
        return Err(state_changed("no bound claim matches the digest card"));
    };
    retire(claim, DIGEST_REPLACEMENT_ACTOR, DIGEST_REPLACEMENT_REASON);
    if let Some(row) = tables.journal.digests.get_mut(&digest.week_start) {
        row.retired_at = Some(at);
    }
    tables.revision += 1;
    Ok(())
}

impl DeliveryJournal for MemoryScheduleStore {
    async fn load_view(&self) -> Result<ActiveClaims, JournalError> {
        let tables = self.tables();
        let held: BTreeSet<DeliveryTarget> = tables
            .journal
            .attempts
            .values()
            .filter(|row| row.dedupe_active && row.state.is_unresolved())
            .flat_map(|row| row.targets.iter())
            .filter(|target| !target.released)
            .map(|target| target.target.clone())
            .collect();
        Ok(ActiveClaims::new(held))
    }

    async fn load_attempt(
        &self,
        attempt: &AttemptId,
    ) -> Result<Option<AttemptRecord>, JournalError> {
        let tables = self.tables();
        Ok(tables
            .journal
            .attempts
            .get(&attempt.0)
            .map(|row| AttemptRecord {
                id: attempt.clone(),
                state: row.state,
                channel_id: row.channel_id.clone(),
                message_id: row.message_id.clone(),
                resolved_by: row.resolved_by.clone(),
                resolution_reason: row.reason.clone(),
                targets: row.targets.iter().map(|row| row.target.clone()).collect(),
            }))
    }

    async fn load_digests(&self) -> Result<DigestLog, JournalError> {
        let tables = self.tables();
        Ok(DigestLog {
            last_digest_week: tables.journal.marker.clone(),
            digests: tables.journal.digests.values().cloned().collect(),
        })
    }

    async fn begin_lease(
        &self,
        instance_id: &str,
        _operation_kind: &str,
        _at: DateTime<Utc>,
    ) -> Result<Lease, JournalError> {
        let lease = Lease::new(
            uuid::Uuid::new_v4().to_string(),
            instance_id.to_owned(),
            0,
            uuid::Uuid::new_v4().to_string(),
        );
        self.journal_write(|tables| {
            tables.journal.leases.insert(
                lease.operation_id.clone(),
                LeaseRow {
                    token_hash: lease.token_hash(),
                    lifecycle: Lifecycle::Live,
                },
            );
            Ok(())
        })?;
        Ok(lease)
    }

    async fn end_lease(&self, lease: &Lease, _at: DateTime<Utc>) -> Result<(), JournalError> {
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            if let Some(row) = tables.journal.leases.get_mut(&lease.operation_id) {
                row.lifecycle = Lifecycle::Retired;
            }
            Ok(())
        })
    }

    async fn claim(
        &self,
        lease: &Lease,
        intent: &NotificationIntent,
        effect_ordinal: Option<i64>,
        _at: DateTime<Utc>,
    ) -> Result<Claim, JournalError> {
        self.journal_write(|tables| claim_in(tables, lease, intent, effect_ordinal))
    }

    async fn bind(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        receipt: &Receipt,
        record_week: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        self.journal_write(|tables| bind_in(tables, lease, attempt, receipt, record_week, at))
    }

    async fn mark_indeterminate(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
    ) -> Result<(), JournalError> {
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            let row = tables.journal.attempt(attempt)?;
            if row.state != AttemptState::Intent {
                return Err(state_changed(format!(
                    "attempt {attempt} is {}",
                    row.state.as_str()
                )));
            }
            row.state = AttemptState::Indeterminate;
            Ok(())
        })
    }

    async fn retire_rejected(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        check_resolution(REJECTED_ACTOR, reason)?;
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            let row = tables.journal.attempt(attempt)?;
            if row.state != AttemptState::Intent {
                return Err(state_changed(format!(
                    "attempt {attempt} is {}",
                    row.state.as_str()
                )));
            }
            retire(row, REJECTED_ACTOR, reason);
            let targets = row.targets.clone();
            suppress_natives(tables, &targets, at)?;
            tables.revision += 1;
            Ok(())
        })
    }

    async fn retire_for_replacement(
        &self,
        lease: &Lease,
        digest: &WeeklyDigest,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        self.journal_write(|tables| replace_in(tables, lease, digest, at))
    }

    async fn retire_unproven(
        &self,
        attempt: &AttemptId,
        actor: &str,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        check_resolution(actor, reason)?;
        self.journal_write(|tables| {
            let operation_live = {
                let row = tables.journal.attempt(attempt)?;
                if !row.state.is_unresolved() || !row.dedupe_active {
                    return Err(state_changed(format!("attempt {attempt} is resolved")));
                }
                if row.targets.iter().any(|target| target.released) {
                    return Err(state_changed("an unresolved attempt has a released target"));
                }
                let operation = row.operation_id.clone();
                tables
                    .journal
                    .leases
                    .get(&operation)
                    .is_some_and(|lease| lease.lifecycle == Lifecycle::Live)
            };
            if operation_live {
                return Err(state_changed("the attempt's own lease is still live"));
            }
            let row = tables.journal.attempt(attempt)?;
            retire(row, actor, reason);
            let targets = row.targets.clone();
            suppress_natives(tables, &targets, at)?;
            tables.revision += 1;
            Ok(())
        })
    }

    async fn release_unsent(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        _at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        check_resolution(NOT_SENT_ACTOR, reason)?;
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            let row = tables.journal.attempt(attempt)?;
            if row.state != AttemptState::Intent {
                return Err(state_changed(format!(
                    "attempt {attempt} is {}",
                    row.state.as_str()
                )));
            }
            if row.operation_id != lease.operation_id {
                return Err(state_changed(format!(
                    "attempt {attempt} belongs to another operation"
                )));
            }
            retire(row, NOT_SENT_ACTOR, reason);
            Ok(())
        })
    }

    async fn record_digest_week(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            tables.journal.raise_marker(week, at)
        })
    }

    async fn retire_digests_before(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<usize, JournalError> {
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            let mut retired = 0;
            for row in tables.journal.digests.values_mut() {
                if row.week_start < week && row.retired_at.is_none() {
                    row.retired_at = Some(at);
                    retired += 1;
                }
            }
            if retired > 0 {
                tables.revision += 1;
            }
            Ok(retired)
        })
    }

    async fn recover_on_start(&self, _at: DateTime<Utc>) -> Result<Recovery, JournalError> {
        self.journal_write(|tables| {
            let mut recovery = Recovery::default();
            for (id, row) in &mut tables.journal.attempts {
                if row.state == AttemptState::Intent {
                    row.state = AttemptState::Indeterminate;
                    recovery.indeterminate.push(AttemptId(id.clone()));
                }
            }
            for lease in tables.journal.leases.values_mut() {
                if lease.lifecycle == Lifecycle::Live {
                    lease.lifecycle = Lifecycle::Orphaned;
                    recovery.orphaned_leases += 1;
                }
            }
            Ok(recovery)
        })
    }
}

impl CardIndex for MemoryScheduleStore {
    async fn runs_for_message(
        &self,
        message: Id<MessageMarker>,
    ) -> Result<Vec<String>, LookupError> {
        let message = message.get().to_string();
        let tables = self.tables();
        let mut runs: BTreeSet<String> = tables
            .journal
            .card_runs
            .iter()
            .filter(|(id, _)| *id == message)
            .map(|(_, run_id)| run_id.clone())
            .collect();
        runs.extend(
            tables
                .reminders
                .values()
                .filter(|row| row.message_id.as_deref() == Some(message.as_str()))
                .map(|row| row.run_id.clone()),
        );
        Ok(runs.into_iter().collect())
    }
}
