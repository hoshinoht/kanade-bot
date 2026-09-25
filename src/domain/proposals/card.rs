//! What a proposal's Discord card shows (v4's `amendments` row as the card
//! formatter reads it), stored with the proposal so a card can be re-rendered
//! after a restart and shown by the admin portal, and the card-store port.

use std::future::Future;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::change::ChangeKind;
use crate::domain::scheduler::StoreError;
use crate::domain::time::{from_iso, to_iso};

/// v4 `payload` keys the card reads: a timing's slot (`weekday` Monday = 0,
/// `time` `HH:MM`), a timing edit/removal (`op`, `weekly_when`, new
/// `participants`), and a stand-in's `remove`/`add`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardPayload {
    pub weekday: Option<u8>,
    pub time: Option<String>,
    pub op: Option<String>,
    pub weekly_when: Option<String>,
    pub participants: Vec<String>,
    pub remove: Vec<String>,
    pub add: Vec<String>,
}

/// One change as its card renders it.
#[derive(Clone, Debug, PartialEq)]
pub struct CardDetails {
    pub kind: ChangeKind,
    pub run_id: Option<String>,
    pub bosses: Vec<String>,
    pub participants: Vec<String>,
    pub new_datetime: Option<DateTime<Utc>>,
    pub day_ref: Option<String>,
    pub time_ref: Option<String>,
    /// As written (v4 renders unknown answers as "answered").
    pub rsvp: Option<String>,
    pub is_question: bool,
    pub summary: Option<String>,
    /// Kind names the burst also said about this run.
    pub also_mentioned: Vec<String>,
    pub confidence: f64,
    pub payload: CardPayload,
    pub evidence_message_ids: Vec<String>,
    /// The rendered self-service line (lead-in, action and link), if any.
    pub self_service: Option<String>,
}

fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Some(_) => None,
    }
}

fn text(value: Option<&Value>) -> Option<Option<String>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    }
}

impl CardPayload {
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        if let Some(weekday) = self.weekday {
            out.insert("weekday".into(), json!(weekday));
        }
        for (key, value) in [
            ("time", &self.time),
            ("op", &self.op),
            ("weekly_when", &self.weekly_when),
        ] {
            if let Some(value) = value {
                out.insert(key.into(), json!(value));
            }
        }
        for (key, value) in [
            ("participants", &self.participants),
            ("remove", &self.remove),
            ("add", &self.add),
        ] {
            if !value.is_empty() {
                out.insert(key.into(), json!(value));
            }
        }
        Value::Object(out)
    }

    /// v4 payload JSON; `None` when a key has the wrong type.
    pub fn from_json(value: &Value) -> Option<Self> {
        let object = match value {
            Value::Null => return Some(Self::default()),
            Value::Object(object) => object,
            _ => return None,
        };
        let weekday = match object.get("weekday") {
            None | Some(Value::Null) => None,
            Some(value) => Some(u8::try_from(value.as_u64()?).ok().filter(|day| *day < 7)?),
        };
        Some(Self {
            weekday,
            time: text(object.get("time"))?,
            op: text(object.get("op"))?,
            weekly_when: text(object.get("weekly_when"))?,
            participants: strings(object.get("participants"))?,
            remove: strings(object.get("remove"))?,
            add: strings(object.get("add"))?,
        })
    }
}

impl CardDetails {
    /// v4 row keys (as the card vectors spell them) plus `evidence_message_ids`
    /// and `self_service`.
    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "run_id": self.run_id,
            "bosses": self.bosses,
            "participants": self.participants,
            "new_datetime": self.new_datetime.and_then(|at| to_iso(&at).ok()),
            "day_ref": self.day_ref,
            "time_ref": self.time_ref,
            "rsvp": self.rsvp,
            "is_question": self.is_question,
            "summary": self.summary,
            "also_mentioned": self.also_mentioned,
            "confidence": self.confidence,
            "payload": self.payload.to_json(),
            "evidence_message_ids": self.evidence_message_ids,
            "self_service": self.self_service,
        })
    }

    /// `None` for anything malformed; absent optional keys read as empty.
    pub fn from_json(value: &Value) -> Option<Self> {
        let new_datetime = match text(value.get("new_datetime"))? {
            None => None,
            Some(at) => Some(from_iso(&at).ok()?),
        };
        Some(Self {
            kind: ChangeKind::parse(value.get("kind")?.as_str()?)?,
            run_id: text(value.get("run_id"))?,
            bosses: strings(value.get("bosses"))?,
            participants: strings(value.get("participants"))?,
            new_datetime,
            day_ref: text(value.get("day_ref"))?,
            time_ref: text(value.get("time_ref"))?,
            rsvp: text(value.get("rsvp"))?,
            is_question: match value.get("is_question") {
                None | Some(Value::Null) => false,
                Some(flag) => flag.as_bool()?,
            },
            summary: text(value.get("summary"))?,
            also_mentioned: strings(value.get("also_mentioned"))?,
            confidence: match value.get("confidence") {
                None | Some(Value::Null) => 0.0,
                Some(number) => number.as_f64()?,
            },
            payload: CardPayload::from_json(value.get("payload").unwrap_or(&Value::Null))?,
            evidence_message_ids: strings(value.get("evidence_message_ids"))?,
            self_service: text(value.get("self_service"))?,
        })
    }
}

/// A proposal's stored card: where it goes and, once posted, where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredCard {
    pub proposal_id: String,
    pub channel_id: String,
    pub details: CardDetails,
    /// Set by the delivery journal's bind, in the same transaction.
    pub message_id: Option<String>,
    pub posted_at: Option<DateTime<Utc>>,
}

/// Card details beside proposals. The message binding is written only by the
/// delivery journal (`DeliveryTarget::Card` bind).
pub trait ProposalCardStore {
    /// Record a live proposal's card once; the same details again are a no-op,
    /// different ones (or a missing proposal) are [`StoreError::Constraint`].
    fn save_card(
        &self,
        proposal_id: &str,
        channel_id: &str,
        details: &CardDetails,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The cards of these proposals, in the given order; unknown ids skipped.
    fn load_cards(
        &self,
        proposal_ids: &[String],
    ) -> impl Future<Output = Result<Vec<StoredCard>, StoreError>> + Send;

    /// Every proposal posted on `message_id`, by proposal id.
    fn cards_on_message(
        &self,
        message_id: &str,
    ) -> impl Future<Output = Result<Vec<StoredCard>, StoreError>> + Send;

    /// Live proposals' cards in `channel_id` never posted (v4 stranded rows),
    /// oldest first.
    fn unposted_cards(
        &self,
        channel_id: &str,
    ) -> impl Future<Output = Result<Vec<StoredCard>, StoreError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_round_trip_through_json() {
        let details = CardDetails {
            kind: ChangeKind::Fix,
            run_id: None,
            bosses: vec!["NKalos".into()],
            participants: vec!["1".into()],
            new_datetime: Some(DateTime::from_timestamp(1_788_000_000, 0).expect("instant")),
            day_ref: Some("thu".into()),
            time_ref: None,
            rsvp: None,
            is_question: true,
            summary: Some("weekly".into()),
            also_mentioned: vec!["otot".into()],
            confidence: 0.75,
            payload: CardPayload {
                weekday: Some(4),
                time: Some("20:00".into()),
                op: Some("edit".into()),
                weekly_when: Some("Thu 21:00".into()),
                participants: vec!["1".into(), "3".into()],
                ..CardPayload::default()
            },
            evidence_message_ids: vec!["101".into()],
            self_service: Some("→ edit the run: https://x".into()),
        };
        assert_eq!(
            CardDetails::from_json(&details.to_json()),
            Some(details.clone())
        );
        assert_eq!(CardDetails::from_json(&json!({"kind": "nope"})), None);
        assert_eq!(
            CardPayload::from_json(&json!({"weekday": 9})),
            None,
            "weekday is Monday = 0 .. Sunday = 6"
        );
    }
}
