//! The admin Model view of a masked chat turn (user decision D7): each
//! round's request exactly as the model received it, the model's raw reply
//! and tool-call arguments before names were restored, the final reply
//! members saw, and the turn's token → member mapping. Stored only when
//! pseudonymization is on, with the chat log (same retention, admin only);
//! never logged, and `Debug` shows sizes only.

use std::fmt;

use serde_json::Value;

use crate::domain::scheduler::StoreError;

/// One model request of a masked turn.
#[derive(Clone, PartialEq, Eq)]
pub struct MaskedRound {
    /// Tool-loop round (the clean retry repeats the last round number).
    pub round: u32,
    pub clean: bool,
    /// The request messages as sent (masked); always a JSON array.
    pub request: Value,
    /// The model's reply text before decoding.
    pub reply: Option<String>,
    /// `[{"name", "arguments"}]` before decoding; always a JSON array.
    pub tool_calls: Value,
}

/// One token the turn issued and whom it stood for.
#[derive(Clone, PartialEq, Eq)]
pub struct MaskedName {
    pub token: String,
    pub user_id: String,
    pub display_name: Option<String>,
}

/// A masked chat turn's Model view, keyed by its chat interaction.
#[derive(Clone, PartialEq, Eq)]
pub struct MaskedTurn {
    pub rounds: Vec<MaskedRound>,
    /// The decoded, finished reply members saw (empty when none).
    pub reply: String,
    pub mapping: Vec<MaskedName>,
}

impl MaskedTurn {
    /// The shape every store refuses to write otherwise.
    pub fn check_shape(&self) -> Result<(), StoreError> {
        let ok = self
            .rounds
            .iter()
            .all(|round| round.request.is_array() && round.tool_calls.is_array());
        if ok {
            Ok(())
        } else {
            Err(StoreError::Constraint(
                "masked chat rounds have the wrong shape".into(),
            ))
        }
    }

    pub fn rounds_json(&self) -> Value {
        Value::Array(
            self.rounds
                .iter()
                .map(|round| {
                    serde_json::json!({
                        "round": round.round,
                        "clean": round.clean,
                        "request": round.request,
                        "reply": round.reply,
                        "tool_calls": round.tool_calls,
                    })
                })
                .collect(),
        )
    }

    pub fn mapping_json(&self) -> Value {
        Value::Array(
            self.mapping
                .iter()
                .map(|name| {
                    serde_json::json!({
                        "token": name.token,
                        "user_id": name.user_id,
                        "display_name": name.display_name,
                    })
                })
                .collect(),
        )
    }

    /// The inverse of [`Self::rounds_json`]/[`Self::mapping_json`]; `None`
    /// when a stored value has the wrong shape.
    pub fn from_json(rounds: &Value, reply: String, mapping: &Value) -> Option<Self> {
        let rounds = rounds
            .as_array()?
            .iter()
            .map(|round| {
                Some(MaskedRound {
                    round: u32::try_from(round.get("round")?.as_u64()?).ok()?,
                    clean: round.get("clean")?.as_bool()?,
                    request: round.get("request").filter(|v| v.is_array())?.clone(),
                    reply: round.get("reply")?.as_str().map(str::to_owned),
                    tool_calls: round.get("tool_calls").filter(|v| v.is_array())?.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let mapping = mapping
            .as_array()?
            .iter()
            .map(|name| {
                Some(MaskedName {
                    token: name.get("token")?.as_str()?.to_owned(),
                    user_id: name.get("user_id")?.as_str()?.to_owned(),
                    display_name: name.get("display_name")?.as_str().map(str::to_owned),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            rounds,
            reply,
            mapping,
        })
    }
}

impl fmt::Debug for MaskedRound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaskedRound")
            .field("round", &self.round)
            .field("clean", &self.clean)
            .field("reply_len", &self.reply.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for MaskedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaskedName").finish_non_exhaustive()
    }
}

impl fmt::Debug for MaskedTurn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaskedTurn")
            .field("rounds", &self.rounds.len())
            .field("reply_len", &self.reply.len())
            .field("mapping", &self.mapping.len())
            .finish()
    }
}
