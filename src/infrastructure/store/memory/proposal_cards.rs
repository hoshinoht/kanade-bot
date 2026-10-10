//! In-memory `ProposalCardStore`, mirroring `sqlite/proposal_cards.rs`.

use chrono::{DateTime, Utc};

use super::{MemoryScheduleStore, micros};
use crate::domain::proposals::{CardDetails, ProposalCardStore, StoredCard};
use crate::domain::scheduler::StoreError;

impl ProposalCardStore for MemoryScheduleStore {
    async fn save_card(
        &self,
        proposal_id: &str,
        channel_id: &str,
        details: &CardDetails,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let result = async {
            let mut tables = self.tables();
            if let Some((_, card)) = tables.drafts.cards.get(proposal_id) {
                // The same round trip SQLite stores, so equality matches.
                let stored = CardDetails::from_json(&card.details.to_json());
                let given = CardDetails::from_json(&details.to_json());
                return if card.channel_id == channel_id && stored == given {
                    Ok(())
                } else {
                    Err(StoreError::Constraint(format!(
                        "proposal {proposal_id} already has another card"
                    )))
                };
            }
            if !tables.drafts.proposals.contains_key(proposal_id) {
                return Err(StoreError::Constraint(format!(
                    "proposal {proposal_id} does not exist"
                )));
            }
            let details = CardDetails::from_json(&details.to_json())
                .ok_or_else(|| StoreError::Constraint("card details do not round-trip".into()))?;
            let card = StoredCard {
                proposal_id: proposal_id.to_owned(),
                channel_id: channel_id.to_owned(),
                details,
                message_id: None,
                posted_at: None,
            };
            let order = micros(at).timestamp_micros();
            tables.drafts.cards.insert(
                proposal_id.to_owned(),
                (u64::try_from(order).unwrap_or_default(), card),
            );
            Ok(())
        }
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Inbox, result)
    }

    async fn load_cards(&self, proposal_ids: &[String]) -> Result<Vec<StoredCard>, StoreError> {
        let tables = self.tables();
        Ok(proposal_ids
            .iter()
            .filter_map(|id| tables.drafts.cards.get(id).map(|(_, card)| card.clone()))
            .collect())
    }

    async fn cards_on_message(&self, message_id: &str) -> Result<Vec<StoredCard>, StoreError> {
        Ok(self
            .tables()
            .drafts
            .cards
            .values()
            .filter(|(_, card)| card.message_id.as_deref() == Some(message_id))
            .map(|(_, card)| card.clone())
            .collect())
    }

    async fn unposted_cards(&self, channel_id: &str) -> Result<Vec<StoredCard>, StoreError> {
        let tables = self.tables();
        let mut found: Vec<&(u64, StoredCard)> = tables
            .drafts
            .cards
            .values()
            .filter(|(_, card)| {
                card.channel_id == channel_id
                    && card.message_id.is_none()
                    && tables
                        .drafts
                        .drafts
                        .get(&card.proposal_id)
                        .is_some_and(|draft| draft.status.is_live())
            })
            .collect();
        found.sort_by(|a, b| (a.0, &a.1.proposal_id).cmp(&(b.0, &b.1.proposal_id)));
        Ok(found.into_iter().map(|(_, card)| card.clone()).collect())
    }
}
