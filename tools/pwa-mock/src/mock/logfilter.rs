//! Server-side filters for the Chat and Extractions logs (user request,
//! 2026-09-25): model, date range in guild time, typed outcome (several),
//! channel, member, free text; Chat also tool used and minimum latency.

use super::MoveError;
use super::clock::iso_date;
use serde::Deserialize;

pub const CHAT_OUTCOMES: [&str; 10] = [
    "answered",
    "refused",
    "clarified",
    "error",
    "timeout",
    "rate_limited",
    "turned_away",
    "content_blocked",
    "withheld",
    "clean_retry",
];

pub const EXTRACTION_OUTCOMES: [&str; 6] = [
    "proposed",
    "no_change",
    "failed",
    "turned_away",
    "content_blocked",
    "self_service_link",
];

#[derive(Deserialize, Default)]
pub struct LogQuery {
    pub model: Option<String>,
    /// Inclusive guild-local dates, `YYYY-MM-DD`.
    pub from: Option<String>,
    pub to: Option<String>,
    /// Comma-separated outcomes; a row matches any of them.
    pub outcome: Option<String>,
    pub channel: Option<String>,
    pub member: Option<String>,
    pub q: Option<String>,
    /// Chat only.
    pub tool: Option<String>,
    /// Kept as text so a malformed value reaches `validate` (422), not axum's 400.
    pub min_ms: Option<String>,
}

/// What a row offers the filters.
pub struct Facts<'a> {
    pub minute: i64,
    pub models: &'a [&'a str],
    pub outcome: &'a str,
    pub channel: &'a str,
    pub members: &'a [&'a str],
    pub text: &'a [&'a str],
    pub tools: &'a [&'a str],
    pub latency_ms: u32,
}

fn some(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn valid_date(d: &str) -> bool {
    d.len() == 10
        && d.as_bytes()[4] == b'-'
        && d.as_bytes()[7] == b'-'
        && d.chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

impl LogQuery {
    fn outcomes(&self) -> Vec<&str> {
        some(&self.outcome)
            .map(|o| {
                o.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whole milliseconds; `None` when unset or malformed (`validate` refuses the latter).
    fn min_ms(&self) -> Option<u32> {
        some(&self.min_ms).and_then(|ms| ms.parse().ok())
    }

    /// Unknown outcomes and malformed dates are refused (422), never ignored.
    pub fn validate(&self, allowed: &[&str], chat: bool) -> Result<(), MoveError> {
        let bad = |m: String| MoveError::Coded(422, "invalid_filter", m);
        for o in self.outcomes() {
            if !allowed.contains(&o) {
                return Err(bad(format!("Unknown outcome “{o}”.")));
            }
        }
        for d in [some(&self.from), some(&self.to)].into_iter().flatten() {
            if !valid_date(d) {
                return Err(bad(format!("Dates are YYYY-MM-DD, not “{d}”.")));
            }
        }
        if let (Some(f), Some(t)) = (some(&self.from), some(&self.to))
            && f > t
        {
            return Err(bad("The range starts after it ends.".into()));
        }
        if !chat && (some(&self.tool).is_some() || some(&self.min_ms).is_some()) {
            return Err(bad("Tool and latency filters are for Chat only.".into()));
        }
        if let Some(ms) = some(&self.min_ms)
            && !(ms.bytes().all(|b| b.is_ascii_digit()) && ms.parse::<u32>().is_ok())
        {
            return Err(bad(format!(
                "Minimum latency is whole milliseconds, not “{ms}”."
            )));
        }
        Ok(())
    }

    pub fn matches(&self, f: &Facts) -> bool {
        let day = iso_date(f.minute.div_euclid(1440));
        let q = some(&self.q).map(str::to_lowercase);
        some(&self.model).is_none_or(|m| f.models.contains(&m))
            && some(&self.from).is_none_or(|d| day.as_str() >= d)
            && some(&self.to).is_none_or(|d| day.as_str() <= d)
            && {
                let wanted = self.outcomes();
                wanted.is_empty() || wanted.contains(&f.outcome)
            }
            && some(&self.channel).is_none_or(|c| c == f.channel)
            && some(&self.member).is_none_or(|m| f.members.contains(&m))
            && q.is_none_or(|q| f.text.iter().any(|t| t.to_lowercase().contains(&q)))
            && some(&self.tool).is_none_or(|t| f.tools.contains(&t))
            && self.min_ms().is_none_or(|ms| f.latency_ms >= ms)
    }
}

#[cfg(test)]
mod tests {
    use super::{CHAT_OUTCOMES, LogQuery};
    use axum::extract::Query;

    fn query(qs: &str) -> LogQuery {
        let uri = format!("http://mock/api/admin/chat?{qs}").parse().unwrap();
        Query::<LogQuery>::try_from_uri(&uri).ok().unwrap().0
    }

    #[test]
    fn malformed_min_ms_is_an_invalid_filter_not_a_400() {
        for bad in ["1e3", "-5", "5.5", "%2B5", "lots", "99999999999"] {
            let err = query(&format!("min_ms={bad}"))
                .validate(&CHAT_OUTCOMES, true)
                .err()
                .unwrap_or_else(|| panic!("{bad} was accepted"));
            assert!(
                matches!(err, super::MoveError::Coded(422, "invalid_filter", _)),
                "{bad}"
            );
        }
        assert!(query("min_ms=5000").validate(&CHAT_OUTCOMES, true).is_ok());
        assert!(query("min_ms=").validate(&CHAT_OUTCOMES, true).is_ok());
        assert!(query("min_ms=5000").validate(&[], false).is_err());
    }
}
