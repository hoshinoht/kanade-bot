//! The estimate share chart on redesigned day-of and countdown posts: each
//! run's party from the roster (as the admin and portal character reads
//! resolve it) and each member's cached scouter estimate, drawn once as a
//! plain `estimate.png` attachment when the card is posted. Cached only: a
//! card never starts a lookup. Edits never upload, so the posted image stays
//! as it was ("as of" its readings). Anything that fails leaves the image
//! off and the card posts as it would have; it is logged, never alerted.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;

use super::cards::chart::{self, Chart, ChartRun};
use super::cards::{CardContext, CardKit, card_runs};
use crate::bot::commands::text::{MONTH_NAMES, local_time, weekday_name};
use crate::bot::transport::{OutgoingMessage, Upload};
use crate::domain::catalog::BossTable;
use crate::domain::characters::{CharacterStore, PartyRoster, PartySlot};
use crate::domain::chardata::{Carrier, PartyShares};
use crate::domain::notify::IntentContent;
use crate::domain::schedule::{Run, RunStatus, ScheduleSnapshot};
use crate::domain::settings::MessageStyle;
use crate::infrastructure::character_data::CharacterData;
use crate::infrastructure::scouter::{Combined, Member, RosterIgn, scouter_keys};
use crate::runtime::logging;

/// How long drawing may take before the card posts without it.
pub const CHART_DEADLINE: Duration = Duration::from_secs(2);
/// The largest image attached; a bigger one is left off.
pub const MAX_CHART_BYTES: usize = 2 * 1024 * 1024;
/// Discord's attachments per message.
pub const MAX_ATTACHMENTS: usize = 10;

/// One run's party with its cached estimates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunEstimate {
    pub party: PartyShares,
    /// The party's estimated minutes per boss (a cached whole-party lookup).
    pub minutes: Vec<Option<f64>>,
    /// When the oldest reading used was fetched.
    pub oldest: Option<DateTime<Utc>>,
}

pub type EstimateFuture<'a> = Pin<Box<dyn Future<Output = Option<RunEstimate>> + Send + 'a>>;

/// Each run's cached estimates; `None` when the party can't be read (the
/// card then posts without the image).
pub trait PartyEstimates: Send + Sync {
    fn run<'a>(
        &'a self,
        run: &'a Run,
        schedule: &'a ScheduleSnapshot,
        catalog: &'a BossTable,
    ) -> EstimateFuture<'a>;
}

/// The serve port: the roster store and module `character_data`'s
/// estimates memory (only built while both are on).
pub struct CachedEstimates<S> {
    store: Arc<S>,
    module: Arc<CharacterData>,
}

impl<S> CachedEstimates<S> {
    pub fn new(store: Arc<S>, module: Arc<CharacterData>) -> Self {
        Self { store, module }
    }
}

impl<S: CharacterStore + Send + Sync + 'static> PartyEstimates for CachedEstimates<S> {
    fn run<'a>(
        &'a self,
        run: &'a Run,
        schedule: &'a ScheduleSnapshot,
        catalog: &'a BossTable,
    ) -> EstimateFuture<'a> {
        Box::pin(async move {
            let estimates = self.module.estimates()?;
            let week: Vec<Run> = schedule
                .runs
                .iter()
                .filter(|other| other.week_start == run.week_start)
                .cloned()
                .collect();
            let roster = match PartyRoster::load(self.store.as_ref(), run, &week).await {
                Ok(roster) => roster,
                Err(_) => {
                    logging::log!(warn, "estimate_chart_skipped", reason = "roster_unreadable");
                    return None;
                }
            };
            let slots = roster.slots(run, &week, &schedule.fixed_runs, catalog);
            Some(read_estimates(
                run,
                &slots,
                |ign| estimates.cached(ign),
                |igns| estimates.cached_party(igns),
            ))
        })
    }
}

/// `run`'s slots read against the estimates memory: a member bringing a
/// character the scouter can take is a carrier (with nothing cached, a
/// carrier without figures); anyone else is missing.
pub fn read_estimates(
    run: &Run,
    slots: &[PartySlot],
    cached: impl Fn(&RosterIgn) -> Option<(Member, DateTime<Utc>)>,
    cached_party: impl Fn(&[RosterIgn]) -> Option<BTreeMap<String, Combined>>,
) -> RunEstimate {
    let mut party = PartyShares {
        bosses: run.bosses.clone(),
        ..PartyShares::default()
    };
    let mut igns: Vec<RosterIgn> = Vec::new();
    let mut oldest: Option<DateTime<Utc>> = None;
    for slot in slots {
        let Some(ign) = slot
            .brought()
            .and_then(|(character, _)| RosterIgn::lookup(character).ok())
        else {
            party.missing += 1;
            continue;
        };
        let reading = cached(&ign);
        if let Some((_, fetched_at)) = &reading {
            oldest = Some(oldest.map_or(*fetched_at, |seen| seen.min(*fetched_at)));
        }
        party.carriers.push(Carrier {
            ign: ign.as_str().to_owned(),
            crit_assumed: reading
                .as_ref()
                .is_some_and(|(member, _)| member.crit_assumed()),
            effective: run
                .bosses
                .iter()
                .map(|token| {
                    reading
                        .as_ref()
                        .and_then(|(member, _)| member.effective(token))
                })
                .collect(),
        });
        igns.push(ign);
    }
    let whole = (party.missing == 0 && !igns.is_empty())
        .then(|| cached_party(&igns))
        .flatten();
    let minutes = run
        .bosses
        .iter()
        .map(|token| {
            let combined = whole.as_ref()?;
            scouter_keys(token)
                .find_map(|key| combined.get(key))
                .and_then(|total| total.estimated_minutes)
        })
        .collect();
    RunEstimate {
        party,
        minutes,
        oldest,
    }
}

/// `Tue 29 Sep 08:41` in the guild's zone.
fn stamp(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!(
        "{} {} {} {}",
        weekday_name(local.weekday()),
        local.day(),
        MONTH_NAMES[local.month0() as usize],
        local_time(at, zone)
    )
}

fn skipped(reason: &str) {
    logging::log!(warn, "estimate_chart_skipped", reason = reason);
}

/// The chart for a card's runs: redesigned day-of and countdown cards
/// only, while the port is set. `None` without one or when a party can't
/// be read; nobody having an estimate still draws ("no estimate yet").
pub async fn chart_for(
    content: &IntentContent,
    ctx: &CardContext<'_>,
    kit: &CardKit,
    now: DateTime<Utc>,
) -> Option<Chart> {
    if ctx.style != MessageStyle::Redesigned {
        return None;
    }
    let port = kit.estimates.as_ref()?;
    let catalog = ctx.catalog?;
    let runs: Vec<&Run> = match content {
        IntentContent::DayOf { run_ids } => card_runs(ctx, run_ids),
        IntentContent::Countdown { run_id, .. } => ctx.run(run_id).into_iter().collect(),
        _ => return None,
    };
    if runs.is_empty() {
        return None;
    }
    let mut chart = Chart {
        portal: ctx.v2.is_some_and(|kit| kit.portal_url().is_some()),
        ..Chart::default()
    };
    let mut oldest: Option<DateTime<Utc>> = None;
    for run in runs {
        let estimate = port.run(run, ctx.schedule, catalog).await?;
        oldest = match (oldest, estimate.oldest) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        chart.runs.push(ChartRun {
            time: if run.status == RunStatus::Otot {
                "own time".to_owned()
            } else {
                local_time(run.datetime, ctx.zone)
            },
            party: estimate.party,
            minutes: estimate.minutes,
        });
    }
    chart.as_of = stamp(oldest.unwrap_or(now), ctx.zone);
    Some(chart)
}

/// `chart` drawn and encoded off the runtime, within [`CHART_DEADLINE`].
pub async fn draw(chart: Chart) -> Result<Upload, &'static str> {
    let description = chart::alt(&chart);
    let job = tokio::task::spawn_blocking(move || chart::png(&chart::svg(&chart)));
    let bytes = match tokio::time::timeout(CHART_DEADLINE, job).await {
        Err(_) => return Err("timeout"),
        Ok(Err(_)) => return Err("panic"),
        Ok(Ok(Err(_))) => return Err("render"),
        Ok(Ok(Ok(bytes))) => bytes,
    };
    if bytes.len() > MAX_CHART_BYTES {
        return Err("too_large");
    }
    Ok(Upload {
        filename: chart::FILENAME.to_owned(),
        bytes: Arc::from(bytes),
        description: Some(description),
    })
}

/// Add the chart to `message`, a new post of `content` (never an edit).
pub async fn attach_chart(
    message: &mut OutgoingMessage,
    content: &IntentContent,
    ctx: &CardContext<'_>,
    kit: &CardKit,
    now: DateTime<Utc>,
) {
    // A V2 layout or an empty render has nowhere to show a file.
    if !message.components.is_empty() || message.content.as_deref().is_none_or(str::is_empty) {
        return;
    }
    let Some(chart) = chart_for(content, ctx, kit, now).await else {
        return;
    };
    if message.attachments.len() >= MAX_ATTACHMENTS {
        skipped("attachment_limit");
        return;
    }
    match draw(chart).await {
        Ok(upload) => message.attachments.push(upload),
        Err(reason) => skipped(reason),
    }
}
