//! In-memory `OwnerRequestStore`, mirroring SQLite's `owner_requests`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::domain::ownership::{OwnerRequest, OwnerRequestStatus, OwnerRequestStore};
use crate::domain::scheduler::StoreError;

pub(super) type OwnerRequestTable = BTreeMap<String, OwnerRequest>;

impl super::MemoryScheduleStore {
    fn owner_requests(&self) -> std::sync::MutexGuard<'_, OwnerRequestTable> {
        self.owner_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl OwnerRequestStore for super::MemoryScheduleStore {
    async fn create_owner_request(&self, request: OwnerRequest) -> Result<(), StoreError> {
        request.check()?;
        let mut table = self.owner_requests();
        let duplicate = table.values().any(|open| {
            open.status == OwnerRequestStatus::Open
                && open.fixed_run_id == request.fixed_run_id
                && open.requester == request.requester
        });
        if duplicate || table.contains_key(&request.id) {
            return Err(StoreError::Constraint(
                "an open owner request exists for this member and timing".into(),
            ));
        }
        table.insert(request.id.clone(), request);
        Ok(())
    }

    async fn owner_request(&self, id: &str) -> Result<Option<OwnerRequest>, StoreError> {
        Ok(self.owner_requests().get(id).cloned())
    }

    async fn open_owner_requests(&self) -> Result<Vec<OwnerRequest>, StoreError> {
        let mut open: Vec<OwnerRequest> = self
            .owner_requests()
            .values()
            .filter(|request| request.status == OwnerRequestStatus::Open)
            .cloned()
            .collect();
        open.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(open)
    }

    async fn close_owner_request(
        &self,
        id: &str,
        status: OwnerRequestStatus,
        decided_by: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        if status == OwnerRequestStatus::Open {
            return Err(StoreError::Constraint("closing to open".into()));
        }
        let mut table = self.owner_requests();
        let Some(request) = table
            .get_mut(id)
            .filter(|r| r.status == OwnerRequestStatus::Open)
        else {
            return Ok(false);
        };
        request.status = status;
        request.decided_by = Some(decided_by.to_owned());
        request.decided_at = Some(at);
        Ok(true)
    }

    async fn unsettled_owner_requests(&self) -> Result<Vec<OwnerRequest>, StoreError> {
        let mut closed: Vec<OwnerRequest> = self
            .owner_requests()
            .values()
            .filter(|r| {
                r.status != OwnerRequestStatus::Open && r.message_id.is_some() && !r.message_settled
            })
            .cloned()
            .collect();
        closed.sort_by(|a, b| (a.decided_at, &a.id).cmp(&(b.decided_at, &b.id)));
        Ok(closed)
    }

    async fn settle_owner_request_message(&self, id: &str) -> Result<(), StoreError> {
        if let Some(request) = self.owner_requests().get_mut(id) {
            request.message_settled = true;
        }
        Ok(())
    }

    async fn set_owner_request_message(
        &self,
        id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), StoreError> {
        if let Some(request) = self.owner_requests().get_mut(id) {
            request.channel_id = Some(channel_id.to_owned());
            request.message_id = Some(message_id.to_owned());
        }
        Ok(())
    }
}
