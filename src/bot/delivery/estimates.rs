//! The estimate share chart on redesigned day-of and countdown posts: each
//! run's party from the roster (as the admin and portal character reads
//! resolve it) and each member's stored scouter estimate, of any age, drawn
//! once as a plain `estimate.png` attachment when the card is posted.
//! Stored only: a card never starts a lookup. While a queued, running or
//! waiting lookup (manual or background) includes a member who has no
//! stored result yet, it posts without the image (it would draw them as
//! "no estimate"); a refresh of members who all have one still draws what
//! is stored. When the oldest reading used is at least `estimate_days`
//! old, the "as of" line and the alt text add "older than n days". Edits
//! never upload, so the posted image stays as it was ("as of" its
//! readings). Anything that fails leaves the image off and the card posts
//! as it would have; it is logged, never alerted.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use twilight_model::channel::message::Component;

use super::cards::chart::{self, Chart, ChartRun};
use super::cards::{CardContext, CardKit, card_runs};
use crate::bot::commands::text::{MONTH_NAMES, local_time, weekday_name};
use crate::bot::transport::{OutgoingMessage, Upload, legacy_rows};
use crate::domain::catalog::BossTable;
use crate::domain::characters::{PartyReads, PartyRoster, PartySlot};
use crate::domain::chardata::{Carrier, PartyShares};
use crate::domain::notify::IntentContent;
use crate::domain::schedule::{Run, RunStatus, ScheduleSnapshot};
use crate::domain::settings::MessageStyle;
use crate::infrastructure::character_data::CharacterData;
use crate::infrastructure::scouter::{Combined, Estimates, Member, RosterIgn, scouter_keys};
use crate::runtime::logging;

/// How long drawing may take before the card posts without it.
pub const CHART_DEADLINE: Duration = Duration::from_secs(2);
/// The largest image attached; a bigger one is left off.
pub const MAX_CHART_BYTES: usize = 2 * 1024 * 1024;
/// Discord's attachments per message.
pub const MAX_ATTACHMENTS: usize = 10;

/// One run's party with its stored estimates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunEstimate {
    pub party: PartyShares,
    /// The party's estimated minutes per boss (a stored whole-party lookup).
    pub minutes: Vec<Option<f64>>,
    /// When the oldest reading used (members' and the party total) was
    /// fetched.
    pub oldest: Option<DateTime<Utc>>,
    /// The saved `estimate_days` while `oldest` is at least that old (the
    /// chart says "older than n days"); `None` while fresh.
    pub older_than: Option<u32>,
}

pub type EstimateFuture<'a> = Pin<Box<dyn Future<Output = Option<RunEstimate>> + Send + 'a>>;

/// Each run's stored estimates; `None` when the party can't be read or a
/// lookup is still finding a member with no stored result (the card then
/// posts without the image).
pub trait PartyEstimates: Send + Sync {
    fn run<'a>(
        &'a self,
        run: &'a Run,
        schedule: &'a ScheduleSnapshot,
        catalog: &'a BossTable,
    ) -> EstimateFuture<'a>;
}

/// The serve port: the roster store and module `character_data`'s
/// estimates memory (only built while both are on). The admin preview
/// reads the same through its roster port.
pub struct CachedEstimates<S: ?Sized> {
    store: Arc<S>,
    module: Arc<CharacterData>,
}

impl<S: ?Sized> CachedEstimates<S> {
    pub fn new(store: Arc<S>, module: Arc<CharacterData>) -> Self {
        Self { store, module }
    }
}

impl<S: PartyReads + Send + Sync + ?Sized + 'static> PartyEstimates for CachedEstimates<S> {
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
            settled_estimate(run, &slots, estimates)
        })
    }
}

/// The IGN `slot`'s member is looked up under, when they bring a
/// character the scouter can take.
fn brought_ign(slot: &PartySlot) -> Option<RosterIgn> {
    slot.brought()
        .and_then(|(character, _)| RosterIgn::lookup(character).ok())
}

/// `run`'s stored estimates, unless a queued, running or waiting lookup
/// includes a member bringing a character with no stored result: then the
/// chart is left off (logged) rather than drawn with that member missing.
/// A lookup refreshing members who all have a result draws what is stored.
fn settled_estimate(run: &Run, slots: &[PartySlot], estimates: &Estimates) -> Option<RunEstimate> {
    let pending = Cell::new(false);
    let mut estimate = read_estimates(
        run,
        slots,
        |ign| {
            let reading = estimates.cached(ign);
            if reading.is_none() && estimates.looking_up(std::slice::from_ref(ign)) {
                pending.set(true);
            }
            reading
        },
        |igns| estimates.cached_party(igns),
    );
    if pending.get() {
        skipped("lookup_running");
        return None;
    }
    estimate.older_than = estimate
        .oldest
        .filter(|oldest| estimates.stale(*oldest))
        .map(|_| estimates.fresh_days());
    Some(estimate)
}

/// `run`'s slots read against the estimates memory: a member bringing a
/// character the scouter can take is a carrier (with nothing stored, a
/// carrier without figures); anyone else is missing. `older_than` is left
/// for the caller.
pub fn read_estimates(
    run: &Run,
    slots: &[PartySlot],
    cached: impl Fn(&RosterIgn) -> Option<(Member, DateTime<Utc>)>,
    cached_party: impl Fn(&[RosterIgn]) -> Option<(BTreeMap<String, Combined>, DateTime<Utc>)>,
) -> RunEstimate {
    let mut party = PartyShares {
        bosses: run.bosses.clone(),
        ..PartyShares::default()
    };
    let mut igns: Vec<RosterIgn> = Vec::new();
    let mut oldest: Option<DateTime<Utc>> = None;
    for slot in slots {
        let Some(ign) = brought_ign(slot) else {
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
    if let Some((_, fetched_at)) = &whole {
        oldest = Some(oldest.map_or(*fetched_at, |seen| seen.min(*fetched_at)));
    }
    let minutes = run
        .bosses
        .iter()
        .map(|token| {
            let (combined, _) = whole.as_ref()?;
            scouter_keys(token)
                .find_map(|key| combined.get(key))
                .and_then(|total| total.estimated_minutes)
        })
        .collect();
    RunEstimate {
        party,
        minutes,
        oldest,
        older_than: None,
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
/// only. `None` otherwise or when a party can't be read; nobody having an
/// estimate still draws ("no estimate yet"). A stale oldest reading in any
/// run marks the whole chart "older than n days".
pub async fn chart_for(
    content: &IntentContent,
    ctx: &CardContext<'_>,
    port: &dyn PartyEstimates,
    now: DateTime<Utc>,
) -> Option<Chart> {
    if ctx.style != MessageStyle::Redesigned {
        return None;
    }
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
        chart.older_than = chart.older_than.max(estimate.older_than);
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

/// One render at a time: a render that outlives its deadline keeps its
/// permit until it really ends, so timed-out work can never pile up.
static RENDER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// `chart` drawn and encoded off the runtime, within [`CHART_DEADLINE`]
/// (waiting for the render slot included).
pub async fn draw(chart: Chart) -> Result<Upload, &'static str> {
    let description = chart::alt(&chart);
    let render = async {
        let permit = RENDER.acquire().await.map_err(|_| "busy")?;
        tokio::task::spawn_blocking(move || {
            let _held = permit;
            chart::png(&chart::svg(&chart))
        })
        .await
        .map_err(|_| "panic")?
        .map_err(|_| "render")
    };
    let bytes = match tokio::time::timeout(CHART_DEADLINE, render).await {
        Err(_) => return Err("timeout"),
        Ok(result) => result?,
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

/// Whether a post of `content` and `components` can show a file: text
/// with no components or only legacy button rows (the redesigned cards'
/// "Open in portal" link); a V2 layout or an empty render has nowhere to.
pub(super) fn shows_files(content: Option<&str>, components: &[Component]) -> bool {
    let text = content.is_some_and(|text| !text.is_empty());
    (text && components.is_empty()) || legacy_rows(content, components)
}

/// Add the chart to `message`, a new post of `content` (never an edit),
/// while `kit` has the estimates port.
pub async fn attach_chart(
    message: &mut OutgoingMessage,
    content: &IntentContent,
    ctx: &CardContext<'_>,
    kit: &CardKit,
    now: DateTime<Utc>,
) {
    if !shows_files(message.content.as_deref(), &message.components) {
        return;
    }
    let Some(port) = kit.estimates.as_deref() else {
        return;
    };
    let Some(chart) = chart_for(content, ctx, port, now).await else {
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

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::domain::characters::{Basis, Character, CharacterRole};
    use crate::domain::chardata::CharDataStore;
    use crate::domain::history::Actor;
    use crate::domain::schedule::RunSource;
    use crate::infrastructure::scouter::{JobRequest, ScouterClient, Start, Subject, Target};
    use crate::infrastructure::store::SqliteCharData;
    use crate::infrastructure::store::chardata_conformance::{member_estimate, party_estimate};

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap()
    }

    fn days_ago(days: i64) -> DateTime<Utc> {
        at() - TimeDelta::days(days)
    }

    /// The memory over nothing listening: a job is read before its worker
    /// ever runs.
    fn offline() -> Estimates {
        let url = "http://127.0.0.1:9".to_owned();
        Estimates::new(ScouterClient::new(&url).unwrap(), url, Arc::new(at))
    }

    /// The memory loaded from a file holding each `(IGN, fetched)` result
    /// and, with `party`, the total for all of them fetched then; results
    /// are fresh for `days`.
    async fn stored(
        members: &[(&str, DateTime<Utc>)],
        party: Option<DateTime<Utc>>,
        days: u32,
    ) -> Estimates {
        let file = Arc::new(SqliteCharData::open_temp().await);
        for (ign, fetched) in members {
            let row = member_estimate(&format!("ch-{ign}"), ign, *fetched);
            file.put_member_estimate(row).await.unwrap();
        }
        if let Some(fetched) = party {
            let mut ids: Vec<String> = members.iter().map(|(ign, _)| format!("ch-{ign}")).collect();
            ids.sort_unstable();
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            let mut keys: Vec<String> = members
                .iter()
                .map(|(ign, _)| ign.to_ascii_lowercase())
                .collect();
            keys.sort_unstable();
            let mut total = party_estimate(&ids, fetched);
            total.set_key = keys.join(",");
            file.put_party_estimate(total).await.unwrap();
        }
        let estimates = offline()
            .with_fresh_days(Arc::new(move || days))
            .with_store(file as Arc<dyn CharDataStore>);
        estimates.load().await;
        estimates
    }

    /// A queued lookup of `(member, IGN)` for `target`, as a refresh does:
    /// never answered from memory.
    fn refresh(estimates: &Estimates, target: Target, members: &[(&str, &str)]) {
        let started = estimates.start(JobRequest {
            target,
            subjects: members
                .iter()
                .map(|(member, ign)| Subject {
                    character_id: format!("ch-{ign}"),
                    member_id: (*member).into(),
                    ign: brought_ign(&slot(member, ign)).unwrap(),
                })
                .collect(),
            missing: Vec::new(),
            started_by: Actor::system("test"),
            refresh: true,
        });
        let Start::Started(job) = started else {
            panic!("a new job");
        };
        assert_eq!(job.outcome, None, "queued, not answered from memory");
    }

    /// Invented party member `member` bringing their main `ign`.
    fn slot(member: &str, ign: &str) -> PartySlot {
        let character = Character {
            id: format!("ch-{member}"),
            member_id: member.into(),
            ign: ign.into(),
            world: "msea".into(),
            class: None,
            level: None,
            role: CharacterRole::Main,
            retired_at: None,
            created_at: at(),
            updated_at: at(),
            updated_by: Actor::system("test"),
            version: 1,
        };
        PartySlot {
            member_id: member.into(),
            brings: Some((character.id.clone(), Basis::Main)),
            candidates: vec![character],
            retired: 0,
            pick: None,
            version: 1,
            clashes: Vec::new(),
        }
    }

    fn run() -> Run {
        Run {
            id: "r-kalos".into(),
            fixed_run_id: None,
            channel_id: None,
            week_start: at(),
            datetime: at(),
            bosses: vec!["XKalos".into()],
            participants: vec!["1001".into(), "1002".into()],
            status: RunStatus::Planned,
            source: RunSource::Fixed,
            attendance: Vec::new(),
            status_pin: None,
        }
    }

    /// The live report: a card drawn while the party's lookup had found
    /// only its first members drew the rest as "no estimate". While a
    /// lookup includes a member with no stored result there is no chart.
    #[tokio::test]
    async fn a_party_with_a_lookup_still_running_has_no_chart() {
        logging::capture();
        let estimates = offline();
        let run = run();
        let slots = [slot("1001", "MikaBish"), slot("1002", "RenShade")];
        let started = estimates.start(JobRequest {
            target: Target::Run {
                run_id: run.id.clone(),
                bosses: run.bosses.clone(),
            },
            subjects: vec![Subject {
                character_id: "ch-1002".into(),
                member_id: "1002".into(),
                // Matched as the scouter matches IGNs: whatever the case.
                ign: brought_ign(&slot("1002", "RENSHADE")).unwrap(),
            }],
            missing: Vec::new(),
            started_by: Actor::system("test"),
            refresh: false,
        });
        assert!(matches!(started, Start::Started(_)));

        assert_eq!(settled_estimate(&run, &slots, &estimates), None);
        let lines = logging::captured();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["event"], "estimate_chart_skipped");
        assert_eq!(lines[0]["reason"], "lookup_running");

        // A party nobody is looking up draws from what is stored.
        let other = settled_estimate(&run, &slots[..1], &estimates).expect("a chart");
        assert_eq!(other.party.carriers.len(), 1);
        assert_eq!(logging::captured().len(), 1, "nothing more logged");
    }

    /// A refresh (manual or background) of members who all have a stored
    /// result still draws what is stored, "as of" the oldest reading, the
    /// party total's included.
    #[tokio::test]
    async fn a_refresh_of_members_all_stored_still_draws() {
        let estimates = stored(
            &[("MikaBish", days_ago(1)), ("RenShade", days_ago(2))],
            Some(days_ago(3)),
            7,
        )
        .await;
        let run = run();
        let slots = [slot("1001", "MikaBish"), slot("1002", "RenShade")];
        let target = Target::Run {
            run_id: run.id.clone(),
            bosses: run.bosses.clone(),
        };
        refresh(
            &estimates,
            target,
            &[("1001", "MikaBish"), ("1002", "RenShade")],
        );
        assert!(estimates.looking_up(&[brought_ign(&slots[0]).unwrap()]));
        logging::capture();

        let drawn = settled_estimate(&run, &slots, &estimates).expect("a chart");
        assert_eq!(drawn.party.carriers.len(), 2);
        assert_eq!(drawn.party.missing, 0);
        assert_eq!(drawn.oldest, Some(days_ago(3)), "the party total's time");
        assert_eq!(drawn.older_than, None, "fresh");
        assert!(logging::captured().is_empty(), "nothing skipped");
    }

    /// Only a member with no stored result holds the chart back while
    /// looked up; one with none and no lookup draws "no estimate yet".
    #[tokio::test]
    async fn a_lookup_of_a_member_never_looked_up_leaves_the_chart_off() {
        let estimates = stored(&[("MikaBish", days_ago(1))], None, 7).await;
        let run = run();
        let slots = [slot("1001", "MikaBish"), slot("1002", "RenShade")];

        refresh(
            &estimates,
            Target::Characters(vec!["ch-MikaBish".into()]),
            &[("1001", "MikaBish")],
        );
        logging::capture();
        let drawn = settled_estimate(&run, &slots, &estimates).expect("a chart");
        assert_eq!(drawn.party.carriers.len(), 2, "RenShade: no estimate yet");
        assert_eq!(drawn.oldest, Some(days_ago(1)), "MikaBish's reading only");
        assert!(logging::captured().is_empty());

        refresh(
            &estimates,
            Target::Characters(vec!["ch-RenShade".into()]),
            &[("1002", "RenShade")],
        );
        logging::capture();
        assert_eq!(settled_estimate(&run, &slots, &estimates), None);
        let lines = logging::captured();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["event"], "estimate_chart_skipped");
        assert_eq!(lines[0]["reason"], "lookup_running");
    }

    /// The oldest reading at least `estimate_days` old marks the estimate
    /// with that n; younger, it carries none.
    #[tokio::test]
    async fn a_stale_oldest_reading_marks_the_estimate() {
        let run = run();
        let slots = [slot("1001", "MikaBish"), slot("1002", "RenShade")];
        let readings = [("MikaBish", days_ago(1)), ("RenShade", days_ago(10))];
        for (days, older_than) in [(7, Some(7)), (11, None), (1, Some(1))] {
            let estimates = stored(&readings, None, days).await;
            let estimate = settled_estimate(&run, &slots, &estimates).expect("a chart");
            assert_eq!(estimate.oldest, Some(days_ago(10)));
            assert_eq!(estimate.older_than, older_than, "estimate_days {days}");
        }
        // A stale party total marks it too, fresh members or not.
        let readings = [("MikaBish", days_ago(1)), ("RenShade", days_ago(2))];
        let estimates = stored(&readings, Some(days_ago(8)), 7).await;
        let estimate = settled_estimate(&run, &slots, &estimates).expect("a chart");
        assert_eq!(estimate.older_than, Some(7));
    }
}
