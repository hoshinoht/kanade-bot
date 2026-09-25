//! Self-service redirect for one kept change (N1): plan the link under the
//! effective mode, and claim the author's once-per-boss-week lead-in only for
//! a link that will actually be posted.

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};

use super::call::Kept;
use super::extractor::Extractor;
use super::ports::{Outbox, Proposer, SelfServiceTip};
use crate::chat::nudge::{NudgeFacts, mood_for};
use crate::domain::model_log::ModelLogStore;
use crate::domain::proposals::{Payload, ProposedChange};
use crate::domain::scheduler::ScheduleStore;
use crate::extract::redirect::{RedirectFacts, RedirectLink, SelfServiceMode, plan};
use crate::infrastructure::llm::LlmProvider;

/// A link that will be posted, and whether its card stays.
#[derive(Clone, Debug)]
pub(super) struct LinkPlan {
    pub link: RedirectLink,
    pub keep_card: bool,
    pub author_id: String,
}

fn day_code(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

impl<S, P, X, O> Extractor<S, P, X, O>
where
    S: ScheduleStore + ModelLogStore + Send + Sync,
    P: LlmProvider,
    X: Proposer,
    O: Outbox,
{
    /// `None` when no link goes out: cards-only (portal closed or no deps),
    /// several authors behind the change, or a case that keeps today's card.
    pub(super) fn link_plan(
        &self,
        kept: &Kept,
        change: &ProposedChange,
        authors: &[String],
        changes_in_message: usize,
        now: DateTime<Utc>,
    ) -> Option<LinkPlan> {
        let deps = self.self_service.as_ref()?;
        let mode = self.config.self_service.effective_mode();
        if mode == SelfServiceMode::CardsOnly {
            return None;
        }
        // The tip and the classification are about one member.
        let [author_id] = authors else {
            return None;
        };
        let run = kept.run.as_ref();
        let facts = RedirectFacts {
            author_id,
            change,
            run,
            confidence: kept.amendment.confidence,
            is_question: kept.amendment.is_question,
            min_confidence: self.config.min_confidence,
            changes_in_message,
            reset: self.config.week_reset(),
            now,
        };
        let redirect = plan(&facts, mode, deps.links.as_ref());
        Some(LinkPlan {
            link: redirect.link?,
            keep_card: redirect.keep_card,
            author_id: author_id.clone(),
        })
    }

    /// Claims the author's weekly tip (only now that the link is certain) and
    /// composes its lead-in; returns the log label with the tip.
    pub(super) async fn tip(
        &self,
        channel_id: &str,
        change: &ProposedChange,
        link_plan: LinkPlan,
        now: DateTime<Utc>,
        errors: &mut Vec<String>,
    ) -> SelfServiceTip {
        let mut tip = SelfServiceTip {
            link: link_plan.link,
            lead_in: None,
            line: None,
            claimed: None,
        };
        let Some(deps) = self.self_service.as_ref() else {
            return tip;
        };
        let Some(persona) = deps.personas.persona_for(&link_plan.author_id) else {
            return tip;
        };
        let (boss, day, time) = self.slot_text(change);
        let facts = NudgeFacts {
            channel_id,
            purpose: tip.link.purpose,
            // The extractor flags neither failure nor frustration yet.
            mood: mood_for(false, false),
            boss: &boss,
            day: &day,
            time: &time,
        };
        let reset = self.config.week_reset();
        match deps
            .nudger
            .tip(
                self.store.as_ref(),
                &link_plan.author_id,
                &reset,
                now,
                &persona,
                &facts,
            )
            .await
        {
            Ok(Some(nudge)) => {
                tip.lead_in = Some(nudge.lead_in);
                tip.line = Some(nudge.line);
                // `Nudger::tip` claimed this week's tip for the member.
                tip.claimed = reset
                    .current_week(now)
                    .ok()
                    .map(|week| (link_plan.author_id.clone(), week));
            }
            Ok(None) => {}
            Err(error) => errors.push(format!("self-service tip: {error}")),
        }
        tip
    }

    /// `{boss}`, `{day}`, `{time}` in the guild's zone; blank when unknown
    /// (the nudge then falls back to a line without that field).
    fn slot_text(&self, change: &ProposedChange) -> (String, String, String) {
        let boss = change.bosses.join(", ");
        let (day, time) = match (change.new_datetime, &change.payload) {
            (Some(at), _) => {
                let local = at.with_timezone(&self.config.zone);
                (
                    day_code(local.weekday()).to_owned(),
                    format!("{:02}:{:02}", local.hour(), local.minute()),
                )
            }
            (
                None,
                Payload::FixEdit {
                    weekday: Some(weekday),
                    time: Some(time),
                    ..
                },
            ) => (
                day_code(*weekday).to_owned(),
                format!("{:02}:{:02}", time.hour(), time.minute()),
            ),
            _ => (String::new(), String::new()),
        };
        (boss, day, time)
    }
}
