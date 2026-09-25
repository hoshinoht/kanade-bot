//! Which runs a posted card is for: the card→run rows written when the card
//! was bound (they survive reminder rebuilds), plus reminders stamped with the
//! message id without a journal binding (v4 `reminders_by_message`).
//!
//! [`CardIndex`] passes only the message id; Discord message ids are unique
//! snowflakes, so the lookup is not channel-qualified.

use twilight_model::id::Id;
use twilight_model::id::marker::MessageMarker;

use super::super::SqliteStore;
use crate::bot::events::{CardIndex, LookupError};

impl CardIndex for SqliteStore {
    async fn runs_for_message(
        &self,
        message: Id<MessageMarker>,
    ) -> Result<Vec<String>, LookupError> {
        sqlx::query_scalar(
            "SELECT run_id FROM delivery_card_runs WHERE message_id = ?1
             UNION
             SELECT run_id FROM reminders WHERE message_id = ?1
             ORDER BY 1",
        )
        .bind(message.get().to_string())
        .fetch_all(&self.readers)
        .await
        .map_err(|error| LookupError(error.to_string()))
    }
}
