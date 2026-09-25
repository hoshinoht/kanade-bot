//! In-memory `MemberStore` mirroring the SQLite constraints (valid, unique aliases).

use std::collections::BTreeMap;
use std::sync::MutexGuard;

use crate::domain::members::{MemberProfile, MemberStore, is_valid_alias};
use crate::domain::scheduler::StoreError;

type MemberScheduleGuard<'a> = MutexGuard<'a, BTreeMap<String, MemberProfile>>;

impl MemberStore for super::MemoryScheduleStore {
    async fn list_members(&self) -> Result<Vec<MemberProfile>, StoreError> {
        Ok(self.members().values().cloned().collect())
    }

    async fn load_member(&self, user_id: &str) -> Result<Option<MemberProfile>, StoreError> {
        Ok(self.members().get(user_id).cloned())
    }

    async fn put_member(&self, profile: MemberProfile) -> Result<(), StoreError> {
        let mut members = self.members();
        let user_id = &profile.member.user_id;
        let mut seen = std::collections::BTreeSet::new();
        for alias in &profile.aliases {
            let taken = members
                .values()
                .any(|other| other.member.user_id != *user_id && other.aliases.contains(alias));
            if !is_valid_alias(alias) || taken || !seen.insert(alias) {
                return Err(StoreError::Constraint(format!("alias {alias:?} refused")));
            }
        }
        members.insert(user_id.clone(), profile);
        Ok(())
    }
}

impl super::MemoryScheduleStore {
    fn members(&self) -> MemberScheduleGuard<'_> {
        self.members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
