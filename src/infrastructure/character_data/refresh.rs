//! Fetching a linked character (`id` when the ocid is missing or 29 days
//! old, then `basic` + `stat`), the admin's on-demand refresh (sharing the
//! 24-hour cap) and the daily batch and retry passes.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::calls::{Batch, CallFailed};
use super::{BatchReport, CharacterData, log};
use crate::domain::characters::{Character, ign_key};
use crate::domain::chardata::{
    FUTURE_SKEW, Failure, Link, LinkPut, LinkStatus, MAX_TEXT_CHARS, MAX_URL_CHARS,
    OCID_RENEW_AFTER, PREPARING_RETRY, REFRESH_AGE, REFRESH_CAP, RefreshState, Snapshot,
    parse_date,
};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::nexon::Basic;

/// How a fetch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchState {
    /// New data is stored.
    Stored,
    /// The character could not be resolved or changed its name: the link
    /// is stale and its data gone.
    Stale,
    /// The link went away (or changed) while the calls ran; nothing stored.
    Unlinked,
    /// Not done: an error code or reason (`budget`, `maintenance`,
    /// `key_refused`, `OPENAPI00009`, …) and when it is tried again.
    Failed {
        code: String,
        retry_at: Option<DateTime<Utc>>,
    },
}

/// The admin's on-demand refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshOutcome {
    Done(FetchState),
    NotConnected,
    /// No active link on an active roster character.
    NotLinked,
    /// Refreshed less than 24 hours ago: possible from then.
    TooSoon(DateTime<Utc>),
    /// Calls are held (key refused, maintenance or the day's budget).
    Unavailable(String),
}

fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && value.chars().count() <= MAX_TEXT_CHARS)
}

/// The snapshot `basic` + combat power make, everything optional and
/// bounded; image URLs only over https.
fn facts(
    character_id: &str,
    ocid: String,
    ocid_fetched_at: DateTime<Utc>,
    basic: Basic,
    combat_power: Option<u64>,
    fetched_at: DateTime<Utc>,
) -> Snapshot {
    Snapshot {
        character_id: character_id.to_owned(),
        ocid,
        ocid_fetched_at,
        character_name: text(basic.character_name),
        world_name: text(basic.world_name),
        character_class: text(basic.character_class),
        level: basic
            .character_level
            .and_then(|level| u16::try_from(level).ok()),
        combat_power,
        image_url: basic
            .character_image
            .filter(|url| url.starts_with("https://") && url.len() <= MAX_URL_CHARS),
        data_date: basic.date.as_deref().and_then(parse_date),
        fetched_at,
        expires_at: Snapshot::expiry(fetched_at, ocid_fetched_at),
    }
}

/// A call answered `OPENAPI00003`.
fn invalid_identifier(failed: &CallFailed) -> bool {
    failed.failure() == Some(Failure::InvalidIdentifier)
}

/// `/id` answers an unknown name with `400 OPENAPI00004` ("Please input
/// valid parameter"; live MapleSEA, 2026-10-12), not an empty body. The name
/// is its only parameter, so on `/id` this means no such character.
fn no_such_name(failed: &CallFailed) -> bool {
    failed.label() == "OPENAPI00004"
}

enum Step<T> {
    Ok(T),
    Stale,
    Failed(CallFailed),
}

impl CharacterData {
    async fn resolve(&self, ign: &str, batch: &mut Option<&mut Batch>) -> Step<String> {
        match self
            .call("id", batch.as_deref_mut(), |client| client.id(ign))
            .await
        {
            Ok(Some(ocid)) => Step::Ok(ocid),
            Ok(None) => Step::Stale,
            Err(failed) if invalid_identifier(&failed) || no_such_name(&failed) => Step::Stale,
            Err(failed) => Step::Failed(failed),
        }
    }

    /// Fetch `link` (active) for `character` (its roster row) and store the
    /// result. The caller holds the character's limiter slot.
    pub(super) async fn fetch(
        &self,
        link: &Link,
        character: &Character,
        mut batch: Option<&mut Batch>,
    ) -> Result<FetchState, StoreError> {
        let id = link.character_id.clone();
        let now = self.now();
        let previous = self.store.snapshot(id.clone(), now).await?;
        let (mut ocid, mut ocid_at, mut resolved) = match previous {
            Some(snapshot)
                if snapshot.ocid_fetched_at > now - OCID_RENEW_AFTER
                    && snapshot.ocid_fetched_at <= now + FUTURE_SKEW =>
            {
                (snapshot.ocid, snapshot.ocid_fetched_at, false)
            }
            _ => match self.resolve(&character.ign, &mut batch).await {
                Step::Ok(ocid) => (ocid, self.now(), true),
                Step::Stale => return self.make_stale(link).await,
                Step::Failed(failed) => return self.failed(&id, failed).await,
            },
        };
        let basic = loop {
            match self
                .call("character/basic", batch.as_deref_mut(), |client| {
                    client.basic(&ocid)
                })
                .await
            {
                Ok(basic) => break basic,
                // The ocid changed: resolve it again once, never in a loop.
                Err(failed) if invalid_identifier(&failed) && !resolved => {
                    match self.resolve(&character.ign, &mut batch).await {
                        Step::Ok(fresh) => (ocid, ocid_at, resolved) = (fresh, self.now(), true),
                        Step::Stale => return self.make_stale(link).await,
                        Step::Failed(failed) => return self.failed(&id, failed).await,
                    }
                }
                Err(failed) if invalid_identifier(&failed) => return self.make_stale(link).await,
                Err(failed) => return self.failed(&id, failed).await,
            }
        };
        // Another name for this ocid: not trusted as the roster character.
        if basic
            .character_name
            .as_deref()
            .is_some_and(|name| ign_key(name.trim()) != link.ign_key)
        {
            return self.make_stale(link).await;
        }
        let combat_power = match self
            .call("character/stat", batch, |client| client.combat_power(&ocid))
            .await
        {
            Ok(power) => power,
            Err(failed) if invalid_identifier(&failed) => return self.make_stale(link).await,
            Err(failed) => return self.failed(&id, failed).await,
        };
        let snapshot = facts(&id, ocid, ocid_at, basic, combat_power, self.now());
        if !self.store.put_snapshot(snapshot).await? {
            return Ok(FetchState::Unlinked);
        }
        self.store
            .put_refresh_state(RefreshState {
                character_id: id,
                next_attempt_at: None,
                last_error_code: None,
            })
            .await?;
        Ok(FetchState::Stored)
    }

    async fn make_stale(&self, link: &Link) -> Result<FetchState, StoreError> {
        let written = self
            .store
            .put_link(link.clone().with_status(LinkStatus::Stale), link.version)
            .await?;
        log("INFO", "chardata_link_stale", json!({}));
        Ok(match written {
            LinkPut::Written => FetchState::Stale,
            LinkPut::Stale(_) => FetchState::Unlinked,
        })
    }

    /// Record why a fetch stopped and when the character is tried again.
    async fn failed(&self, id: &str, failed: CallFailed) -> Result<FetchState, StoreError> {
        let now = self.now();
        let retry_at = match &failed {
            CallFailed::Held(until) => Some(*until),
            CallFailed::Failed(error) => match error.failure() {
                Failure::Preparing => Some(now + PREPARING_RETRY),
                Failure::Maintenance => self.holds().maintenance_until,
                // 429 (tomorrow's batch), a bug (logged) or key refused.
                _ => None,
            },
            _ => None,
        };
        let code = failed.label();
        self.store
            .put_refresh_state(RefreshState {
                character_id: id.to_owned(),
                next_attempt_at: retry_at,
                last_error_code: Some(code.clone()),
            })
            .await?;
        Ok(FetchState::Failed { code, retry_at })
    }

    /// The active link of an active roster character that still matches
    /// it: refreshable.
    pub(super) async fn refreshable(
        &self,
        character_id: &str,
    ) -> Result<Option<(Link, Character)>, StoreError> {
        // Fails closed like every read: matching identity and current consent.
        let Some((link, true)) = self.current_link(character_id).await? else {
            return Ok(None);
        };
        if link.status != LinkStatus::Active {
            return Ok(None);
        }
        let character = self.roster.character(character_id.to_owned()).await?;
        Ok(character.map(|character| (link, character)))
    }

    /// `POST /api/admin/characters/{id}/refresh`: once per 24 hours, as the
    /// batch counts it.
    pub async fn refresh_now(&self, character_id: &str) -> Result<RefreshOutcome, StoreError> {
        if !self.connected() {
            return Ok(RefreshOutcome::NotConnected);
        }
        let _slot = self.limiter.character(character_id).await;
        let Some((link, character)) = self.refreshable(character_id).await? else {
            return Ok(RefreshOutcome::NotLinked);
        };
        let now = self.now();
        if let Some(snapshot) = self.store.snapshot(character_id.to_owned(), now).await?
            && snapshot.fetched_at > now - REFRESH_CAP
        {
            return Ok(RefreshOutcome::TooSoon(snapshot.fetched_at + REFRESH_CAP));
        }
        if let Some(held) = self.held(now) {
            return Ok(RefreshOutcome::Unavailable(held.label()));
        }
        Ok(RefreshOutcome::Done(
            self.fetch(&link, &character, None).await?,
        ))
    }

    /// The daily batch: every refreshable link whose data is at least 20
    /// hours old (or missing) and not waiting for a retry. Stops on a
    /// second 429, a refused key or a spent budget.
    pub async fn refresh_due(&self) -> BatchReport {
        let at = self.now();
        let report = self.run_batch(at, false).await;
        log(
            "INFO",
            "chardata_batch",
            json!({
                "refreshed": report.refreshed,
                "failed": report.failed,
                "stopped": report.stopped,
            }),
        );
        self.holds().last_batch = Some(report.clone());
        report
    }

    /// Characters whose retry time has come (after `OPENAPI00009` or
    /// maintenance), outside the daily batch.
    pub async fn retry_due(&self) {
        if self.connected() {
            let _ = self.run_batch(self.now(), true).await;
        }
    }

    /// The earliest retry time stored, for the task's next wake.
    pub async fn next_retry(&self) -> Option<DateTime<Utc>> {
        let states = self.store.refresh_states().await.ok()?;
        states
            .iter()
            .filter_map(|state| state.next_attempt_at)
            .min()
    }

    async fn run_batch(&self, at: DateTime<Utc>, retries_only: bool) -> BatchReport {
        let mut report = BatchReport {
            at,
            refreshed: 0,
            failed: 0,
            stopped: None,
        };
        if !self.connected() {
            report.stopped = Some("no_key".into());
            return report;
        }
        let (links, states) = match (self.store.links().await, self.store.refresh_states().await) {
            (Ok(links), Ok(states)) => (links, states),
            _ => {
                report.stopped = Some("store".into());
                return report;
            }
        };
        let mut batch = Batch::default();
        for link in links
            .iter()
            .filter(|link| link.status == LinkStatus::Active)
        {
            let id = &link.character_id;
            let retry = states
                .iter()
                .find(|state| &state.character_id == id)
                .and_then(|state| state.next_attempt_at);
            let due = match (retry, retries_only) {
                (Some(retry), _) if retry > at => false,
                (Some(_), true) => true,
                (None, true) => false,
                (_, false) => match self.store.snapshot(id.clone(), at).await {
                    Ok(snapshot) => snapshot.is_none_or(|data| data.fetched_at <= at - REFRESH_AGE),
                    Err(_) => false,
                },
            };
            if !due {
                continue;
            }
            let _slot = self.limiter.character(id).await;
            let Ok(Some((link, character))) = self.refreshable(id).await else {
                continue;
            };
            match self.fetch(&link, &character, Some(&mut batch)).await {
                Ok(FetchState::Stored) => report.refreshed += 1,
                Ok(FetchState::Failed { code, .. }) => {
                    report.failed += 1;
                    // A second 429, a spent budget or a process hold ends it.
                    let stop = if batch.rate_limited >= 2 {
                        Some("OPENAPI00007".to_owned())
                    } else if code == "budget" {
                        Some(code)
                    } else {
                        self.held(self.now()).map(|held| held.label())
                    };
                    if stop.is_some() {
                        report.stopped = stop;
                        break;
                    }
                }
                Ok(FetchState::Stale | FetchState::Unlinked) => {}
                Err(_) => report.failed += 1,
            }
        }
        report
    }
}
