//! Posts the gateway made, recorded as its delivery journal records them
//! (claim, then bind the receipt): proposal cards and this week's digest
//! (mock `config::last_digest`).

use chrono::{DateTime, Duration, Utc};
use kanade::domain::notify::{
    Claim, DeliveryJournal, DeliveryTarget, DigestInclusion, EffectKind, IntentContent,
    NotificationIntent, Receipt,
};

use super::Seed;
use crate::Fail;

/// The channel the digest is posted in.
const DIGEST_CHANNEL: &str = "boss-schedule";

/// Claim `intent` under a lease of its own and bind it as posted at `at`
/// as `message_id` in its channel; `record_week` raises the digest marker.
pub async fn deliver(
    seed: &Seed<'_>,
    intent: NotificationIntent,
    message_id: String,
    record_week: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
) -> Result<(), Fail> {
    let journal = &*seed.store;
    let fail = |error| -> Fail { format!("delivering a seeded post: {error:?}").into() };
    let lease = journal
        .begin_lease("e2e-seed", "seed", at)
        .await
        .map_err(fail)?;
    let Claim::Fresh(attempt) = journal
        .claim(&lease, &intent, None, at)
        .await
        .map_err(fail)?
    else {
        return Err("a seeded post was already claimed".into());
    };
    let receipt = Receipt {
        channel_id: intent.channel_id.clone(),
        message_id,
    };
    journal
        .bind(&lease, &attempt, &receipt, record_week, at)
        .await
        .map_err(fail)?;
    journal.end_lease(&lease, at).await.map_err(fail)?;
    Ok(())
}

/// This week's digest, posted 15 minutes into the boss week.
pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    let posted = seed.week + Duration::minutes(15);
    deliver(
        seed,
        NotificationIntent {
            effect: EffectKind::Digest,
            effect_context: Vec::new(),
            channel_id: DIGEST_CHANNEL.into(),
            targets: vec![DeliveryTarget::Digest(seed.week)],
            mentions: Vec::new(),
            content: IntentContent::Digest {
                week_start: seed.week,
                inclusion: DigestInclusion::default(),
            },
            warnings: Vec::new(),
        },
        Seed::snowflake(posted, 0x4000),
        Some(seed.week),
        posted,
    )
    .await
}
