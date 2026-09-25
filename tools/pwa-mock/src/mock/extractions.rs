//! Extractions (v4 /extractions): the extractor's model calls, and rescan
//! jobs (v4 /rescan) that re-read party channels with visible progress.

use super::logfilter::{EXTRACTION_OUTCOMES, Facts, LogQuery};
use super::seed;
use super::{MoveError, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

struct Call {
    id: String,
    short_id: String,
    hour: i64,
    latency_ms: Option<u32>,
    channel: &'static str,
    messages: Vec<(&'static str, &'static str)>,
    amendments: Vec<(&'static str, &'static str, &'static str, f32, &'static str)>,
    error: Option<&'static str>,
    model: &'static str,
    /// proposed, no_change, failed, turned_away, content_blocked, self_service_link.
    outcome: &'static str,
}

fn calls() -> Vec<Call> {
    let mut out = vec![
        Call {
            id: "x-bm".into(),
            short_id: "e1f2a3b4".into(),
            hour: 108,
            latency_ms: Some(14_210),
            channel: "bm-trio",
            messages: vec![
                ("1012", "tue cannot, wed same time ok?"),
                ("1009", "wed ok for me"),
            ],
            amendments: vec![("move", "XBM", "Wed 23:30", 0.86, "proposed")],
            error: None,
            model: MODEL,
            outcome: "proposed",
        },
        Call {
            id: "x-kalos".into(),
            short_id: "c5d6e7f8".into(),
            hour: 40,
            latency_ms: Some(11_874),
            channel: "kalos-four",
            messages: vec![
                ("1002", "kalos 10pm instead? 9:30 too early"),
                ("1001", "ok 10"),
                ("1006", "can"),
            ],
            amendments: vec![("move", "XKalos", "Fri 22:00", 0.93, "confirmed")],
            error: None,
            model: MODEL,
            outcome: "proposed",
        },
        Call {
            id: "x-limbo".into(),
            short_id: "a9b0c1d2".into(),
            hour: 99,
            latency_ms: Some(9_302),
            channel: "limbo-trio",
            messages: vec![("1003", "nlimbo sat 9pm anyone?")],
            amendments: vec![("add", "NLimbo", "Sat 21:00?", 0.52, "proposed")],
            error: None,
            model: MODEL,
            outcome: "proposed",
        },
        Call {
            id: "x-timeout".into(),
            short_id: "f3e4d5c6".into(),
            hour: 70,
            latency_ms: None,
            channel: "fa-night",
            messages: vec![("1009", "fa same as usual")],
            amendments: vec![],
            error: Some("Gateway timed out after 60 s; the burst was retried later."),
            model: MODEL,
            outcome: "failed",
        },
    ];
    // Older quiet calls, so the list pages.
    for i in 0..30u32 {
        let outcome = match i % 10 {
            3 => "turned_away",
            5 => "content_blocked",
            7 => "self_service_link",
            _ => "no_change",
        };
        out.push(Call {
            id: format!("x-old{i}"),
            short_id: format!("{:08x}", 0x0dd0_0000 + i),
            hour: 60 - i64::from(i) * 2,
            latency_ms: (outcome != "turned_away").then_some(8_000 + i * 97),
            channel: seed::CHANNELS[(i as usize) % seed::CHANNELS.len()].0,
            messages: vec![("1004", "gg")],
            amendments: vec![],
            error: None,
            model: if i % 4 == 0 { "kanata/legacy" } else { MODEL },
            outcome,
        });
    }
    out
}

#[derive(Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub state: &'static str,
    pub window: String,
    pub channels: Vec<JobChannel>,
    pub proposals: usize,
}

#[derive(Clone, Serialize)]
pub struct JobChannel {
    pub id: &'static str,
    pub name: &'static str,
    pub state: &'static str,
    pub messages: u32,
}

#[derive(Deserialize)]
pub struct RescanRequest {
    pub channels: Vec<String>,
    pub window: String,
}

const MODEL: &str = "kanata/extract";

impl Store {
    fn hour_minute(h: i64) -> i64 {
        Self::start(false) * 1440 - 8 * 60 + h * 60
    }

    pub fn extractions(&self, query: &LogQuery) -> Result<Value, MoveError> {
        query.validate(&EXTRACTION_OUTCOMES, false)?;
        let mut all = calls();
        all.sort_by_key(|c| std::cmp::Reverse(c.hour));
        let rows: Vec<Value> = all
            .iter()
            .filter(|c| {
                let members: Vec<&str> = c.messages.iter().map(|m| m.0).collect();
                let text: Vec<&str> = c
                    .messages
                    .iter()
                    .map(|m| m.1)
                    .chain([c.short_id.as_str()])
                    .collect();
                query.matches(&Facts {
                    minute: Self::hour_minute(c.hour),
                    models: &[c.model],
                    outcome: c.outcome,
                    channel: c.channel,
                    members: &members,
                    text: &text,
                    tools: &[],
                    latency_ms: c.latency_ms.unwrap_or(0),
                })
            })
            .map(|c| {
                json!({
                    "id": c.id, "short_id": c.short_id, "at": Self::when(Self::hour_minute(c.hour)),
                    "model": c.model, "latency_ms": c.latency_ms, "messages": c.messages.len(),
                    "changes": c.amendments.len(), "channel": seed::channel(c.channel).map(|x| x.1),
                    "channel_id": c.channel, "error": c.error, "outcome": c.outcome,
                })
            })
            .collect();
        let mut models: Vec<&str> = all.iter().map(|c| c.model).collect();
        models.sort_unstable();
        models.dedup();
        Ok(json!({
            "model": MODEL,
            "rows": rows,
            "total": all.len(),
            "facets": {
                "models": models,
                "tools": [],
                "outcomes": EXTRACTION_OUTCOMES,
                "channels": seed::CHANNELS.iter().map(|c| json!({ "id": c.0, "name": c.1 })).collect::<Vec<_>>(),
            },
        }))
    }

    pub fn extraction(&self, id: &str) -> Result<Value, MoveError> {
        let c = calls()
            .into_iter()
            .find(|c| c.id == id)
            .ok_or(MoveError::NotFound)?;
        let chat: Vec<Value> = c
            .messages
            .iter()
            .enumerate()
            .map(|(i, (who, text))| json!({ "id": format!("{}-{i}", c.short_id), "author": seed::member_name(who).map_or("someone", |m| m.1), "at": Self::when(Self::hour_minute(c.hour) - 5 + i as i64), "content": text }))
            .collect();
        let prompt = format!(
            "System: You extract boss-schedule amendments from party chat. Reply with JSON only.\n\nFixed timings for {channel}: …\nThis week's runs: …\nRoster: …\n\nMessages:\n{lines}",
            channel = seed::channel(c.channel).map_or(c.channel, |x| x.1),
            lines = c
                .messages
                .iter()
                .map(|(w, t)| format!("[{}] {t}", seed::member_name(w).map_or("?", |m| m.1)))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let amendments: Vec<Value> = c.amendments.iter().map(|(kind, bosses, when, conf, status)| json!({ "kind": kind, "bosses": bosses, "when": when, "confidence": conf, "status": status })).collect();
        let raw = if c.error.is_some() {
            Value::Null
        } else {
            json!({ "amendments": amendments, "summary": "…" })
        };
        Ok(json!({
            "id": c.id, "short_id": c.short_id, "at": Self::when(Self::hour_minute(c.hour)), "model": c.model, "outcome": c.outcome,
            "latency_ms": c.latency_ms, "channel": seed::channel(c.channel).map(|x| x.1), "error": c.error,
            "prompt": prompt, "raw_response": raw.to_string(), "amendments": amendments, "messages": chat,
        }))
    }

    pub fn rescan_targets() -> Value {
        json!(
            seed::CHANNELS
                .iter()
                .filter(|c| c.2)
                .map(|c| json!({ "id": c.0, "name": c.1 }))
                .collect::<Vec<_>>()
        )
    }

    pub fn start_rescan(&mut self, req: RescanRequest) -> Result<Job, MoveError> {
        if !matches!(req.window.as_str(), "week" | "since_reset" | "two_weeks") {
            return Err(MoveError::invalid(
                "Pick a window: this boss week, since reset or two weeks.",
            ));
        }
        let channels: Vec<JobChannel> = req
            .channels
            .iter()
            .filter_map(|id| seed::channel(id).filter(|c| c.2))
            .map(|c| JobChannel {
                id: c.0,
                name: c.1,
                state: "queued",
                messages: 0,
            })
            .collect();
        if channels.is_empty() {
            if let [one] = req.channels.as_slice()
                && let Some(c) = seed::channel(one)
            {
                return Err(MoveError::Invalid(format!(
                    "{} is not watched, so there is nothing to re-read.",
                    c.1
                )));
            }
            return Err(MoveError::invalid("Choose at least one watched channel."));
        }
        if self.jobs.iter().any(|j| j.state == "running") {
            return Err(MoveError::invalid(
                "A rescan is already running; cancel it or wait.",
            ));
        }
        let (id, _) = self.fresh_id("job");
        let job = Job {
            id,
            state: "running",
            window: req.window,
            channels,
            proposals: 0,
        };
        self.jobs.push(job.clone());
        Ok(job)
    }

    /// Each poll advances the job one channel, like a cooperative drain.
    pub fn poll_rescan(&mut self, id: &str) -> Result<Job, MoveError> {
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or(MoveError::NotFound)?;
        if job.state == "running" {
            if let Some(ch) = job.channels.iter_mut().find(|c| c.state == "reading") {
                ch.state = "done";
                ch.messages = 40 + ch.name.len() as u32;
            }
            if let Some(next) = job.channels.iter_mut().find(|c| c.state == "queued") {
                next.state = "reading";
            } else if job.channels.iter().all(|c| c.state == "done") {
                job.state = "done";
            }
        }
        Ok(job.clone())
    }

    pub fn cancel_rescan(&mut self, id: &str) -> Result<Job, MoveError> {
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or(MoveError::NotFound)?;
        if job.state == "running" {
            job.state = "cancelled";
        }
        Ok(job.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::super::logfilter::LogQuery;
    use super::super::tests::store;

    #[test]
    fn extraction_filters_by_outcome_model_member_and_refuse_chat_only_ones() {
        let s = store();
        let n = |q: LogQuery| {
            s.extractions(&q).ok().unwrap()["rows"]
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!(n(LogQuery::default()), 34);
        assert_eq!(
            n(LogQuery {
                outcome: Some("proposed".into()),
                ..Default::default()
            }),
            3
        );
        assert_eq!(
            n(LogQuery {
                outcome: Some("failed,self_service_link".into()),
                ..Default::default()
            }),
            4
        );
        assert_eq!(
            n(LogQuery {
                model: Some("kanata/legacy".into()),
                ..Default::default()
            }),
            8
        );
        assert_eq!(
            n(LogQuery {
                member: Some("1012".into()),
                ..Default::default()
            }),
            1
        );
        assert!(
            s.extractions(&LogQuery {
                tool: Some("x".into()),
                ..Default::default()
            })
            .is_err()
        );
    }
}
