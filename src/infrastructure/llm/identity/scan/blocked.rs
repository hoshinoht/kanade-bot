use std::fmt;

use serde_json::{Value, json};

use crate::infrastructure::llm::governor::Role;

/// What kind of raw identity the scanner found; never the text itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LeakKind {
    /// A masked name, nickname, alias or author label.
    Name,
    /// A roster or issued user id.
    Id,
    /// Any other 17–20 digit run.
    Snowflake,
    /// A pseudonymizing session reported nothing to scan for; fails closed.
    Unscannable,
}

impl LeakKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Id => "id",
            Self::Snowflake => "snowflake",
            Self::Unscannable => "unscannable",
        }
    }
}

/// Scanner hits in one request: sorted distinct kinds and the match count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeakFound {
    pub kinds: Vec<LeakKind>,
    pub count: usize,
}

impl LeakFound {
    pub(super) fn add(&mut self, kind: LeakKind) {
        if let Err(at) = self.kinds.binary_search(&kind) {
            self.kinds.insert(at, kind);
        }
        self.count += 1;
    }

    pub(in crate::infrastructure::llm) fn unscannable() -> Self {
        Self {
            kinds: vec![LeakKind::Unscannable],
            count: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

/// A request refused at the provider boundary because it still carried a raw
/// member identity. Nothing was sent; never retried; refunded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityLeakBlocked {
    pub role: Role,
    pub kinds: Vec<LeakKind>,
    pub count: usize,
}

impl IdentityLeakBlocked {
    /// Event/outcome name for logs and model-log rows.
    pub const EVENT: &'static str = "identity_leak_blocked";

    pub(in crate::infrastructure::llm) fn new(role: Role, found: LeakFound) -> Self {
        Self {
            role,
            kinds: found.kinds,
            count: found.count,
        }
    }

    /// `{role, kinds, count}` for the `identity_leak_blocked` log payload.
    pub fn payload(&self) -> Value {
        json!({
            "role": self.role.as_str(),
            "kinds": self.kinds.iter().map(|kind| kind.as_str()).collect::<Vec<_>>(),
            "count": self.count,
        })
    }
}

impl IdentityLeakBlocked {
    /// A structured WARN line for ports without the runtime logger: the
    /// event name and the payload fields, nothing else.
    pub fn log_line(&self) -> Value {
        let mut line = json!({"level": "WARN", "event": Self::EVENT});
        if let (Some(fields), Value::Object(payload)) = (line.as_object_mut(), self.payload()) {
            fields.extend(payload);
        }
        line
    }
}

impl fmt::Display for IdentityLeakBlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kinds: Vec<&str> = self.kinds.iter().map(|kind| kind.as_str()).collect();
        write!(
            f,
            "{}: {} request refused before sending ({}; {} match{})",
            Self::EVENT,
            self.role.as_str(),
            kinds.join(", "),
            self.count,
            if self.count == 1 { "" } else { "es" }
        )
    }
}

impl std::error::Error for IdentityLeakBlocked {}
