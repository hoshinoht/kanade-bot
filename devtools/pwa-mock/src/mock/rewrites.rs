//! Rewrites (the persona rewrite log): the daily reminder-header batch and catch-up,
//! `/debug` trials and self-service nudges, each with its verdict, gate rule
//! or error code, route, usage against the reservation and the reply.

use std::collections::BTreeMap;

use super::clock::iso_date;
use super::extractions::usage_summary;
use super::{MoveError, Store};
use serde_json::{Value, json};

const MODEL: &str = "kanata/rewrite";
const KINDS: [&str; 4] = ["day_of", "countdown", "digest", "nudge"];
const STAGES: [&str; 4] = ["batch", "catchup", "debug", "nudge"];
const VERDICTS: [&str; 8] = [
    "accepted",
    "rejected",
    "timeout",
    "unavailable",
    "refused",
    "misconfigured",
    "no_rewriter",
    "no_persona",
];
const KEYS: [&str; 7] = ["model", "from", "to", "kind", "stage", "verdict", "q"];

struct Attempt {
    id: &'static str,
    /// Minutes before the mock's week start (see `Store::hour_minute`).
    hour: i64,
    kind: &'static str,
    stage: &'static str,
    context: &'static str,
    verdict: &'static str,
    rule: Option<&'static str>,
    code: Option<&'static str>,
    latency_ms: Option<u32>,
    model: Option<&'static str>,
    /// Reported (prompt, completion) tokens.
    usage: Option<(u32, u32)>,
    reasoning_tokens: Option<u32>,
    /// (reservation, max_tokens).
    reserved: Option<(u32, u32)>,
    /// The call budget a reservation exceeded (refused before sending).
    budget: Option<u32>,
    seed: &'static str,
    reply: Option<&'static str>,
    reasoning: Option<&'static str>,
    line: Option<&'static str>,
}

fn attempts() -> Vec<Attempt> {
    let call = |id, hour, kind, stage, context, verdict| Attempt {
        id,
        hour,
        kind,
        stage,
        context,
        verdict,
        rule: None,
        code: None,
        latency_ms: Some(1_240),
        model: Some(MODEL),
        usage: Some((191, 9)),
        reasoning_tokens: None,
        reserved: Some((287, 96)),
        budget: None,
        seed: "Today — {day}",
        reply: None,
        reasoning: None,
        line: None,
    };
    vec![
        Attempt {
            reply: Some("Waku waku — {day}!"),
            line: Some("Waku waku — {day}!"),
            reasoning: Some("Keep {day} and stay short."),
            reasoning_tokens: Some(12),
            ..call(
                "rw-dayof",
                108,
                "day_of",
                "batch",
                "day_of:r-bm:2026-09-29",
                "accepted",
            )
        },
        Attempt {
            code: Some("budget_exceeded"),
            latency_ms: Some(14_708),
            usage: Some((300, 112)),
            reply: Some("Waku waku!"),
            reasoning: Some(
                "The user wants an interjection. Let me think about which one fits the countdown…",
            ),
            reasoning_tokens: Some(98),
            seed: "Onward!",
            line: Some("Onward!"),
            ..call(
                "rw-over",
                104,
                "countdown",
                "batch",
                "countdown:r-kalos:60",
                "unavailable",
            )
        },
        Attempt {
            rule: Some("factual term"),
            reply: Some("Ready at 9!"),
            seed: "Let's go!",
            line: Some("Let's go!"),
            ..call(
                "rw-debug",
                96,
                "digest",
                "debug",
                "/debug header digest · try 1/2",
                "rejected",
            )
        },
        Attempt {
            reply: Some("Yay!"),
            seed: "Let's go!",
            line: Some("Yay!"),
            ..call(
                "rw-debug-2",
                95,
                "digest",
                "debug",
                "/debug header digest · try 2/2",
                "accepted",
            )
        },
        Attempt {
            code: Some("budget_exceeded"),
            latency_ms: Some(1),
            usage: None,
            reserved: Some((16_391, 16_000)),
            budget: Some(16_384),
            seed: "Let's go!",
            line: Some("Let's go!"),
            ..call(
                "rw-reserve",
                90,
                "digest",
                "batch",
                "digest:2026-10-01",
                "unavailable",
            )
        },
        Attempt {
            code: Some("busy"),
            latency_ms: Some(0),
            usage: None,
            reserved: None,
            seed: "Onward!",
            line: Some("Onward!"),
            ..call(
                "rw-busy",
                80,
                "countdown",
                "catchup",
                "countdown:r-lotus:15",
                "unavailable",
            )
        },
        Attempt {
            latency_ms: Some(2_000),
            usage: None,
            reserved: None,
            model: None,
            seed: "Move {boss} to {day} {time} yourself.",
            line: Some("Move {boss} to {day} {time} yourself."),
            ..call(
                "rw-nudge",
                60,
                "nudge",
                "nudge",
                "self_service · playful",
                "timeout",
            )
        },
        Attempt {
            latency_ms: None,
            usage: None,
            reserved: None,
            model: None,
            line: Some("Today — {day}"),
            ..call(
                "rw-persona",
                30,
                "day_of",
                "batch",
                "day_of:r-fa:2026-09-27",
                "no_persona",
            )
        },
    ]
}

fn some(map: &BTreeMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn valid_date(d: &str) -> bool {
    d.len() == 10
        && d.as_bytes()[4] == b'-'
        && d.as_bytes()[7] == b'-'
        && d.chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

impl Attempt {
    fn row(&self) -> serde_json::Map<String, Value> {
        let Value::Object(row) = json!({
            "id": self.id,
            "short_id": self.id.replace('-', "").chars().take(8).collect::<String>(),
            "at": super::clock::iso_z(Store::hour_minute(self.hour)),
            "kind": self.kind,
            "stage": self.stage,
            "context": self.context,
            "verdict": self.verdict,
            "rule": self.rule,
            "code": self.code,
            "latency_ms": self.latency_ms,
            "model": self.model,
            "reasoning": self.model.map(|_| "low"),
            "prompt_tokens": self.usage.map(|u| u.0),
            "completion_tokens": self.usage.map(|u| u.1),
            "reasoning_tokens": self.reasoning_tokens,
            "reservation": self.reserved.map(|r| r.0),
            "budget": self.budget,
            "seed": self.seed,
            "line": self.line,
        }) else {
            unreachable!()
        };
        row
    }

    fn estimate(&self) -> Option<u32> {
        self.reserved.map(|(reservation, max)| reservation - max)
    }
}

impl Store {
    /// As the server: unknown keys or values, malformed or inverted dates
    /// are `422 invalid_filter`.
    pub fn rewrites(&self, query: &BTreeMap<String, String>) -> Result<Value, MoveError> {
        let bad = |m: String| MoveError::Coded(422, "invalid_filter", m);
        if let Some(key) = query.keys().find(|key| !KEYS.contains(&key.as_str())) {
            return Err(bad(format!("Unknown filter “{key}”.")));
        }
        let kind = some(query, "kind");
        let stage = some(query, "stage");
        let verdicts: Vec<String> = some(query, "verdict")
            .map(|list| {
                list.split(',')
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        for (key, value, allowed) in [
            ("kind", kind.as_deref(), &KINDS[..]),
            ("stage", stage.as_deref(), &STAGES[..]),
        ] {
            if let Some(value) = value
                && !allowed.contains(&value)
            {
                return Err(bad(format!("Unknown {key} “{value}”.")));
            }
        }
        if let Some(v) = verdicts.iter().find(|v| !VERDICTS.contains(&v.as_str())) {
            return Err(bad(format!("Unknown verdict “{v}”.")));
        }
        let (from, to) = (some(query, "from"), some(query, "to"));
        for d in [&from, &to].into_iter().flatten() {
            if !valid_date(d) {
                return Err(bad(format!("Dates are YYYY-MM-DD, not “{d}”.")));
            }
        }
        if let (Some(f), Some(t)) = (&from, &to)
            && f > t
        {
            return Err(bad("The range starts after it ends.".into()));
        }
        let model = some(query, "model");
        let q = some(query, "q").map(|q| q.to_lowercase());
        let mut all = attempts();
        all.sort_by_key(|a| std::cmp::Reverse(a.hour));
        let listed: Vec<&Attempt> = all
            .iter()
            .filter(|a| {
                let day = iso_date(Self::hour_minute(a.hour).div_euclid(1440));
                let text = [
                    Some(a.seed),
                    a.reply,
                    a.line,
                    Some(a.context),
                    a.rule,
                    a.code,
                ];
                model.as_deref().is_none_or(|m| a.model == Some(m))
                    && kind.as_deref().is_none_or(|k| a.kind == k)
                    && stage.as_deref().is_none_or(|s| a.stage == s)
                    && (verdicts.is_empty() || verdicts.iter().any(|v| v == a.verdict))
                    && from.as_deref().is_none_or(|d| day.as_str() >= d)
                    && to.as_deref().is_none_or(|d| day.as_str() <= d)
                    && q.as_deref()
                        .is_none_or(|q| text.iter().flatten().any(|t| t.to_lowercase().contains(q)))
            })
            .collect();
        let mut models: Vec<&str> = listed.iter().filter_map(|a| a.model).collect();
        models.sort_unstable();
        models.dedup();
        let summary: Vec<Value> = models
            .iter()
            .map(|m| {
                let mine: Vec<&&Attempt> = listed.iter().filter(|a| a.model == Some(*m)).collect();
                let mut row = usage_summary(
                    mine.iter()
                        .map(|a| (a.usage.map(|u| u.0), a.usage.map(|u| u.1), a.estimate())),
                );
                row.insert("model".into(), json!(m));
                row.insert("count".into(), json!(mine.len()));
                row.insert(
                    "accepted".into(),
                    json!(mine.iter().filter(|a| a.verdict == "accepted").count()),
                );
                Value::Object(row)
            })
            .collect();
        let facet = |pick: fn(&Attempt) -> Option<&'static str>| {
            let mut values: Vec<&str> = all.iter().filter_map(pick).collect();
            values.sort_unstable();
            values.dedup();
            values
        };
        Ok(json!({
            "summary": summary,
            "rows": listed.iter().map(|a| Value::Object(a.row())).collect::<Vec<_>>(),
            "total": all.len(),
            "facets": {
                "models": facet(|a| a.model),
                "kinds": facet(|a| Some(a.kind)),
                "stages": facet(|a| Some(a.stage)),
                "verdicts": facet(|a| Some(a.verdict)),
            },
        }))
    }

    pub fn rewrite(&self, id: &str) -> Result<Value, MoveError> {
        let all = attempts();
        let a = all.iter().find(|a| a.id == id).ok_or(MoveError::NotFound)?;
        let mut row = a.row();
        row.insert("reply".into(), json!(a.reply));
        row.insert("reasoning_content".into(), json!(a.reasoning));
        row.insert("max_output_tokens".into(), json!(a.reserved.map(|r| r.1)));
        row.insert("prompt_estimate".into(), json!(a.estimate()));
        row.insert(
            "request_id".into(),
            json!(
                a.model
                    .map(|_| format!("kanade-rewrite-0000beef-{}-1", a.hour))
            ),
        );
        Ok(Value::Object(row))
    }
}
