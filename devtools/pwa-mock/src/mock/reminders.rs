//! Reminders (v4 /reminders): every card the bot will post or has posted,
//! and one card's preview as the server's `bot::delivery::preview` shapes it
//! (synthetic text in the bot's v4 card wording; the server renders the real
//! card with the delivery tick's builder).

use serde_json::{Value, json};

use super::clock::{clock, iso_now, iso_z};
use super::dto::*;
use super::{Store, clock::minutes};

const REACT_HINT: &str = "React \u{2705} if you're on, \u{274c} if not.";
const INK_BLUE: &str = "#4d5c9e";

/// The redesigned style's boss label: `Hard Radiant Malefic Star`.
fn labels(run: &Run) -> String {
    run.bosses
        .iter()
        .map(|boss| {
            let word = match boss.difficulty {
                "e" => "Easy",
                "n" => "Normal",
                "h" => "Hard",
                "c" => "Chaos",
                "x" => "Extreme",
                other => other,
            };
            format!("{word} {}", boss.name)
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// One redesigned day-of embed: In / Waiting / Out side by side, the run id
/// and levels below; the entry art on the day's first run only.
fn redesigned_embed(run: &Run, first: bool) -> Value {
    let names = |answer: &str| {
        let names: Vec<&str> = run
            .participants
            .iter()
            .filter(|p| p.answer == answer)
            .map(|p| p.name)
            .collect();
        if names.is_empty() {
            "—".to_owned()
        } else {
            names.join(", ")
        }
    };
    let waiting = run
        .participants
        .iter()
        .filter(|p| p.answer == "waiting")
        .count();
    let state = match run.status {
        "at_risk" => "❗ at risk".to_owned(),
        "done" => "🏁 done".to_owned(),
        _ if waiting > 0 => format!("⚠️ waiting on {waiting}"),
        _ => "✅ all in".to_owned(),
    };
    let when = match &run.time {
        Some(time) => format!("🕘 **{time}**"),
        None => "🕒 own time".to_owned(),
    };
    let levels: Vec<String> = run
        .bosses
        .iter()
        .filter_map(|boss| boss.level.map(|level| format!("Lv{level}")))
        .collect();
    let footer = if levels.is_empty() {
        format!("#{}", run.short_id)
    } else {
        format!("#{} · {}", run.short_id, levels.join(" + "))
    };
    let lead = run.bosses.first();
    json!({
        "color": colour(run, INK_BLUE),
        "title": labels(run),
        "description": format!("{when} · {state}"),
        "fields": [
            { "name": "In ✅", "value": names("yes"), "inline": true },
            { "name": "Waiting", "value": names("waiting"), "inline": true },
            { "name": "Out ❌", "value": names("no"), "inline": true },
        ],
        "footer": footer,
        "thumbnail": lead.and_then(|boss| boss.portrait.clone()),
        "image": if first { lead.and_then(|boss| boss.art.clone()) } else { None },
    })
}

fn bosses_text(run: &Run) -> String {
    run.bosses
        .iter()
        .map(|boss| boss.token.as_str())
        .collect::<Vec<_>>()
        .join(" + ")
}

fn detail(run: &Run) -> Vec<String> {
    let mut lines: Vec<String> = run
        .bosses
        .iter()
        .map(|boss| format!("**{}** · {}", boss.token, boss.name))
        .collect();
    let yes = run
        .participants
        .iter()
        .filter(|p| p.answer == "yes")
        .count();
    let status = match run.status {
        "confirmed" => "✅ confirmed",
        "otot" => "🕒 own time",
        "cancelled" => "🚫 cancelled",
        "done" => "🏁 done",
        _ => "⚠️ unconfirmed",
    };
    lines.push(format!("{status} · {yes}/{} in", run.participants.len()));
    let waiting: Vec<String> = run
        .participants
        .iter()
        .filter(|p| p.answer == "waiting")
        .map(|p| format!("<@{}>", p.id))
        .collect();
    if !waiting.is_empty() {
        lines.push(format!("Still to answer: {}", waiting.join(", ")));
    }
    lines
}

fn colour(run: &Run, fallback: &str) -> String {
    match run.bosses.first().map(|boss| boss.hue) {
        Some(hue) if hue > 0 => {
            // A lead-boss tint stands in for the catalog guide colour.
            let (r, g, b) = hsl(f64::from(hue), 0.62, 0.55);
            format!("#{r:02x}{g:02x}{b:02x}")
        }
        _ => fallback.to_owned(),
    }
}

fn hsl(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f64| ((v + m) * 255.0).round() as u8;
    (byte(r), byte(g), byte(b))
}

impl Store {
    /// Every card as `(local minute, run start minute, run, row)`.
    fn reminder_rows(&self) -> Vec<(i64, i64, Run, ReminderRow)> {
        let now = Self::now_minute();
        let mut rows = Vec::new();
        for rec in &self.runs {
            let run = self.dto(rec);
            let day = (Self::start(rec.next_week) + i64::from(rec.day)) * 1440;
            // An own-time run counts as starting at the end of its day.
            let start = day + run.time.as_deref().map_or(1439, minutes);
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
                    fire_at: iso_z(at),
                    bosses: run.bosses.clone(),
                    party: run.participants.iter().map(|p| p.name).collect(),
                    url: card.url.clone(),
                };
                rows.push((at, start, run.clone(), row));
            }
        }
        rows
    }

    pub fn reminders(&self) -> Reminders {
        let mut upcoming = Vec::new();
        let mut sent = Vec::new();
        for (at, _, _, row) in self.reminder_rows() {
            if matches!(row.state, "queued" | "due") {
                upcoming.push((at, row));
            } else {
                sent.push((at, row));
            }
        }
        upcoming.sort_by_key(|(at, _)| *at);
        sent.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        Reminders {
            upcoming: upcoming.into_iter().map(|(_, r)| r).collect(),
            sent: sent.into_iter().map(|(_, r)| r).collect(),
            generated_at: iso_now(),
        }
    }

    /// `GET /api/admin/reminders/{id}/preview`; `None` for an unknown id.
    pub fn reminder_preview(&self, id: &str) -> Option<Value> {
        let (at, start, run, row) = self
            .reminder_rows()
            .into_iter()
            .find(|(_, _, _, row)| row.id == id)?;
        let lead = run.bosses.first();
        let mentions: Vec<String> = run
            .participants
            .iter()
            .filter(|p| p.answer != "no")
            .map(|p| format!("<@{}>", p.id))
            .collect();
        let sent = row.state == "sent";
        // As the server: a row retired without posting has no card.
        let card = if run.status == "cancelled" || row.state == "stale" {
            Value::Null
        } else if row.kind == "morning" && self.config.message_style == "redesigned" {
            // Every run whose morning card fires at the same minute shares it.
            let mut day: Vec<(i64, Run)> = self
                .reminder_rows()
                .into_iter()
                .filter(|(other, _, run, row)| {
                    *other == at && row.kind == "morning" && run.status != "cancelled"
                })
                .map(|(_, start, run, _)| (start, run))
                .collect();
            day.sort_by_key(|(start, _)| *start);
            let mut embeds = day
                .iter()
                .enumerate()
                .map(|(index, (_, run))| redesigned_embed(run, index == 0));
            let mut card = embeds
                .next()
                .unwrap_or_else(|| redesigned_embed(&run, true));
            let waiting: Vec<String> = day
                .iter()
                .flat_map(|(_, run)| &run.participants)
                .filter(|p| p.answer == "waiting")
                .map(|p| format!("<@{}>", p.id))
                .collect();
            let line = if waiting.is_empty() {
                "react ✅ in / ❌ out".to_owned()
            } else {
                format!("Waiting on {} · react ✅ in / ❌ out", waiting.join(" "))
            };
            let day = Self::when(at);
            let day = day.rsplit_once(' ').map_or(day.as_str(), |(day, _)| day);
            card["content"] = json!(format!("📅 **Today — {day}**\n-# {line}"));
            card["more_embeds"] = json!(embeds.collect::<Vec<_>>());
            card["heading_final"] = json!(sent);
            card
        } else if row.kind == "morning" {
            let day = Self::when(at);
            let day = day.rsplit_once(' ').map_or(day.as_str(), |(day, _)| day);
            let time = run
                .time
                .clone()
                .unwrap_or_else(|| clock(at.rem_euclid(1440)));
            json!({
                "content": format!("📅 **Today — {day}**\n{}", mentions.join(", ")),
                "color": colour(&run, "#5865f2"),
                "title": null,
                "description": null,
                "fields": [{
                    "name": format!("🕘 {time}  ·  {}", bosses_text(&run)),
                    "value": detail(&run).join("\n"),
                    "inline": false,
                }],
                "footer": REACT_HINT,
                "thumbnail": lead.and_then(|boss| boss.portrait.clone()),
                "image": lead.and_then(|boss| boss.art.clone()),
                "more_embeds": [],
                "heading_final": sent,
            })
        } else {
            let offset = if row.kind == "T-1h" { "1h" } else { "15m" };
            let time = run.time.clone().unwrap_or_default();
            let waiting = run.participants.iter().any(|p| p.answer == "waiting");
            json!({
                "content": format!(
                    "⏰ **{}** in {offset} ({time}) — {}",
                    bosses_text(&run),
                    mentions.join(", ")
                ),
                "color": colour(&run, if waiting { "#fee75c" } else { "#57f287" }),
                "title": null,
                "description": detail(&run).join("\n"),
                "fields": [],
                "footer": if waiting { Some(REACT_HINT) } else { None },
                "thumbnail": lead.and_then(|boss| boss.portrait.clone()),
                "image": null,
                "more_embeds": [],
                "heading_final": sent,
            })
        };
        let run_started = start <= Self::now_minute();
        Some(
            json!({ "reminder": row, "card": card, "run_started": run_started, "generated_at": iso_now() }),
        )
    }
}
