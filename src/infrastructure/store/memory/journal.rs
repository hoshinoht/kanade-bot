//! The in-memory delivery journal, sharing tables (and the revision) with the
//! in-memory schedule store. Each write works on a copy and swaps it in.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use twilight_model::id::Id;
use twilight_model::id::marker::MessageMarker;

use super::{MemoryScheduleStore, Tables};
use crate::bot::delivery::cards::{
    CardRecord, DAY_OF_KIND, DigestPhraseStore, PostedCard, ReminderCardStore,
};
use crate::bot::delivery::debug::{DebugCardStore, PostedDebugCard};
use crate::bot::events::{CardIndex, LookupError, ReplayCard, ReplayCards, ReplayRunCards};
use crate::domain::notify::{
    ActiveClaims, AttemptId, AttemptRecord, AttemptState, Claim, DIGEST_REPLACEMENT_ACTOR,
    DIGEST_REPLACEMENT_REASON, DedupeKey, DeliveryJournal, DeliveryTarget, DigestLog, EffectKind,
    JournalError, Lease, NOT_SENT_ACTOR, NotificationIntent, REJECTED_ACTOR, Receipt, Recovery,
    WeeklyDigest, check_resolution, effect_ordinal, is_sandbox_kind,
};
use crate::domain::scheduler::StoreError;
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
    intended_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
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
    /// Reminder card records by dedupe key, as SQLite `reminder_cards`.
    reminder_cards: BTreeMap<String, CardRecord>,
    /// Digest phrases by native target dedupe key, independent of bound digests.
    digest_phrases: BTreeMap<String, String>,
    card_record_read_failures: usize,
    card_record_write_failures: usize,
    digest_phrase_read_failures: usize,
    digest_phrase_write_failures: usize,
    /// Test cards by attempt id, as SQLite `debug_cards`.
    pub(super) debug_cards: BTreeMap<String, DebugRow>,
}

/// One `/debug ping` test card (SQLite `debug_cards`).
#[derive(Clone, Debug)]
pub(super) struct DebugRow {
    pub(super) run_id: String,
    pub(super) kind: String,
    pub(super) channel_id: String,
    pub(super) message_id: Option<String>,
    pub(super) posted_at: Option<DateTime<Utc>>,
    pub(super) cleared_at: Option<DateTime<Utc>>,
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

    /// Retired by an operator without proof: never reposted. Rejected and
    /// not-sent retirements prove nothing was posted.
    fn card_retired_unproven(&self, proposal_id: &str) -> bool {
        self.attempts.values().any(|row| {
            row.state == AttemptState::Retired
                && row.message_id.is_none()
                && !matches!(
                    row.resolved_by.as_deref(),
                    Some(NOT_SENT_ACTOR | REJECTED_ACTOR)
                )
                && row.targets.iter().any(|target| {
                    target.released
                        && matches!(&target.target, DeliveryTarget::Card(id) if id == proposal_id)
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

    pub(super) fn check_live(&self, lease: &Lease) -> Result<(), JournalError> {
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
            // A refused card stays unposted and may be claimed again.
            DeliveryTarget::Card(_) | DeliveryTarget::Decline { .. } => {}
            DeliveryTarget::DebugCard { .. } => {}
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
    pub(super) fn journal_write<T>(
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
    at: DateTime<Utc>,
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
    let debug = intent.debug_card()?;
    let key = if intent.operation_scoped() {
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
            DeliveryTarget::Card(id) => {
                let unavailable = |detail: String| Err(JournalError::TargetUnavailable(detail));
                let Some((_, card)) = tables.drafts.cards.get(id) else {
                    return unavailable(format!("proposal {id} has no card"));
                };
                if card.message_id.is_some() {
                    return unavailable(format!("proposal {id}'s card was posted"));
                }
                if !tables
                    .drafts
                    .drafts
                    .get(id)
                    .is_some_and(|draft| draft.status.is_live())
                {
                    return unavailable(format!("proposal {id} is closed"));
                }
                if journal.card_retired_unproven(id) {
                    return unavailable(format!(
                        "proposal {id}'s card was retired without proof of delivery"
                    ));
                }
            }
            DeliveryTarget::Decline { run_id, user_id } => {
                tables
                    .declines
                    .claimable(run_id, user_id)
                    .map_err(JournalError::TargetUnavailable)?;
                let notified_at = tables
                    .declines
                    .notified_at(run_id, user_id)
                    .expect("claimable decline notice exists");
                if journal.attempts.values().any(|row| {
                    row.resolved_by.as_deref()
                        == Some(crate::domain::notify::DECLINE_RETRACTION_ACTOR)
                        && row.intended_at >= notified_at
                        && row.targets.iter().any(|held| {
                            held.target
                                == DeliveryTarget::Decline {
                                    run_id: run_id.clone(),
                                    user_id: user_id.clone(),
                                }
                        })
                }) {
                    return Err(JournalError::TargetUnavailable(format!(
                        "decline notice for run {run_id} and member {user_id} was retracted"
                    )));
                }
            }
            DeliveryTarget::DebugCard { kind, .. } if is_sandbox_kind(kind) => {}
            DeliveryTarget::DebugCard { run_id, .. } => {
                if !tables.runs.contains_key(run_id) {
                    return Err(JournalError::TargetUnavailable(format!(
                        "run {run_id} does not exist"
                    )));
                }
            }
        }
    }
    // Test cards are no target rows (SQLite parity): they hold nothing.
    let mut targets: Vec<DeliveryTarget> = intent
        .targets
        .iter()
        .filter(|target| target.is_native())
        .cloned()
        .collect();
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
            intended_at: at,
            resolved_at: None,
            targets: targets
                .into_iter()
                .map(|target| TargetRow {
                    target,
                    released: false,
                })
                .collect(),
        },
    );
    if let Some((run_id, kind)) = debug {
        tables.journal.debug_cards.insert(
            id.clone(),
            DebugRow {
                run_id: run_id.to_owned(),
                kind: kind.to_owned(),
                channel_id: intent.channel_id.clone(),
                message_id: None,
                posted_at: None,
                cleared_at: None,
            },
        );
    }
    Ok(Claim::Fresh(AttemptId(id)))
}

/// Held unless every attempt under the key was proven unsent.
fn claim_source_in(
    tables: &mut Tables,
    lease: &Lease,
    intent: &NotificationIntent,
    source: &str,
    source_ordinal: i64,
    at: DateTime<Utc>,
) -> Result<Claim, JournalError> {
    tables.journal.check_live(lease)?;
    if !intent.targets.is_empty() {
        return Err(JournalError::InvalidInput(
            "source keys claim target-less effects only".into(),
        ));
    }
    let key = DedupeKey::source(source, source_ordinal);
    if tables.journal.attempts.values().any(|row| {
        row.dedupe_key == key
            && (row.dedupe_active || row.resolved_by.as_deref() != Some(NOT_SENT_ACTOR))
    }) {
        return Ok(Claim::Held);
    }
    let ordinal = tables
        .journal
        .attempts
        .values()
        .filter(|row| row.operation_id == lease.operation_id)
        .map(|row| row.ordinal + 1)
        .max()
        .unwrap_or(0);
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
            intended_at: at,
            resolved_at: None,
            targets: Vec::new(),
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
    row.resolved_at = Some(at);
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
            DeliveryTarget::Card(id) => {
                let card = tables
                    .drafts
                    .cards
                    .get_mut(id)
                    .map(|(_, card)| card)
                    .filter(|card| {
                        card.message_id.is_none() && card.channel_id == receipt.channel_id
                    })
                    .ok_or_else(|| {
                        state_changed(format!("proposal {id}'s card is gone or posted"))
                    })?;
                card.message_id = Some(receipt.message_id.clone());
                card.posted_at = Some(super::micros(at));
            }
            DeliveryTarget::Decline { run_id, user_id } => tables
                .declines
                .bind(run_id, user_id, &receipt.channel_id, &receipt.message_id)
                .map_err(state_changed)?,
            DeliveryTarget::DebugCard { .. } => {}
        }
    }
    if let Some(debug) = tables
        .journal
        .debug_cards
        .get_mut(&attempt.0)
        .filter(|row| row.message_id.is_none())
    {
        debug.message_id = Some(receipt.message_id.clone());
        debug.posted_at = Some(super::micros(at));
        if !is_sandbox_kind(&debug.kind) {
            let run_id = debug.run_id.clone();
            tables
                .journal
                .card_runs
                .insert((receipt.message_id.clone(), run_id));
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
        at: DateTime<Utc>,
    ) -> Result<Claim, JournalError> {
        self.journal_write(|tables| claim_in(tables, lease, intent, effect_ordinal, at))
    }

    async fn claim_source(
        &self,
        lease: &Lease,
        intent: &NotificationIntent,
        source: &str,
        ordinal: i64,
        at: DateTime<Utc>,
    ) -> Result<Claim, JournalError> {
        self.journal_write(|tables| claim_source_in(tables, lease, intent, source, ordinal, at))
    }

    async fn bind(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        receipt: &Receipt,
        record_week: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let result = async {
            self.journal_write(|tables| bind_in(tables, lease, attempt, receipt, record_week, at))
        }
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn mark_indeterminate(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
    ) -> Result<(), JournalError> {
        let result = async {
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
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn retire_rejected(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let result = async {
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
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn retire_for_replacement(
        &self,
        lease: &Lease,
        digest: &WeeklyDigest,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let result =
            async { self.journal_write(|tables| replace_in(tables, lease, digest, at)) }.await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn retire_decline_retraction(
        &self,
        lease: &Lease,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
        _at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let result = async {
            self.journal_write(|tables| {
                tables.journal.check_live(lease)?;
                let target = DeliveryTarget::Decline {
                    run_id: run_id.into(),
                    user_id: user_id.into(),
                };
                let attempt = tables.journal.attempts.iter().find_map(|(id, row)| {
                    (row.state == AttemptState::Bound
                        && row.dedupe_active
                        && row.channel_id == channel_id
                        && row.message_id.as_deref() == Some(message_id)
                        && row
                            .targets
                            .iter()
                            .any(|held| !held.released && held.target == target))
                    .then_some(id.clone())
                });
                let Some(attempt) = attempt else {
                    return Err(state_changed("no bound claim matches the decline notice"));
                };
                tables
                    .declines
                    .retract(run_id, user_id, channel_id, message_id)
                    .map_err(state_changed)?;
                let attempt = tables.journal.attempt(&AttemptId(attempt))?;
                retire(
                    attempt,
                    crate::domain::notify::DECLINE_RETRACTION_ACTOR,
                    crate::domain::notify::DECLINE_RETRACTION_REASON,
                );
                tables.revision += 1;
                Ok(())
            })
        }
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn resolve_decline_retract_pending(
        &self,
        lease: &Lease,
        run_id: &str,
        user_id: &str,
        _at: DateTime<Utc>,
    ) -> Result<bool, JournalError> {
        let result = async {
            self.journal_write(|tables| {
                tables.journal.check_live(lease)?;
                let Some(notice) = tables
                    .declines
                    .rows
                    .get_mut(&(run_id.into(), user_id.into()))
                else {
                    return Ok(false);
                };
                if !notice.retract_pending || notice.message_id.is_some() {
                    return Ok(false);
                }
                let target = DeliveryTarget::Decline {
                    run_id: run_id.into(),
                    user_id: user_id.into(),
                };
                let Some(attempt) = tables.journal.attempts.values_mut().find(|row| {
                    row.state == AttemptState::Retired
                        && row.resolved_by.as_deref() == Some(NOT_SENT_ACTOR)
                        && row.targets.iter().any(|held| held.target == target)
                }) else {
                    return Ok(false);
                };
                attempt.resolved_by = Some(crate::domain::notify::DECLINE_RETRACTION_ACTOR.into());
                attempt.reason = crate::domain::notify::DECLINE_RETRACTION_REASON.into();
                notice.retract_pending = false;
                Ok(true)
            })
        }
        .await;
        self.written.after_if(
            crate::infrastructure::store::Written::Delivery,
            result,
            |resolved| *resolved,
        )
    }

    async fn retire_unproven(
        &self,
        attempt: &AttemptId,
        actor: &str,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let result = async {
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
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn release_unsent(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        _at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let result = async {
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
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn record_digest_week(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        let result = async {
            self.journal_write(|tables| {
                tables.journal.check_live(lease)?;
                tables.journal.raise_marker(week, at)
            })
        }
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Delivery, result)
    }

    async fn retire_digests_before(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<usize, JournalError> {
        let result = async {
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
        .await;
        self.written.after_if(
            crate::infrastructure::store::Written::Delivery,
            result,
            |retired| *retired > 0,
        )
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

impl ReplayCards for MemoryScheduleStore {
    async fn replay_cards(
        &self,
        not_before: DateTime<Utc>,
    ) -> Result<Vec<ReplayRunCards>, LookupError> {
        let tables = self.tables();
        let journal = &tables.journal;
        let mut mapped: BTreeMap<(String, String), ReplayCard> = BTreeMap::new();
        let mut add = |run_id: String,
                       message_id: String,
                       channel_id: Option<String>,
                       evidence: bool,
                       resolved_at: Option<DateTime<Utc>>| {
            let card = mapped
                .entry((run_id, message_id.clone()))
                .or_insert(ReplayCard {
                    channel_id,
                    message_id,
                    evidence: false,
                    resolved_at: None,
                });
            card.evidence |= evidence;
            if evidence {
                card.resolved_at = resolved_at;
            }
        };
        for (message, run_id) in &journal.card_runs {
            for row in journal
                .attempts
                .values()
                .filter(|row| row.message_id.as_deref() == Some(message))
            {
                let reminder = journal.reminder_cards.get(row.dedupe_key.as_str());
                let debug = journal
                    .debug_cards
                    .values()
                    .find(|debug| debug.message_id.as_deref() == Some(message));
                let kind = reminder
                    .map(|card| card.kind.as_str())
                    .or_else(|| debug.map(|card| card.kind.as_str()));
                let cleared = debug.is_some_and(|card| card.cleared_at.is_some());
                let evidence = row.state == AttemptState::Bound
                    && row.resolved_at.is_some_and(|at| at >= not_before)
                    && !cleared
                    && kind
                        .is_some_and(|kind| kind == DAY_OF_KIND || kind.starts_with("countdown_"));
                add(
                    run_id.clone(),
                    message.clone(),
                    Some(row.channel_id.clone()),
                    evidence,
                    row.resolved_at,
                );
            }
        }
        for reminder in tables
            .reminders
            .values()
            .filter(|row| row.message_id.is_some())
        {
            let message = reminder.message_id.clone().expect("filtered");
            let attempt = journal
                .attempts
                .values()
                .find(|row| row.message_id.as_deref() == Some(&message));
            let evidence = attempt.is_some_and(|row| {
                row.state == AttemptState::Bound
                    && row.resolved_at.is_some_and(|at| at >= not_before)
                    && journal
                        .reminder_cards
                        .get(row.dedupe_key.as_str())
                        .is_some_and(|card| {
                            card.kind == DAY_OF_KIND || card.kind.starts_with("countdown_")
                        })
            });
            add(
                reminder.run_id.clone(),
                message,
                attempt.map(|row| row.channel_id.clone()),
                evidence,
                attempt.and_then(|row| row.resolved_at),
            );
        }
        let mut runs: BTreeMap<String, Vec<ReplayCard>> = BTreeMap::new();
        for ((run_id, _), card) in mapped {
            runs.entry(run_id).or_default().push(card);
        }
        Ok(runs
            .into_iter()
            .filter_map(|(run_id, cards)| {
                cards
                    .iter()
                    .any(|card| card.evidence)
                    .then_some(ReplayRunCards { run_id, cards })
            })
            .collect())
    }
}

/// SQLite's `reminder_cards` CHECKs.
fn valid_record(dedupe_key: &str, record: &CardRecord) -> bool {
    let key = dedupe_key.len() == 64
        && dedupe_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let kind = record.kind == DAY_OF_KIND || record.kind.starts_with("countdown_");
    key && kind
}

fn valid_phrase(dedupe_key: &str, phrase: &str) -> bool {
    dedupe_key.len() == 64
        && dedupe_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !phrase.trim_matches(' ').is_empty()
        && phrase.chars().count() <= 48
}

impl MemoryScheduleStore {
    /// Test support for a grouped day-of card. Production bindings create
    /// these rows atomically; tests use this to model one message for runs.
    #[cfg(any(test, feature = "test-support"))]
    pub fn test_map_card_run(&self, message_id: &str, run_id: &str) {
        self.tables()
            .journal
            .card_runs
            .insert((message_id.to_owned(), run_id.to_owned()));
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_retire_bound_attempt(&self, attempt: &AttemptId) {
        let mut tables = self.tables();
        let row = tables.journal.attempt(attempt).expect("attempt");
        retire(row, "test", "test retirement");
    }

    pub fn fail_next_card_record_read(&self) {
        self.tables().journal.card_record_read_failures += 1;
    }

    pub fn fail_next_card_record_write(&self) {
        self.tables().journal.card_record_write_failures += 1;
    }

    pub fn fail_next_digest_phrase_read(&self) {
        self.tables().journal.digest_phrase_read_failures += 1;
    }

    pub fn fail_next_digest_phrase_write(&self) {
        self.tables().journal.digest_phrase_write_failures += 1;
    }
}

impl ReminderCardStore for MemoryScheduleStore {
    async fn card_record(&self, dedupe_key: &str) -> Result<Option<CardRecord>, StoreError> {
        let mut tables = self.tables();
        if tables.journal.card_record_read_failures > 0 {
            tables.journal.card_record_read_failures -= 1;
            return Err(StoreError::Backend(
                "injected reminder card record read failure".into(),
            ));
        }
        Ok(tables.journal.reminder_cards.get(dedupe_key).cloned())
    }

    async fn save_card_record(
        &self,
        dedupe_key: &str,
        record: &CardRecord,
        _at: DateTime<Utc>,
    ) -> Result<CardRecord, StoreError> {
        let mut tables = self.tables();
        if tables.journal.card_record_write_failures > 0 {
            tables.journal.card_record_write_failures -= 1;
            return Err(StoreError::Backend(
                "injected reminder card record write failure".into(),
            ));
        }
        if !valid_record(dedupe_key, record) {
            return Err(StoreError::Constraint(
                "reminder card record is invalid".into(),
            ));
        }
        Ok(tables
            .journal
            .reminder_cards
            .entry(dedupe_key.to_owned())
            .or_insert_with(|| record.clone())
            .clone())
    }

    async fn posted_cards(&self, run_id: &str) -> Result<Vec<PostedCard>, StoreError> {
        let tables = self.tables();
        let journal = &tables.journal;
        let mut cards: Vec<PostedCard> = journal
            .attempts
            .values()
            .filter(|row| row.state == AttemptState::Bound && row.effect == "reminder")
            .filter_map(|row| {
                let message = row.message_id.clone()?;
                journal
                    .card_runs
                    .contains(&(message.clone(), run_id.to_owned()))
                    .then_some(())?;
                let record = journal.reminder_cards.get(row.dedupe_key.as_str())?;
                Some(PostedCard {
                    channel_id: row.channel_id.clone(),
                    run_ids: journal
                        .card_runs
                        .iter()
                        .filter(|(id, _)| *id == message)
                        .map(|(_, run)| run.clone())
                        .collect(),
                    message_id: message,
                    record: record.clone(),
                    test: false,
                })
            })
            .collect();
        cards.extend(journal.debug_cards.iter().filter_map(|(attempt, row)| {
            let bound = journal
                .attempts
                .get(attempt)
                .is_some_and(|attempt| attempt.state == AttemptState::Bound);
            let card_kind = row.kind == DAY_OF_KIND || row.kind.starts_with("countdown_");
            (bound && row.run_id == run_id && row.cleared_at.is_none() && card_kind).then(|| {
                Some(PostedCard {
                    channel_id: row.channel_id.clone(),
                    message_id: row.message_id.clone()?,
                    run_ids: vec![row.run_id.clone()],
                    record: CardRecord {
                        kind: row.kind.clone(),
                        heading: None,
                    },
                    test: true,
                })
            })?
        }));
        cards.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(cards)
    }
}

impl DigestPhraseStore for MemoryScheduleStore {
    async fn digest_phrase(&self, dedupe_key: &str) -> Result<Option<String>, StoreError> {
        let mut tables = self.tables();
        if tables.journal.digest_phrase_read_failures > 0 {
            tables.journal.digest_phrase_read_failures -= 1;
            return Err(StoreError::Backend(
                "injected digest phrase read failure".into(),
            ));
        }
        Ok(tables.journal.digest_phrases.get(dedupe_key).cloned())
    }

    async fn save_digest_phrase(
        &self,
        dedupe_key: &str,
        phrase: &str,
        _at: DateTime<Utc>,
    ) -> Result<String, StoreError> {
        let mut tables = self.tables();
        if tables.journal.digest_phrase_write_failures > 0 {
            tables.journal.digest_phrase_write_failures -= 1;
            return Err(StoreError::Backend(
                "injected digest phrase write failure".into(),
            ));
        }
        if !valid_phrase(dedupe_key, phrase) {
            return Err(StoreError::Constraint(
                "digest card phrase record is invalid".into(),
            ));
        }
        Ok(tables
            .journal
            .digest_phrases
            .entry(dedupe_key.to_owned())
            .or_insert_with(|| phrase.to_owned())
            .clone())
    }
}

impl DebugCardStore for MemoryScheduleStore {
    async fn debug_cards_in(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<PostedDebugCard>, StoreError> {
        let tables = self.tables();
        let mut cards: Vec<PostedDebugCard> = tables
            .journal
            .debug_cards
            .values()
            .filter(|row| row.channel_id == channel_id && row.cleared_at.is_none())
            .filter_map(|row| {
                let posted_at = row.posted_at.filter(|at| *at >= since)?;
                Some(PostedDebugCard {
                    channel_id: row.channel_id.clone(),
                    message_id: row.message_id.clone()?,
                    run_id: row.run_id.clone(),
                    kind: row.kind.clone(),
                    posted_at,
                })
            })
            .collect();
        cards.sort_by(|a, b| (a.posted_at, &a.message_id).cmp(&(b.posted_at, &b.message_id)));
        Ok(cards)
    }

    async fn clear_debug_card(
        &self,
        message_id: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let mut tables = self.tables();
        let row =
            tables.journal.debug_cards.values_mut().find(|row| {
                row.message_id.as_deref() == Some(message_id) && row.cleared_at.is_none()
            });
        Ok(row.is_some_and(|row| {
            row.cleared_at = Some(super::micros(at));
            true
        }))
    }
}
