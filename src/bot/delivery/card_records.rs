//! Before a reminder card is claimed: its record (kind and, for day-of, the
//! heading line; countdowns store their phrase). Digest phrases use a separate
//! pre-claim row. First writes survive retries, restarts and edits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;

use super::cards::local_day;
use super::cards::{
    CardContext, CardKit, CardRecord, DAY_OF_KIND, DigestPhraseStore, PhraseKind,
    ReminderCardStore, card_runs,
};
use crate::domain::notify::{DedupeKey, DeliveryTarget, IntentContent, NotificationIntent};
use crate::runtime::logging;

static PREPARATION_LOCKS: OnceLock<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>> = OnceLock::new();

fn preparation_lock(key: &str) -> Arc<AsyncMutex<()>> {
    let locks = PREPARATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap_or_else(PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(key.to_owned(), Arc::downgrade(&lock));
    lock
}

/// The record kind for `content`; `None` for sends that are not reminders.
fn record_kind(content: &IntentContent) -> Option<String> {
    match content {
        IntentContent::DayOf { .. } => Some(DAY_OF_KIND.to_owned()),
        IntentContent::Countdown { minutes, .. } => Some(format!("countdown_{minutes}")),
        _ => None,
    }
}

/// The stored record for `intent`, else a new one prepared before claim.
/// Countdown failures fail closed; day-of preserves its legacy fresh-heading
/// send on a record-store error.
pub async fn prepare<S: ReminderCardStore>(
    store: &S,
    kit: &CardKit,
    ctx: &CardContext<'_>,
    intent: &NotificationIntent,
    now: DateTime<Utc>,
) -> Option<CardRecord> {
    let kind = record_kind(&intent.content)?;
    let key = DedupeKey::native(&intent.targets).ok()?;
    let lock = preparation_lock(&format!("reminder:{}", key.as_str()));
    let _guard = lock.lock().await;
    match store.card_record(key.as_str()).await {
        Ok(Some(record)) => return Some(record),
        Ok(None) => {}
        Err(_) => {
            logging::event("WARN", "card_record_failed", json!({"operation": "read"}));
            if !matches!(&intent.content, IntentContent::DayOf { .. }) {
                return None;
            }
        }
    }
    let record = fresh_record(&kind, kit, ctx, intent).await?;
    match store.save_card_record(key.as_str(), &record, now).await {
        Ok(stored) => Some(stored),
        Err(_) => {
            logging::event("WARN", "card_record_failed", json!({"operation": "write"}));
            (record.kind == DAY_OF_KIND).then_some(record)
        }
    }
}

async fn fresh_record(
    kind: &str,
    kit: &CardKit,
    ctx: &CardContext<'_>,
    intent: &NotificationIntent,
) -> Option<CardRecord> {
    let heading = match &intent.content {
        IntentContent::DayOf { run_ids } => {
            let first = card_runs(ctx, run_ids).first()?.datetime;
            Some(kit.heading.choose(&local_day(first, ctx.zone)).await.0)
        }
        IntentContent::Countdown { .. } => Some(
            kit.heading
                .choose_phrase(PhraseKind::Countdown, kit.catalog.as_deref())
                .await
                .0,
        ),
        _ => return None,
    };
    Some(CardRecord {
        kind: kind.to_owned(),
        heading,
    })
}

/// The pre-claim phrase row's identity is the native digest dedupe key.
pub(crate) fn digest_phrase_key(week_start: DateTime<Utc>) -> Option<String> {
    let target = [DeliveryTarget::Digest(week_start)];
    DedupeKey::native(&target)
        .ok()
        .map(|key| key.as_str().to_owned())
}

/// Prepare a digest's phrase before any replacement delete or journal claim.
pub async fn prepare_digest<S: DigestPhraseStore>(
    store: &S,
    kit: &CardKit,
    intent: &NotificationIntent,
    replacing_legacy_digest: bool,
    now: DateTime<Utc>,
) -> Option<String> {
    let IntentContent::Digest { week_start, .. } = intent.content else {
        return None;
    };
    let [DeliveryTarget::Digest(target_week)] = intent.targets.as_slice() else {
        return None;
    };
    if target_week != &week_start {
        return None;
    }
    let key = digest_phrase_key(week_start)?;
    let lock = preparation_lock(&format!("digest:{key}"));
    let _guard = lock.lock().await;
    match store.digest_phrase(&key).await {
        Ok(Some(phrase)) => return Some(phrase),
        Ok(None) => {}
        Err(_) => {
            logging::event("WARN", "digest_phrase_failed", json!({"operation": "read"}));
            return None;
        }
    }
    // A legacy bound digest may still need refresh if deletion is refused.
    let phrase = if replacing_legacy_digest {
        PhraseKind::Digest.seed().to_owned()
    } else {
        kit.heading
            .choose_phrase(PhraseKind::Digest, kit.catalog.as_deref())
            .await
            .0
    };
    match store.save_digest_phrase(&key, &phrase, now).await {
        Ok(stored) => Some(stored),
        Err(_) => {
            logging::event(
                "WARN",
                "digest_phrase_failed",
                json!({"operation": "write"}),
            );
            None
        }
    }
}
