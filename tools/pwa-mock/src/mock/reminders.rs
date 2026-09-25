//! Reminders (v4 /reminders): every card the bot will post or has posted.

use super::dto::*;
use super::{Store, clock::minutes};

impl Store {
    pub fn reminders(&self) -> Reminders {
        let now = Self::now_minute();
        let mut upcoming = Vec::new();
        let mut sent = Vec::new();
        for rec in &self.runs {
            let run = self.dto(rec);
            let day = (Self::start(rec.next_week) + i64::from(rec.day)) * 1440;
            for card in &run.cards {
                let at = day + minutes(&card.at);
                // Queued but already past its time: due now, the next tick posts it.
                let state = match card.state {
                    "posted" => "sent",
                    "skipped" => "stale",
                    _ if at <= now => "due",
                    _ => "queued",
                };
                let row = ReminderRow {
                    id: format!("{}-{}", run.id, card.label),
                    run_id: run.id.clone(),
                    run_short_id: run.short_id.clone(),
                    kind: card.label,
                    state,
                    at: Self::when(at),
                    bosses: run.bosses.clone(),
                    party: run.participants.iter().map(|p| p.name).collect(),
                    url: card.url.clone(),
                };
                if matches!(state, "queued" | "due") {
                    upcoming.push((at, row));
                } else {
                    sent.push((at, row));
                }
            }
        }
        upcoming.sort_by_key(|(at, _)| *at);
        sent.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        Reminders {
            upcoming: upcoming.into_iter().map(|(_, r)| r).collect(),
            sent: sent.into_iter().map(|(_, r)| r).collect(),
        }
    }
}
