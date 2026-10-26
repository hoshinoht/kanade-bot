//! Posts the gateway made, recorded as its delivery journal records them
//! (claim, then bind the receipt): every reminder card due by the seed's
//! now, with the card record the tick saves before its claim, proposal
//! cards and this week's digest (mock `config::last_digest`).

use chrono::{DateTime, Datelike, Duration, Utc};
use kanade::{
    bot::delivery::{
        cards::{CardRecord, DAY_OF_KIND, PhraseKind, ReminderCardStore, seed_heading},
        preview::{record_key, reminder_intent},
    },
    domain::{
        notify::{
            Claim, DeliveryJournal, DeliverySettings, DeliveryTarget, DigestInclusion, EffectKind,
            IntentContent, NotificationIntent, Receipt,
        },
        scheduler::{ScheduleStore, Scope},
    },
};

use super::{Seed, ids::BOSS_SCHEDULE};
use crate::Fail;

/// The channel the digest is posted in.
const DIGEST_CHANNEL: &str = BOSS_SCHEDULE;

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

/// v4 `local_day` (`Thu 03 Sep`): the day a day-of heading names.
fn local_day(at: DateTime<Utc>, zone: chrono_tz::Tz) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let local = at.with_timezone(&zone);
    let month = MONTHS[local.month0() as usize];
    format!("{} {:02} {month}", local.weekday(), local.day())
}

/// The record the tick saves before claiming `intent` (`card_records`):
/// its kind and seed line, as a send with no pre-generated line stores it.
fn card_record(
    seed: &Seed<'_>,
    intent: &NotificationIntent,
    runs: &[kanade::domain::schedule::Run],
) -> Option<CardRecord> {
    match &intent.content {
        IntentContent::DayOf { run_ids } => {
            let first = runs
                .iter()
                .filter(|run| run_ids.contains(&run.id))
                .map(|run| run.datetime)
                .min()?;
            Some(CardRecord {
                kind: DAY_OF_KIND.into(),
                heading: Some(seed_heading(&local_day(first, seed.state.policy.zone()))),
                slots: None,
            })
        }
        IntentContent::Countdown { minutes, .. } => Some(CardRecord {
            kind: format!("countdown_{minutes}"),
            heading: Some(PhraseKind::Countdown.seed().into()),
            slots: None,
        }),
        _ => None,
    }
}

/// Every reminder due by the seed's now and still unsent, posted at its
/// fire time as the tick plans it (rows firing together share a card):
/// record saved, then claimed and bound, which marks the rows sent.
async fn reminders(seed: &Seed<'_>) -> Result<(), Fail> {
    let settings = match &seed.state.config {
        Some(desk) => desk.settings().await,
        None => Default::default(),
    };
    let roster = seed.directory().await?;
    let mut posts = 0;
    loop {
        let schedule = seed.store.load(&Scope::All).await?;
        let mut due: Vec<_> = schedule
            .reminders
            .iter()
            .filter(|row| row.sent_at.is_none() && row.fire_at <= seed.now)
            .collect();
        due.sort_by(|a, b| (a.fire_at, &a.id).cmp(&(b.fire_at, &b.id)));
        let Some(next) = due.first() else {
            return Ok(());
        };
        let intent = reminder_intent(
            &schedule,
            &next.id,
            &roster,
            DeliverySettings {
                post_channel_id: settings.posting.channel_id.as_deref(),
                quiet_mode: settings.notifications.quiet_mode,
            },
        )
        .ok_or_else(|| format!("the tick would post no card for reminder {}", next.id))?;
        let at = next.fire_at;
        let key = record_key(&intent).ok_or("a reminder intent without a dedupe key")?;
        let record = card_record(seed, &intent, &schedule.runs)
            .ok_or("a reminder intent without a card record")?;
        seed.store.save_card_record(&key, &record, at).await?;
        posts += 1;
        deliver(seed, intent, Seed::snowflake(at, posts), None, at).await?;
    }
}

/// The due reminder cards, then this week's digest, posted 15 minutes into
/// the boss week.
pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    reminders(seed).await?;
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
