//! Log filters from the query string. Every refusal is `422 invalid_filter`:
//! an unknown, repeated or undecodable key, an unknown outcome, a malformed
//! or inverted date range, a `min_ms` that is not whole milliseconds, or a
//! Chat-only filter on Extractions. Empty values mean "not set" (the mock's
//! rule).

use axum::http::{StatusCode, Uri};
use chrono::{DateTime, Days, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

use super::super::{history::parse::date, write::Refusal};
use crate::{
    api::auth::wire,
    domain::model_log::{ChatFilter, ChatOutcome, ExtractionFilter, ExtractionOutcome, MAX_PAGE},
};

const COMMON: [&str; 7] = ["model", "from", "to", "outcome", "channel", "member", "q"];
const CHAT_ONLY: [&str; 2] = ["tool", "min_ms"];

pub fn invalid(message: impl Into<String>) -> Refusal {
    Refusal::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_filter", message)
}

/// `[from, to)` instants, each optional.
type Range = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    fn read(uri: &Uri, chat: bool) -> Result<Self, Refusal> {
        let pairs = wire::query_pairs(uri.query());
        let sent = uri.query().map_or(0, |query| {
            query.split('&').filter(|pair| !pair.is_empty()).count()
        });
        if pairs.len() != sent {
            return Err(invalid("A filter could not be read."));
        }
        for (index, (key, value)) in pairs.iter().enumerate() {
            if CHAT_ONLY.contains(&key.as_str()) && !chat {
                if value.trim().is_empty() {
                    continue;
                }
                return Err(invalid("Tool and latency filters are for Chat only."));
            }
            if !COMMON.contains(&key.as_str()) && !CHAT_ONLY.contains(&key.as_str()) {
                return Err(invalid(format!("Unknown filter “{key}”.")));
            }
            if pairs[..index].iter().any(|(earlier, _)| earlier == key) {
                return Err(invalid(format!("Send “{key}” once.")));
            }
        }
        Ok(Self { pairs })
    }

    fn get(&self, key: &str) -> Option<String> {
        self.pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    fn outcomes<T>(&self, parse: impl Fn(&str) -> Option<T>) -> Result<Vec<T>, Refusal> {
        let Some(list) = self.get("outcome") else {
            return Ok(Vec::new());
        };
        list.split(',')
            .map(str::trim)
            .filter(|outcome| !outcome.is_empty())
            .map(|outcome| {
                parse(outcome).ok_or_else(|| invalid(format!("Unknown outcome “{outcome}”.")))
            })
            .collect()
    }

    /// `from` and `to` as `[from 00:00, day after to 00:00)` in the guild zone.
    fn range(&self, zone: Tz) -> Result<Range, Refusal> {
        let day = |key: &str| -> Result<Option<NaiveDate>, Refusal> {
            self.get(key)
                .map(|text| {
                    date(&text)
                        .ok_or_else(|| invalid(format!("Dates are YYYY-MM-DD, not “{text}”.")))
                })
                .transpose()
        };
        let (from, to) = (day("from")?, day("to")?);
        if let (Some(from), Some(to)) = (from, to)
            && from > to
        {
            return Err(invalid("The range starts after it ends."));
        }
        let start = |day: NaiveDate| {
            day_start(zone, day).ok_or_else(|| invalid("That date is out of range."))
        };
        let end = to
            .map(|to| {
                to.checked_add_days(Days::new(1))
                    .ok_or_else(|| invalid("That date is out of range."))
                    .and_then(start)
            })
            .transpose()?;
        Ok((from.map(start).transpose()?, end))
    }

    fn min_ms(&self) -> Result<Option<u64>, Refusal> {
        self.get("min_ms")
            .map(|text| {
                text.bytes()
                    .all(|byte| byte.is_ascii_digit())
                    .then(|| text.parse::<u32>().ok())
                    .flatten()
                    .map(u64::from)
                    .ok_or_else(|| {
                        invalid(format!(
                            "Minimum latency is whole milliseconds, not “{text}”."
                        ))
                    })
            })
            .transpose()
    }
}

/// The first instant of a guild-local day (01:00 if midnight is skipped).
fn day_start(zone: Tz, day: NaiveDate) -> Option<DateTime<Utc>> {
    [0, 1].into_iter().find_map(|hour| {
        zone.from_local_datetime(&day.and_time(NaiveTime::from_hms_opt(hour, 0, 0)?))
            .earliest()
            .map(|at| at.with_timezone(&Utc))
    })
}

pub fn extractions(uri: &Uri, zone: Tz) -> Result<ExtractionFilter, Refusal> {
    let query = Query::read(uri, false)?;
    let (from, to) = query.range(zone)?;
    Ok(ExtractionFilter {
        model: query.get("model"),
        from,
        to,
        outcomes: query.outcomes(ExtractionOutcome::parse)?,
        channel: query.get("channel"),
        member: query.get("member"),
        q: query.get("q"),
        cursor: None,
        limit: MAX_PAGE,
    })
}

pub fn chats(uri: &Uri, zone: Tz) -> Result<ChatFilter, Refusal> {
    let query = Query::read(uri, true)?;
    let (from, to) = query.range(zone)?;
    Ok(ChatFilter {
        model: query.get("model"),
        from,
        to,
        outcomes: query.outcomes(ChatOutcome::parse)?,
        channel: query.get("channel"),
        member: query.get("member"),
        q: query.get("q"),
        tool: query.get("tool"),
        min_ms: query.min_ms()?,
        cursor: None,
        limit: MAX_PAGE,
    })
}
