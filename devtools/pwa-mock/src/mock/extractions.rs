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
    /// proposed, no_change, failed, turned_away, content_blocked,
    /// self_service_link, identity_leak.
    outcome: &'static str,
    /// Reported (prompt, completion) tokens; `None` when not reported.
    usage: Option<(u32, u32)>,
    /// Local prompt estimate; `None` when nothing was sent.
    estimate: Option<u32>,
}

/// Reported usage over logged requests `(prompt, completion, estimate)`, as
/// the server sums it: totals over those with a pair (`null` when none),
/// how many, and the median reported/estimate ratio to two decimals.
pub(super) fn usage_summary(
    items: impl IntoIterator<Item = (Option<u32>, Option<u32>, Option<u32>)>,
) -> serde_json::Map<String, Value> {
    let (mut prompt, mut completion, mut reported) = (None::<u64>, None::<u64>, 0usize);
    let mut ratios: Vec<f64> = Vec::new();
    for (p, c, e) in items {
        let (Some(p), Some(c)) = (p, c) else { continue };
        prompt = Some(prompt.unwrap_or(0) + u64::from(p));
        completion = Some(completion.unwrap_or(0) + u64::from(c));
        reported += 1;
        if let Some(e) = e.filter(|e| *e > 0) {
            ratios.push(f64::from(p) / f64::from(e));
        }
    }
    ratios.sort_by(f64::total_cmp);
    let mid = ratios.len() / 2;
    let ratio = match ratios.len() {
        0 => None,
        n if n % 2 == 1 => Some(ratios[mid]),
        _ => Some(f64::midpoint(ratios[mid - 1], ratios[mid])),
    }
    .map(|r| (r * 100.0).round() / 100.0);
    let mut out = serde_json::Map::new();
    out.insert("prompt_tokens".into(), json!(prompt));
    out.insert("completion_tokens".into(), json!(completion));
    out.insert("reported".into(), json!(reported));
    out.insert("est_ratio".into(), json!(ratio));
    out
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
            usage: Some((1_820, 64)),
            estimate: Some(1_700),
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
            usage: Some((2_010, 72)),
            estimate: Some(1_880),
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
            // The gateway reported no usage: the estimate alone.
            usage: None,
            estimate: Some(1_500),
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
            usage: None,
            estimate: Some(1_600),
        },
    ];
    // Older quiet calls, so the list pages.
    for i in 0..30u32 {
        let outcome = match i % 10 {
            3 => "turned_away",
            5 => "content_blocked",
            7 => "self_service_link",
            // The boundary scanner refused it: nothing was sent.
            9 => "identity_leak",
            _ => "no_change",
        };
        out.push(Call {
            id: format!("x-old{i}"),
            short_id: format!("{:08x}", 0x0dd0_0000 + i),
            hour: 60 - i64::from(i) * 2,
            latency_ms: (!matches!(outcome, "turned_away" | "identity_leak"))
                .then_some(8_000 + i * 97),
            channel: seed::CHANNELS[(i as usize) % seed::CHANNELS.len()].0,
            messages: vec![("1004", "gg")],
            amendments: vec![],
            error: None,
            model: if i % 4 == 0 { "kanata/legacy" } else { MODEL },
            outcome,
            // Nothing sent when turned away; every third older call reported.
            usage: (!matches!(outcome, "turned_away" | "identity_leak") && i % 3 == 0)
                .then_some((1_400 + i * 10, 30)),
            estimate: (!matches!(outcome, "turned_away" | "identity_leak"))
                .then_some(1_300 + i * 10),
        });
    }
    out
}

#[derive(Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub state: &'static str,
    pub window: String,
    pub started_at: Option<String>,
    pub channels: Vec<JobChannel>,
    /// Gated messages read so far (the channels' sum).
    pub messages: u32,
    /// `messages` plus what each unread channel is expected to find; a
    /// finished job's total is what it read.
    pub messages_total: Option<u32>,
    pub proposals: usize,
}

impl Job {
    /// Queued behind another job: the server shows it `running` with no
    /// `started_at` and no total until the runner takes it.
    fn queued(&self) -> bool {
        self.state == "running" && self.started_at.is_none()
    }

    fn tally(&mut self) {
        self.messages = self.channels.iter().map(|c| c.messages).sum();
        self.messages_total = if self.queued() {
            None
        } else if self.state == "running" {
            Some(
                self.channels
                    .iter()
                    .map(|c| {
                        if c.state == "done" {
                            c.messages
                        } else {
                            c.expected
                        }
                    })
                    .sum(),
            )
        } else {
            Some(self.messages)
        };
    }
}

#[derive(Clone, Serialize)]
pub struct JobChannel {
    pub id: &'static str,
    pub name: &'static str,
    pub state: &'static str,
    pub messages: u32,
    /// What the read will find, known when the job starts.
    #[serde(skip)]
    pub expected: u32,
}

#[derive(Deserialize)]
pub struct RescanRequest {
    pub channels: Vec<String>,
    pub window: String,
}

const MODEL: &str = "kanata/extract";
/// The server's `extraction_off` sentence (also the summary's `rescan_off`).
pub const RESCAN_OFF: &str =
    "Re-reading needs watching and the extractor switched on (Config → Watching).";

impl Store {
    fn hour_minute(h: i64) -> i64 {
        Self::start(false) * 1440 - 8 * 60 + h * 60
    }

    pub fn extractions(&self, query: &LogQuery) -> Result<Value, MoveError> {
        query.validate(&EXTRACTION_OUTCOMES, false)?;
        let mut all = calls();
        all.sort_by_key(|c| std::cmp::Reverse(c.hour));
        let listed: Vec<&Call> = all
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
            .collect();
        let rows: Vec<Value> = listed
            .iter()
            .map(|c| {
                json!({
                    "id": c.id, "short_id": c.short_id, "at": super::clock::iso_z(Self::hour_minute(c.hour)),
                    "model": c.model, "latency_ms": c.latency_ms, "messages": c.messages.len(),
                    "changes": c.amendments.len(), "channel": seed::channel(c.channel).map(|x| x.1),
                    "channel_id": c.channel, "error": c.error, "outcome": c.outcome,
                    "prompt_tokens": c.usage.map(|u| u.0), "completion_tokens": c.usage.map(|u| u.1),
                    "reasoning_tokens": if c.id == "x-bm" { Some(24) } else { None::<u32> },
                })
            })
            .collect();
        let mut listed_models: Vec<&str> = listed.iter().map(|c| c.model).collect();
        listed_models.sort_unstable();
        listed_models.dedup();
        let summary: Vec<Value> = listed_models
            .iter()
            .map(|m| {
                let mine: Vec<&&Call> = listed.iter().filter(|c| c.model == *m).collect();
                let mut row = usage_summary(
                    mine.iter()
                        .map(|c| (c.usage.map(|u| u.0), c.usage.map(|u| u.1), c.estimate)),
                );
                row.insert("model".into(), json!(m));
                row.insert("count".into(), json!(mine.len()));
                Value::Object(row)
            })
            .collect();
        let mut models: Vec<&str> = all.iter().map(|c| c.model).collect();
        models.sort_unstable();
        models.dedup();
        Ok(json!({
            "model": MODEL,
            "summary": summary,
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
            .map(|(i, (who, text))| json!({ "id": format!("{}-{i}", c.short_id), "author": seed::member_name(who).map_or("someone", |m| m.1), "author_id": who, "at": super::clock::iso_z(Self::hour_minute(c.hour) - 5 + i as i64), "content": text }))
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
            "id": c.id, "short_id": c.short_id, "at": super::clock::iso_z(Self::hour_minute(c.hour)), "model": c.model, "outcome": c.outcome,
            "latency_ms": c.latency_ms, "channel": seed::channel(c.channel).map(|x| x.1), "channel_id": c.channel, "error": c.error,
            "prompt": prompt, "raw_response": raw.to_string(), "amendments": amendments, "messages": chat,
            "prompt_tokens": c.usage.map(|u| u.0), "completion_tokens": c.usage.map(|u| u.1),
            "prompt_estimate": c.estimate,
            "reasoning_content": if c.id == "x-bm" { Some("The messages agree on Wednesday at the existing time.") } else { None },
            "reasoning_tokens": if c.id == "x-bm" { Some(24) } else { None::<u32> },
            // Invented gateway ids; older calls predate them.
            "session_id": (c.id == "x-bm").then_some("kanade-extraction-1a2b3c4d-3"),
            "request_ids": if c.id == "x-bm" { vec!["kanade-extraction-1a2b3c4d-3-1"] } else { Vec::new() },
            // Older legacy calls were logged before the context was.
            "context": if c.model == MODEL { json!({"window": 8_192, "reserve": 2_500, "source": "local_default"}) } else { Value::Null },
        }))
    }

    /// Why a re-read would be refused now: watching paused or the extractor off.
    pub fn rescan_off(&self) -> Option<&'static str> {
        (self.config.paused || !self.config.extract_enabled).then_some(RESCAN_OFF)
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
        let mut channels: Vec<JobChannel> = Vec::new();
        for c in req
            .channels
            .iter()
            .filter_map(|id| seed::channel(id).filter(|c| c.2))
        {
            if !channels.iter().any(|known| known.id == c.0) {
                channels.push(JobChannel {
                    id: c.0,
                    name: c.1,
                    state: "queued",
                    messages: 0,
                    expected: 40 + c.1.len() as u32,
                });
            }
        }
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
        // As the server: checked after the request itself is valid.
        if let Some(off) = self.rescan_off() {
            return Err(MoveError::Coded(409, "extraction_off", off.into()));
        }
        // As the server's queue (`Rescans::submit`): the newest live job that
        // already covers these channels is attached to when it is running or
        // reads the same window; queued with another window, it is replaced.
        // Anything else queues behind the running job.
        if let Some(active) = self.jobs.iter_mut().rev().find(|j| j.state == "running")
            && channels
                .iter()
                .all(|c| active.channels.iter().any(|known| known.id == c.id))
        {
            if !active.queued() || active.window == req.window {
                return Ok(active.clone());
            }
            active.state = "cancelled";
            active.tally();
        }
        let busy = self
            .jobs
            .iter()
            .any(|j| j.state == "running" && !j.queued());
        let (id, _) = self.fresh_id("job");
        let mut job = Job {
            id,
            state: "running",
            window: req.window,
            started_at: (!busy).then(super::clock::iso_now),
            channels,
            messages: 0,
            messages_total: None,
            proposals: 0,
        };
        job.tally();
        self.jobs.push(job.clone());
        Ok(job)
    }

    /// The runner takes the oldest queued job once none is running.
    fn start_next(&mut self) {
        if self
            .jobs
            .iter()
            .any(|j| j.state == "running" && !j.queued())
        {
            return;
        }
        if let Some(next) = self.jobs.iter_mut().find(|j| j.queued()) {
            next.started_at = Some(super::clock::iso_now());
            next.tally();
        }
    }

    /// Each poll advances the running job one channel, like a cooperative
    /// drain, then answers with the job asked about (a queued one waits).
    pub fn poll_rescan(&mut self, id: &str) -> Result<Job, MoveError> {
        if !self.jobs.iter().any(|j| j.id == id) {
            return Err(MoveError::NotFound);
        }
        if let Some(job) = self
            .jobs
            .iter_mut()
            .find(|j| j.state == "running" && !j.queued())
        {
            if let Some(ch) = job.channels.iter_mut().find(|c| c.state == "reading") {
                ch.state = "done";
                ch.messages = ch.expected;
            }
            if let Some(next) = job.channels.iter_mut().find(|c| c.state == "queued") {
                next.state = "reading";
            } else if job.channels.iter().all(|c| c.state == "done") {
                job.state = "done";
            }
            job.tally();
        }
        self.start_next();
        Ok(self.jobs.iter().find(|j| j.id == id).unwrap().clone())
    }

    pub fn cancel_rescan(&mut self, id: &str) -> Result<Job, MoveError> {
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or(MoveError::NotFound)?;
        if job.state == "running" {
            job.state = "cancelled";
            job.tally();
        }
        let job = job.clone();
        self.start_next();
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    use super::super::logfilter::LogQuery;
    use super::super::tests::store;

    #[test]
    fn a_rescan_reports_its_start_and_reaches_its_message_total() {
        let mut s = store();
        let req = |channels: &[&str]| super::RescanRequest {
            channels: channels.iter().map(|c| (*c).to_owned()).collect(),
            window: "week".into(),
        };
        let watched: Vec<String> = super::Store::rescan_targets()
            .as_array()
            .unwrap()
            .iter()
            .take(2)
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect();
        let ids: Vec<&str> = watched.iter().map(String::as_str).collect();
        let job = s.start_rescan(req(&ids)).ok().expect("started");
        assert_eq!(job.started_at.as_deref(), Some("2026-09-29T04:00:00Z"));
        let total = job.messages_total.expect("known at the start");
        assert_eq!(job.messages, 0);
        let mut polled = s.poll_rescan(&job.id).ok().unwrap();
        while polled.state == "running" {
            assert_eq!(
                polled.messages_total,
                Some(total),
                "estimates are exact here"
            );
            polled = s.poll_rescan(&job.id).ok().unwrap();
        }
        assert_eq!(polled.state, "done");
        assert_eq!(
            (polled.messages, polled.messages_total),
            (total, Some(total))
        );
        assert_eq!(polled.started_at, job.started_at);
    }

    #[test]
    fn a_rescan_is_refused_while_watching_is_paused_or_the_extractor_is_off() {
        let watched = super::Store::rescan_targets()[0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let req = || super::RescanRequest {
            channels: vec![watched.clone()],
            window: "week".into(),
        };
        for (paused, extract_enabled) in [(true, true), (false, false)] {
            let mut s = store();
            s.config.paused = paused;
            s.config.extract_enabled = extract_enabled;
            match s.start_rescan(req()) {
                Err(super::MoveError::Coded(409, "extraction_off", message)) => {
                    assert!(message.contains("Config → Watching"), "{message}");
                }
                Ok(job) => panic!("expected extraction_off, started {}", job.id),
                Err(other) => panic!("expected extraction_off, got {other}"),
            }
            assert!(s.jobs.is_empty(), "nothing queued");
        }
    }

    #[test]
    fn a_second_rescan_attaches_or_queues_as_the_server_does() {
        let mut s = store();
        let watched: Vec<String> = super::Store::rescan_targets()
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect();
        let (a, b) = (watched[0].as_str(), watched[1].as_str());
        let req = |channels: &[&str], window: &str| super::RescanRequest {
            channels: channels.iter().map(|c| (*c).to_owned()).collect(),
            window: window.into(),
        };
        let first = s.start_rescan(req(&[a, b], "week")).ok().unwrap();
        assert!(first.started_at.is_some(), "nothing ahead: it starts");
        // Covered by the running job: attached, whatever the window.
        let again = s.start_rescan(req(&[a], "two_weeks")).ok().unwrap();
        assert_eq!(again.id, first.id);

        // Not covered: queued behind it, shown running with no start or total.
        let (c, d) = (watched[2].as_str(), watched[3].as_str());
        let queued = s.start_rescan(req(&[c, d], "week")).ok().unwrap();
        assert_ne!(queued.id, first.id);
        assert_eq!(queued.state, "running");
        assert_eq!(
            (queued.started_at.as_ref(), queued.messages_total),
            (None, None)
        );
        // The same window attaches to the queued job ...
        let same = s.start_rescan(req(&[c], "week")).ok().unwrap();
        assert_eq!(same.id, queued.id);
        // ... another window replaces it with a new queued job.
        let replaced = s.start_rescan(req(&[d], "since_reset")).ok().unwrap();
        assert_ne!(replaced.id, queued.id);
        assert_eq!(s.poll_rescan(&queued.id).ok().unwrap().state, "cancelled");
        assert!(
            s.poll_rescan(&replaced.id)
                .ok()
                .unwrap()
                .started_at
                .is_none()
        );

        // Once the running job ends the queued one is taken.
        let stopped = s.cancel_rescan(&first.id).ok().unwrap();
        assert_eq!(stopped.state, "cancelled");
        let taken = s.poll_rescan(&replaced.id).ok().unwrap();
        assert!(taken.started_at.is_some());
        assert!(taken.messages_total.is_some());
        while s.poll_rescan(&replaced.id).ok().unwrap().state == "running" {}
        assert_eq!(s.poll_rescan(&replaced.id).ok().unwrap().state, "done");
    }

    #[test]
    fn the_summary_says_why_a_rescan_would_be_refused() {
        let mut s = store();
        assert_eq!(s.summary().rescan_off, None);
        s.config.extract_enabled = false;
        assert_eq!(s.summary().rescan_off, Some(super::RESCAN_OFF));
        s.config.extract_enabled = true;
        s.config.paused = true;
        assert_eq!(s.summary().rescan_off, Some(super::RESCAN_OFF));
    }

    #[test]
    fn reasoning_fixtures_preserve_text_counts_and_unknowns() {
        let s = store();
        let bm = s.extraction("x-bm").ok().expect("call");
        assert_eq!(
            bm["reasoning_content"],
            "The messages agree on Wednesday at the existing time."
        );
        assert_eq!(bm["reasoning_tokens"], 24);
        let absent = s.extraction("x-limbo").ok().expect("unreported");
        assert!(absent["reasoning_content"].is_null());
        assert!(absent["reasoning_tokens"].is_null());
        assert_eq!(bm["session_id"], "kanade-extraction-1a2b3c4d-3");
        assert_eq!(
            bm["request_ids"],
            serde_json::json!(["kanade-extraction-1a2b3c4d-3-1"])
        );
        assert!(absent["session_id"].is_null());
        assert_eq!(absent["request_ids"], serde_json::json!([]));
    }

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

    #[test]
    fn usage_is_null_when_unreported_and_summed_per_model() {
        let s = store();
        let proposed = s
            .extractions(&LogQuery {
                outcome: Some("proposed".into()),
                ..Default::default()
            })
            .ok()
            .unwrap();
        // x-bm and x-kalos reported (1820/1700, 2010/1880); x-limbo did not.
        assert_eq!(
            proposed["summary"],
            serde_json::json!([{"model": "kanata/extract", "count": 3, "prompt_tokens": 3830,
                "completion_tokens": 136, "reported": 2, "est_ratio": 1.07}])
        );
        let limbo = s.extraction("x-limbo").ok().unwrap();
        assert!(limbo["prompt_tokens"].is_null() && limbo["completion_tokens"].is_null());
        assert_eq!(limbo["prompt_estimate"], 1_500);
        assert_eq!(limbo["context"]["reserve"], 2_500);
    }
}
