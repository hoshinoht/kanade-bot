//! In-memory decline-notice rows, swapped with the deciding schedule commit.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::{MemoryScheduleStore, micros};
use crate::domain::notify::{
    DECLINE_NOTICE_COOLDOWN, DeclineNotice, DeclineNoticeStore, MAX_PENDING_DECLINE_NOTICES,
};
use crate::domain::scheduler::StoreError;

#[derive(Clone, Debug, Default)]
pub(super) struct DeclineNotices {
    pub(super) rows: BTreeMap<(String, String), DeclineNotice>,
}

impl DeclineNotices {
    pub(super) fn upsert(&mut self, mut candidate: DeclineNotice) -> Result<(), StoreError> {
        let Some(display_name) = &candidate.display_name else {
            return Err(StoreError::Constraint(
                "decline notice candidate needs a display name".into(),
            ));
        };
        if display_name.chars().count() > 128 {
            return Err(StoreError::Constraint(
                "decline notice display name exceeds 128 characters".into(),
            ));
        }
        candidate.notified_at = micros(candidate.notified_at);
        candidate.message_id = None;
        candidate.retract_pending = false;
        let key = (candidate.run_id.clone(), candidate.user_id.clone());
        if self
            .rows
            .get(&key)
            .is_none_or(|existing| existing.message_id.is_none())
        {
            self.rows.insert(key, candidate);
        }
        Ok(())
    }

    pub(super) fn claimable(&self, run_id: &str, user_id: &str) -> Result<(), String> {
        match self.rows.get(&(run_id.into(), user_id.into())) {
            None => Err(format!(
                "decline notice for run {run_id} and member {user_id} does not exist"
            )),
            Some(notice) if notice.message_id.is_some() => Err(format!(
                "decline notice for run {run_id} and member {user_id} is already bound"
            )),
            Some(_) => Ok(()),
        }
    }

    pub(super) fn notified_at(&self, run_id: &str, user_id: &str) -> Option<DateTime<Utc>> {
        self.rows
            .get(&(run_id.into(), user_id.into()))
            .map(|notice| notice.notified_at)
    }

    pub(super) fn bind(
        &mut self,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), String> {
        self.claimable(run_id, user_id)?;
        let notice = self
            .rows
            .get_mut(&(run_id.into(), user_id.into()))
            .expect("claimable notice exists");
        notice.channel_id = Some(channel_id.into());
        notice.message_id = Some(message_id.into());
        Ok(())
    }

    pub(super) fn retract(
        &mut self,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), String> {
        let Some(notice) = self.rows.get_mut(&(run_id.into(), user_id.into())) else {
            return Err("decline retraction native row changed".into());
        };
        if notice.channel_id.as_deref() != Some(channel_id)
            || notice.message_id.as_deref() != Some(message_id)
        {
            return Err("decline retraction native row changed".into());
        }
        notice.message_id = None;
        notice.retract_pending = false;
        Ok(())
    }
}

impl DeclineNoticeStore for MemoryScheduleStore {
    async fn commit_with_decline_notices(
        &self,
        expected_revision: u64,
        changes: crate::domain::schedule::ChangeSet,
        meta: crate::domain::history::ChangeMeta,
        candidates: Vec<DeclineNotice>,
        retractions: Vec<(String, String)>,
    ) -> Result<Option<crate::domain::scheduler::Committed>, StoreError> {
        self.commit_with_declines(expected_revision, changes, meta, candidates, retractions)
    }

    async fn decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Result<Option<DeclineNotice>, StoreError> {
        Ok(self
            .tables()
            .declines
            .rows
            .get(&(run_id.into(), user_id.into()))
            .cloned())
    }

    async fn pending_decline_notices(
        &self,
        limit: usize,
    ) -> Result<Vec<DeclineNotice>, StoreError> {
        let mut pending: Vec<DeclineNotice> = self
            .tables()
            .declines
            .rows
            .values()
            .filter(|notice| notice.message_id.is_none() || notice.retract_pending)
            .cloned()
            .collect();
        pending.sort_by(|left, right| {
            (left.notified_at, &left.run_id, &left.user_id).cmp(&(
                right.notified_at,
                &right.run_id,
                &right.user_id,
            ))
        });
        pending.truncate(limit.min(MAX_PENDING_DECLINE_NOTICES));
        Ok(pending)
    }

    async fn bind_decline_notice(
        &self,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<bool, StoreError> {
        let mut tables = self.tables();
        let Some(notice) = tables
            .declines
            .rows
            .get_mut(&(run_id.into(), user_id.into()))
        else {
            return Ok(false);
        };
        if notice.message_id.is_some() {
            return Ok(false);
        }
        notice.channel_id = Some(channel_id.into());
        notice.message_id = Some(message_id.into());
        Ok(true)
    }

    async fn mark_decline_retract_pending(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Result<bool, StoreError> {
        let mut tables = self.tables();
        let Some(notice) = tables
            .declines
            .rows
            .get_mut(&(run_id.into(), user_id.into()))
        else {
            return Ok(false);
        };
        notice.retract_pending = true;
        Ok(true)
    }

    async fn clear_decline_notice_message(
        &self,
        run_id: &str,
        user_id: &str,
        message_id: &str,
    ) -> Result<bool, StoreError> {
        let mut tables = self.tables();
        let Some(notice) = tables
            .declines
            .rows
            .get_mut(&(run_id.into(), user_id.into()))
        else {
            return Ok(false);
        };
        if notice.message_id.as_deref() != Some(message_id) {
            return Ok(false);
        }
        notice.message_id = None;
        notice.retract_pending = false;
        Ok(true)
    }

    async fn decline_notice_on_cooldown(
        &self,
        run_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        Ok(self
            .tables()
            .declines
            .rows
            .get(&(run_id.into(), user_id.into()))
            .is_some_and(|notice| {
                now.signed_duration_since(notice.notified_at) < DECLINE_NOTICE_COOLDOWN
            }))
    }
}
