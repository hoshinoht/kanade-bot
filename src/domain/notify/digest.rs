//! The weekly digest: when it posts (v4 `BossBot._post_week_digest`), where,
//! and which runs it includes (v4 `digest_card`). Rendering lives elsewhere.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;

use super::dispatch::DeliverySettings;
use super::intent::{
    ChannelChoice, ChannelDirectory, DeliveryTarget, EffectKind, IntentContent, JournalView,
    NotificationIntent, PlannedSend, canonical_allow_list, choose_channel,
};
use super::policy::allowed_mentions;
use crate::domain::schedule::{Run, RunStatus};
use crate::domain::time::{DateOutOfRange, local_naive, to_iso};
use crate::domain::weeks;

/// The guild's boss-week reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeekReset {
    pub zone: Tz,
    pub weekday: Weekday,
    pub time: NaiveTime,
}

impl WeekReset {
    /// The start of the boss week containing `now`, in UTC.
    ///
    /// # Errors
    /// [`DateOutOfRange`] outside v4's years.
    pub fn current_week(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>, DateOutOfRange> {
        let start = weeks::week_start(&now, self.zone, self.weekday, self.time)?;
        Ok(start.to_fixed().with_timezone(&Utc))
    }
}

/// A boss week's tracked digest card, kept as a log once retired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeeklyDigest {
    pub week_start: DateTime<Utc>,
    pub channel_id: String,
    pub message_id: String,
    pub posted_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
}

/// Active digests of weeks before `week_start`: they stop following live state.
pub fn retire_digests_before(
    digests: &[WeeklyDigest],
    week_start: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    digests
        .iter()
        .filter(|row| row.retired_at.is_none() && row.week_start < week_start)
        .map(|row| row.week_start)
        .collect()
}

/// Why a tick stamps the week without posting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordReason {
    /// No post channel: setting one later must not back-post a week in progress.
    NoPostChannel,
    /// A store that has never seen a reset has nothing of its own to report.
    FirstTick,
}

/// What one tick does about the digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestAction {
    /// This week's digest was already posted (or deliberately skipped).
    UpToDate,
    /// Stamp `current_week` as done without posting.
    Record(RecordReason),
    /// Post the current week's digest; stamp it only once it binds.
    Post,
}

/// One tick's digest decision. `retire_before` always applies first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DigestTick {
    pub current_week: DateTime<Utc>,
    pub retire_before: DateTime<Utc>,
    pub action: DigestAction,
}

/// Decide the tick. `last_digest_week` is the stored stamp, compared as text
/// with the current week's UTC ISO form as v4 does, so a slept-through reset
/// posts exactly once and a restart before the first tick still posts.
///
/// # Errors
/// [`DateOutOfRange`] outside v4's years.
pub fn plan_digest_tick(
    reset: &WeekReset,
    now: DateTime<Utc>,
    last_digest_week: Option<&str>,
    post_channel_id: Option<&str>,
) -> Result<DigestTick, DateOutOfRange> {
    let current_week = reset.current_week(now)?;
    let action = if last_digest_week == Some(to_iso(&current_week)?.as_str()) {
        DigestAction::UpToDate
    } else if post_channel_id.is_none() {
        DigestAction::Record(RecordReason::NoPostChannel)
    } else if last_digest_week.is_none() {
        DigestAction::Record(RecordReason::FirstTick)
    } else {
        DigestAction::Post
    };
    Ok(DigestTick {
        current_week,
        retire_before: current_week,
        action,
    })
}

/// One local day of the digest: run ids in time order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestDay {
    pub date: NaiveDate,
    pub run_ids: Vec<String>,
}

/// Which runs a week's digest shows, and its summary counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DigestInclusion {
    pub days: Vec<DigestDay>,
    /// `done` runs.
    pub cleared: usize,
    /// Every run shown (all but cancelled).
    pub live: usize,
    /// `planned` or `at_risk`.
    pub unsettled: usize,
    /// `at_risk`, also counted as unsettled.
    pub at_risk: usize,
}

/// The week's runs bar cancelled ones, grouped by guild-local date.
///
/// # Errors
/// [`DateOutOfRange`] outside v4's years.
pub fn digest_inclusion(
    runs: &[Run],
    week_start: DateTime<Utc>,
    zone: Tz,
) -> Result<DigestInclusion, DateOutOfRange> {
    let mut live: Vec<&Run> = runs
        .iter()
        .filter(|run| run.week_start == week_start && run.status != RunStatus::Cancelled)
        .collect();
    live.sort_by(|a, b| (a.datetime, &a.id).cmp(&(b.datetime, &b.id)));
    let mut inclusion = DigestInclusion {
        live: live.len(),
        ..DigestInclusion::default()
    };
    for run in live {
        match run.status {
            RunStatus::Done => inclusion.cleared += 1,
            RunStatus::Planned => inclusion.unsettled += 1,
            RunStatus::AtRisk => {
                inclusion.unsettled += 1;
                inclusion.at_risk += 1;
            }
            _ => {}
        }
        let date = local_naive(&run.datetime, zone)?.date();
        match inclusion.days.iter_mut().find(|day| day.date == date) {
            Some(day) => day.run_ids.push(run.id.clone()),
            None => inclusion.days.push(DigestDay {
                date,
                run_ids: vec![run.id.clone()],
            }),
        }
    }
    Ok(inclusion)
}

/// Everything one digest post reads.
#[derive(Clone, Copy)]
pub struct DigestPostInput<'a> {
    pub week_start: DateTime<Utc>,
    pub current_week: DateTime<Utc>,
    pub zone: Tz,
    pub runs: &'a [Run],
    pub digests: &'a [WeeklyDigest],
    /// A channel named by the caller; no fallback applies to it.
    pub explicit_channel: Option<&'a str>,
    pub settings: DeliverySettings<'a>,
    pub channels: &'a dyn ChannelDirectory,
    pub journal: &'a dyn JournalView,
}

/// A planned digest post (v4 `BossBot._post_digest`).
///
/// Executors must check `send.disposition` before touching `replaces`: a
/// [`SendDisposition::Suppressed`](super::intent::SendDisposition::Suppressed)
/// send means an earlier attempt may have delivered, so the replaced card is
/// never deleted and nothing is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestSend {
    pub send: PlannedSend,
    /// An active card for the week that must be confirmed deleted and retired
    /// before sending; an ambiguous deletion cancels the send.
    pub replaces: Option<WeeklyDigest>,
    /// Stamp this week as posted once the send binds.
    pub record_week: Option<DateTime<Utc>>,
}

/// Plan one digest post; `None` when there is nowhere to post (nothing is
/// stamped). Retire [`retire_digests_before`] `current_week` first.
///
/// # Errors
/// [`DateOutOfRange`] outside v4's years.
pub fn plan_digest_post(input: &DigestPostInput<'_>) -> Result<Option<DigestSend>, DateOutOfRange> {
    let requested = input.explicit_channel.or(input.settings.post_channel_id);
    let ChannelChoice::Requested(channel_id) = choose_channel(requested, None, input.channels)
    else {
        return Ok(None);
    };
    let replaces = input
        .digests
        .iter()
        .find(|row| row.week_start == input.week_start && row.retired_at.is_none())
        .cloned();
    // The digest names people rather than notifying the whole guild.
    let mentions =
        canonical_allow_list(allowed_mentions(&[], Some(&[]), input.settings.quiet_mode).users());
    let intent = NotificationIntent {
        effect: EffectKind::Digest,
        effect_context: Vec::new(),
        channel_id,
        targets: vec![DeliveryTarget::Digest(input.week_start)],
        mentions,
        content: IntentContent::Digest {
            week_start: input.week_start,
            inclusion: digest_inclusion(input.runs, input.week_start, input.zone)?,
        },
        warnings: Vec::new(),
    };
    Ok(Some(DigestSend {
        send: PlannedSend::new(intent, input.journal),
        replaces,
        record_week: (input.week_start == input.current_week).then_some(input.current_week),
    }))
}
