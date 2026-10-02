//! In-memory `MemberStore` mirroring the SQLite constraints (valid, unique aliases).

use std::collections::BTreeMap;
use std::sync::MutexGuard;

use crate::domain::members::{
    GatewayMember, MemberProfile, MemberStore, PortalEdit, is_valid_alias,
};
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

    async fn apply_gateway(&self, update: GatewayMember) -> Result<(), StoreError> {
        let mut members = self.members();
        let profile = members.entry(update.user_id.clone()).or_default();
        profile.member.user_id = update.user_id;
        profile.member.display_name = update.display_name;
        profile.member.nickname = update.nickname;
        profile.member.has_role = update.has_role;
        profile.member.is_bot = update.is_bot;
        profile.roles = update.roles;
        profile.is_guild_admin = update.is_guild_admin;
        Ok(())
    }

    async fn member_departed(&self, user_id: &str) -> Result<bool, StoreError> {
        Ok(match self.members().get_mut(user_id) {
            Some(profile) => {
                profile.member.has_role = false;
                profile.roles.clear();
                profile.is_guild_admin = false;
                true
            }
            None => false,
        })
    }

    async fn clear_guild_admin(&self, user_id: &str) -> Result<bool, StoreError> {
        Ok(match self.members().get_mut(user_id) {
            Some(profile) => {
                profile.is_guild_admin = false;
                true
            }
            None => false,
        })
    }

    async fn apply_portal(
        &self,
        user_id: &str,
        edit: PortalEdit,
    ) -> Result<Option<MemberProfile>, StoreError> {
        let mut members = self.members();
        if !members.contains_key(user_id) {
            return Ok(None);
        }
        if let Some(alias) = &edit.add_alias {
            let taken = members
                .values()
                .any(|other| other.member.user_id != user_id && other.aliases.contains(alias));
            if taken || !is_valid_alias(alias) {
                return Err(StoreError::Constraint(format!("alias {alias:?} refused")));
            }
        }
        let Some(profile) = members.get_mut(user_id) else {
            return Ok(None);
        };
        if let Some(level) = edit.ping_level {
            profile.member.ping_level = level;
        }
        if let Some(style) = edit.reply_style {
            profile.reply_style = style;
        }
        if let Some(alias) = edit.add_alias
            && !profile.aliases.contains(&alias)
        {
            profile.aliases.push(alias);
        }
        if let Some(alias) = edit.remove_alias {
            profile.aliases.retain(|held| *held != alias);
        }
        Ok(Some(profile.clone()))
    }
}

impl super::MemoryScheduleStore {
    fn members(&self) -> MemberScheduleGuard<'_> {
        self.members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
