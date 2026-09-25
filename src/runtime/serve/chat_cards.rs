//! Chat proposal cards on Discord: posted through the card desk (journalled,
//! approved by ✅ like extraction cards) and the pending-card inbox the
//! `get_pending` tool reads.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{NaiveTime, Weekday};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use crate::{
    bot::{
        cards::{CardDesk, when_text},
        delivery::LogAlerts,
    },
    chat::{
        answer::ChatPorts,
        tools::{
            ProposalCard,
            propose::{FIX_EDIT, FIX_REMOVE, kind_label},
            read::PendingCard,
        },
    },
    domain::{
        drafts::ProposalStore,
        ids::{RandomIds, short_id},
        proposals::{ChangeKind, Payload, ProposalCardStore, ProposedChange},
    },
    extract::{
        AmendmentKind,
        pipeline::{Card, CardEntry, PostResult},
    },
    infrastructure::store::SqliteStore,
};

use super::discord::GatewayTransport;

pub type ChatDesk<T> = CardDesk<SqliteStore, T, RandomIds, LogAlerts>;

/// One question's card ports.
pub struct ChatCards<'a, T> {
    pub desk: &'a ChatDesk<T>,
    pub store: &'a SqliteStore,
    pub zone: Tz,
    /// The question was deleted: its cards are not posted.
    pub cancelled: &'a AtomicBool,
}

fn weekday(payload: &Map<String, Value>) -> Option<Weekday> {
    let index = payload.get("weekday")?.as_u64()?;
    Weekday::try_from(u8::try_from(index).ok()?).ok()
}

fn time(payload: &Map<String, Value>) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(payload.get("time")?.as_str()?, "%H:%M").ok()
}

fn text(payload: &Map<String, Value>, key: &str) -> Option<String> {
    payload.get(key)?.as_str().map(str::to_owned)
}

/// The staged change as the chat tool built it, from the card's v4 payload.
fn change(card: &ProposalCard) -> ProposedChange {
    let payload = &card.payload;
    let op = payload.get("op").and_then(Value::as_str);
    let payload = match (card.kind, op) {
        (ChangeKind::Fix, Some(FIX_REMOVE)) => Payload::FixRemove {
            fixed_run_id: text(payload, "fixed_run_id"),
        },
        (ChangeKind::Fix, Some(FIX_EDIT)) => Payload::FixEdit {
            fixed_run_id: text(payload, "fixed_run_id"),
            weekday: weekday(payload),
            time: time(payload),
            participants: payload
                .get("participants")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        },
        (ChangeKind::Fix, _) => Payload::Fix {
            weekday: weekday(payload),
            time: time(payload),
        },
        _ => Payload::None,
    };
    ProposedChange {
        kind: card.kind,
        run_id: card.run_id.clone(),
        channel_id: Some(card.channel_id.clone()),
        bosses: card.bosses.clone(),
        participants: card.participants.clone(),
        new_datetime: card.new_datetime,
        rsvp: card.rsvp,
        payload,
    }
}

fn entry(card: &ProposalCard) -> Option<CardEntry> {
    Some(CardEntry {
        proposal_id: card.proposal_id.clone(),
        change: change(card),
        kind: AmendmentKind::parse(card.kind.as_str())?,
        run_id: card.run_id.clone(),
        summary: card.summary.clone(),
        is_question: false,
        needs_answer: false,
        confidence: 1.0,
        also_mentioned: Vec::new(),
        day_ref: None,
        time_ref: None,
        evidence_message_ids: card.evidence_message_ids.clone(),
        self_service: None,
    })
}

impl<T: GatewayTransport> ChatPorts for ChatCards<'_, T> {
    /// Live proposals with a card, guild-wide (v4 `service.pending`).
    async fn pending(&self) -> Vec<PendingCard> {
        let Ok(live) = self.store.list_proposals(true).await else {
            return Vec::new();
        };
        let ids: Vec<String> = live.into_iter().map(|row| row.draft.id).collect();
        let Ok(cards) = self.store.load_cards(&ids).await else {
            return Vec::new();
        };
        cards
            .into_iter()
            .map(|card| {
                let details = &card.details;
                let mut payload = Map::new();
                if let Some(op) = &details.payload.op {
                    payload.insert("op".into(), json!(op));
                }
                PendingCard {
                    short_id: short_id(&card.proposal_id),
                    kind_label: kind_label(details.kind, &payload).to_owned(),
                    bosses: details.bosses.clone(),
                    when: when_text(details, self.zone),
                }
            })
            .collect()
    }

    async fn post_card(&self, card: &ProposalCard) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err("the question was deleted".into());
        }
        let entry = entry(card).ok_or("unknown change kind")?;
        let posted = self
            .desk
            .post_card(&Card {
                channel_id: card.channel_id.clone(),
                entries: vec![entry],
                superseded: card.superseded.clone(),
            })
            .await;
        match posted {
            PostResult::Posted => Ok(()),
            // Saved for a later repost, or not sent: nobody can see it now.
            PostResult::Pending | PostResult::NotPosted => Err("the card was not posted".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn card(kind: ChangeKind, payload: Value) -> ProposalCard {
        ProposalCard {
            proposal_id: "p1".into(),
            kind,
            kind_label: "x",
            run_id: None,
            channel_id: "50".into(),
            bosses: vec!["lotus".into()],
            participants: vec!["11".into()],
            new_datetime: None,
            rsvp: None,
            payload: payload.as_object().cloned().unwrap_or_default(),
            summary: "s".into(),
            week_start: Utc.with_ymd_and_hms(2026, 9, 3, 0, 0, 0).unwrap(),
            when: String::new(),
            party: String::new(),
            evidence_message_ids: Vec::new(),
            superseded: Vec::new(),
        }
    }

    #[test]
    fn weekly_card_payloads_become_the_staged_change() {
        let nine = NaiveTime::from_hms_opt(21, 0, 0);
        let new = change(&card(
            ChangeKind::Fix,
            json!({"weekday": 5, "time": "21:00"}),
        ));
        assert_eq!(
            new.payload,
            Payload::Fix {
                weekday: Some(Weekday::Sat),
                time: nine
            }
        );
        let edit = change(&card(
            ChangeKind::Fix,
            json!({"op": FIX_EDIT, "fixed_run_id": "f1", "participants": ["12"]}),
        ));
        assert_eq!(
            edit.payload,
            Payload::FixEdit {
                fixed_run_id: Some("f1".into()),
                weekday: None,
                time: None,
                participants: vec!["12".into()],
            }
        );
        let remove = change(&card(
            ChangeKind::Fix,
            json!({"op": FIX_REMOVE, "fixed_run_id": "f1"}),
        ));
        assert_eq!(
            remove.payload,
            Payload::FixRemove {
                fixed_run_id: Some("f1".into())
            }
        );
        let moved = change(&card(ChangeKind::Move, json!({})));
        assert_eq!(moved.payload, Payload::None);
        assert_eq!(moved.channel_id.as_deref(), Some("50"));
        assert!(entry(&card(ChangeKind::Rsvp, json!({}))).is_some());
    }
}
