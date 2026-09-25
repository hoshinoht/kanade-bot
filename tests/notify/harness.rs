//! Test stand-ins for what the notify core leaves to later slices: the roster,
//! the delivery journal and the Discord transport.

use std::collections::BTreeSet;

use kanade::domain::{
    ids::short_id,
    members::{Member, PingLevel, Roster},
    notify::{DeliveryTarget, DeliveryWarning, IntentContent, PlannedSend, SendDisposition},
};
use serde_json::{Value, json};

use crate::common::{strings, text};

/// v4's first stub message id.
const FIRST_MESSAGE_ID: u64 = 700_000_000_000_000_001;

pub fn roster(input: &Value) -> Roster {
    let mut roster = Roster::new();
    for member in input["members"].as_array().expect("members") {
        roster.upsert(Member {
            user_id: text(&member["user_id"]).to_owned(),
            display_name: member["display_name"].as_str().map(str::to_owned),
            nickname: member["nickname"].as_str().map(str::to_owned),
            has_role: member["has_role"].as_bool().unwrap_or_default(),
            is_bot: false,
            ping_level: PingLevel::parse_stored(text(&member["ping_level"])).expect("level"),
        });
    }
    roster
}

pub fn channels(input: &Value) -> BTreeSet<String> {
    strings(&input["available_channel_ids"])
        .into_iter()
        .collect()
}

/// What executing one planned send did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Bound(String),
    Uncertain,
    Suppressed,
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Self::Bound(_) => "bound",
            Self::Uncertain => "raised:DeliveryUncertainError",
            Self::Suppressed => "suppressed",
        }
    }
}

/// A journal that keeps ambiguous attempts claimed forever, over a transport
/// that either confirms with a counter id or times out (v4 `set_transport`).
pub struct Journal {
    pub held: BTreeSet<DeliveryTarget>,
    pub fail_sends: bool,
    next_message_id: u64,
    pub effects: Vec<Value>,
}

impl Journal {
    pub fn new() -> Self {
        Self {
            held: BTreeSet::new(),
            fail_sends: false,
            next_message_id: FIRST_MESSAGE_ID,
            effects: Vec::new(),
        }
    }

    /// Execute one send and record it as the v4 replayer records a `SendPlan`.
    pub fn execute(&mut self, send: &PlannedSend) -> Outcome {
        let outcome = match send.disposition {
            SendDisposition::Suppressed => Outcome::Suppressed,
            SendDisposition::Send if self.fail_sends => {
                self.held.extend(send.intent.targets.iter().cloned());
                Outcome::Uncertain
            }
            SendDisposition::Send => {
                let id = self.next_message_id;
                self.next_message_id += 1;
                Outcome::Bound(id.to_string())
            }
        };
        self.effects.push(effect_json(send, outcome.label()));
        outcome
    }
}

fn effect_json(send: &PlannedSend, outcome: &str) -> Value {
    let intent = &send.intent;
    let mut value = json!({
        "effect_kind": intent.effect.as_str(),
        "channel_id": intent.channel_id,
        "dedupe_scope": "native",
        "targets": intent.targets.iter().map(|target| json!({
            "binding_type": target.binding_type(),
            "key_primary": target.key_primary().expect("in range"),
            "key_secondary": null,
        })).collect::<Vec<_>>(),
        "mentions": intent.mentions,
        "role_mentions": [],
        "mention_everyone": false,
        "outcome": outcome,
    });
    if let IntentContent::Digest { inclusion, .. } = &intent.content {
        value["digest"] = json!({
            "days": inclusion.days.iter().map(|day| json!({
                "runs": day.run_ids.iter().map(|id| short_id(id)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "cleared": inclusion.cleared,
            "live": inclusion.live,
            "unsettled": inclusion.unsettled,
            "at_risk": inclusion.at_risk,
        });
    }
    if !intent.warnings.is_empty() {
        value["warnings"] = intent.warnings.iter().map(warning_json).collect();
    }
    value
}

pub fn warning_json(warning: &DeliveryWarning) -> Value {
    match warning {
        DeliveryWarning::HomeChannelUnavailable {
            home_channel_id,
            run_ids,
        } => json!({
            "kind": "home_channel_unavailable",
            "home_channel_id": home_channel_id,
            "run_ids": run_ids,
        }),
    }
}
