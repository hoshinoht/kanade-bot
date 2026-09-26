//! Re-rendering posted reminder cards after an RSVP (v4 `refresh_run_cards`
//! for reminders): each bound card with a record naming a run still ahead is
//! edited in place from the current answers. The edit keeps the posted
//! attachments, reuses the stored day-of heading and notifies nobody.
//! Cards posted before records existed (plain text) are left alone.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};

use super::cards::{self, CardContext, CardKit, DAY_OF_KIND, PostedCard, ReminderCardStore};
use crate::bot::ids::parse_id;
use crate::bot::transport::DiscordTransport;
use crate::domain::attendance::{AttendanceMode, countdown_mentions, morning_mentions};
use crate::domain::members::Directory;
use crate::domain::notify::{
    IntentContent, PingKind, countdown_minutes, everyone_on, resolve_mentions,
};
use crate::domain::schedule::{SchedulePolicy, ScheduleSnapshot};
use crate::domain::scheduler::{ScheduleStore, Scope};

pub type Now = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub struct CardRefresh<S, T> {
    pub store: Arc<S>,
    pub transport: Arc<T>,
    pub members: Arc<dyn Directory + Send + Sync>,
    pub cards: CardKit,
    pub policy: SchedulePolicy,
    /// The tick's live quiet-mode setting.
    pub quiet: Arc<AtomicBool>,
    pub now: Now,
}

impl<S, T> CardRefresh<S, T>
where
    S: ScheduleStore + ReminderCardStore + Sync,
    T: DiscordTransport,
{
    /// Edit the posted cards of `run_ids`; returns how many edits landed.
    /// Failures are skipped: a stale tally is cosmetic.
    pub async fn refresh(&self, run_ids: &[String]) -> usize {
        let now = (self.now)();
        let Ok(schedule) = self.store.load(&Scope::All).await else {
            return 0;
        };
        let mut seen = BTreeSet::new();
        let mut edited = 0;
        for run_id in run_ids {
            // v4: a run that has started keeps its cards as a record.
            let ahead = schedule
                .runs
                .iter()
                .any(|run| &run.id == run_id && run.datetime > now);
            if !ahead {
                continue;
            }
            let Ok(posted) = self.store.posted_cards(run_id).await else {
                continue;
            };
            for card in posted {
                if seen.insert(card.message_id.clone()) && self.edit(&schedule, &card).await {
                    edited += 1;
                }
            }
        }
        edited
    }

    async fn edit(&self, schedule: &ScheduleSnapshot, posted: &PostedCard) -> bool {
        let (Some(channel), Some(message)) =
            (parse_id(&posted.channel_id), parse_id(&posted.message_id))
        else {
            return false;
        };
        let content = if posted.record.kind == DAY_OF_KIND {
            IntentContent::DayOf {
                run_ids: posted.run_ids.clone(),
            }
        } else {
            let (Some(minutes), Some(run_id)) = (
                countdown_minutes(&posted.record.kind),
                posted.run_ids.first(),
            ) else {
                return false;
            };
            IntentContent::Countdown {
                run_id: run_id.clone(),
                minutes,
            }
        };
        let ctx = CardContext {
            schedule,
            attendance: self.policy.attendance,
            zone: self.policy.zone(),
            quiet: self.quiet.load(Ordering::Relaxed),
            members: &*self.members,
            catalog: self.cards.catalog.as_deref(),
        };
        let mentioned = self.mentioned(&ctx, &content);
        let Some(card) = cards::build(&content, &ctx, posted.record.heading.as_deref(), &mentioned)
        else {
            return false;
        };
        let edit = card.edit(self.cards.art.as_deref());
        self.transport
            .edit_message(channel, message, &edit)
            .await
            .is_delivered()
    }

    /// Who the card names as a mention, planned as dispatch plans it, so an
    /// edit reads like a fresh post (it notifies nobody either way).
    fn mentioned(&self, ctx: &CardContext<'_>, content: &IntentContent) -> Vec<String> {
        if ctx.quiet {
            return Vec::new();
        }
        let members: &dyn Directory = &*self.members;
        match content {
            IntentContent::DayOf { run_ids } => {
                let runs = cards::card_runs(ctx, run_ids);
                let candidates = match ctx.attendance.mode {
                    AttendanceMode::V4Compat => {
                        everyone_on(runs.iter().map(|run| run.participants.as_slice()))
                    }
                    AttendanceMode::V5 => {
                        let unknown: Vec<Vec<String>> = runs
                            .iter()
                            .map(|run| morning_mentions(&ctx.states(run)))
                            .collect();
                        everyone_on(unknown.iter().map(Vec::as_slice))
                    }
                };
                resolve_mentions(members, &candidates, &PingKind::DayOf)
            }
            IntentContent::Countdown { run_id, .. } => match ctx.run(run_id) {
                Some(run) => resolve_mentions(
                    members,
                    &countdown_mentions(&ctx.states(run)),
                    &PingKind::Countdown,
                ),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}
