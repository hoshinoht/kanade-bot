//! Who may approve (✅) or reject (❌) a proposal: v4 `may_commit`, kept by
//! user decision (2026-09-25).

/// What the caller knows about the reacting member. Role and admin checks
/// belong to the caller (`is_admin` also covers the guild owner, as v4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approver {
    pub user_id: String,
    /// Holds the bossing role (live member, else the synced roster).
    pub has_role: bool,
    pub is_admin: bool,
}

/// v4 `may_commit`: an admin or owner always; otherwise the bossing role
/// and, with a target run, being on it, or without one, being named (a
/// change that names nobody is anyone's with the role).
pub fn may_commit(
    named: &[String],
    run_participants: Option<&[String]>,
    user_id: &str,
    has_role: bool,
    is_admin: bool,
    is_owner: bool,
) -> bool {
    if is_admin || is_owner {
        return true;
    }
    if !has_role {
        return false;
    }
    match run_participants {
        Some(participants) => participants.iter().any(|uid| uid == user_id),
        None => named.is_empty() || named.iter().any(|uid| uid == user_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn participants_admins_owners_and_named_members_may_commit() {
        let party = ["1".to_owned(), "2".to_owned()];
        assert!(may_commit(&[], Some(&party), "2", true, false, false));
        assert!(!may_commit(&[], Some(&party), "9", true, false, false));
        assert!(!may_commit(&[], Some(&party), "2", false, false, false));
        assert!(may_commit(&[], Some(&party), "9", false, true, false));
        assert!(may_commit(&[], Some(&party), "9", false, false, true));
        assert!(may_commit(&["4".into()], None, "4", true, false, false));
        assert!(!may_commit(&["4".into()], None, "9", true, false, false));
        assert!(may_commit(&[], None, "9", true, false, false));
        assert!(!may_commit(&[], None, "9", false, false, false));
    }
}
