use std::fmt;

/// A roster member as the ports know it; `user_id` is the decimal Discord id.
#[derive(Clone, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub display_name: String,
    pub nickname: Option<String>,
    pub aliases: Vec<String>,
}

impl fmt::Debug for Member {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Member")
            .field("user_id_bytes", &self.user_id.len())
            .field("has_nickname", &self.nickname.is_some())
            .field("alias_count", &self.aliases.len())
            .finish()
    }
}
