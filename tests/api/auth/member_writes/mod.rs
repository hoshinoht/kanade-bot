//! Member writes on the public origin (`member-writes-contract.md`): answers
//! and moves of the member's own runs, the run deep link, and requests, over
//! the admin reads' seeded store, writer and pinned clock (Tue 29 Sep 2026
//! 12:00 in Kuala Lumpur; boss weeks reset Thursday). The seeded members
//! 1001 Alice, 1002 Bob and 1004 Dan have the bossing role; 1003 Cara does not.

mod requests;
mod runs;

use std::collections::BTreeSet;

use chrono::{DateTime, TimeZone, Utc};
use kanade::domain::{
    history::{Actor, ChangeHistory, ChangeMeta, ChangeRecord, Origin, Surface},
    schedule::{Change, ChangeSet, Run, RunSource, RunStatus},
    scheduler::{ScheduleStore, Scope},
};
use serde_json::Value;

use super::member_support::{Browser, MemberHarness, Options, PUB_ORIGIN, discord_user};
use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{PUBLIC_HOST, Reply, send},
};

const IP: &str = "198.51.100.10";
pub(super) const ALICE: u64 = 1001;
pub(super) const BOB: u64 = 1002;
pub(super) const CARA: u64 = 1003;
pub(super) const DAN: u64 = 1004;

pub(super) fn utc(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

/// Thursday 24 Sep 00:00 KL: this boss week.
pub(super) fn this_week() -> DateTime<Utc> {
    utc(9, 23, 16, 0)
}

pub(super) fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

pub(super) fn refused(reply: &Reply) -> (u16, String) {
    assert_valid("error.json#/$defs/ApiError", "refusal", &reply.json());
    (reply.status, reply.api_error())
}

/// A run in the `star` channel, from no weekly timing.
pub(super) fn run(
    id: &str,
    week: DateTime<Utc>,
    at: DateTime<Utc>,
    party: &[u64],
    status: RunStatus,
) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some("star".into()),
        week_start: week,
        datetime: at,
        bosses: vec!["HMaleficStar".into()],
        participants: party.iter().map(u64::to_string).collect(),
        status,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

/// The public origin over `reads`' state, with 1001–1004 eligible to sign in.
pub(super) struct Portal {
    pub reads: Reads,
    pub harness: MemberHarness,
}

impl Portal {
    pub async fn over(reads: Reads) -> Self {
        let harness = MemberHarness::with(Options {
            state: reads.site.state.clone(),
            boss_dir: reads.site.boss_dir.clone(),
            ..Options::default()
        })
        .await;
        for id in [ALICE, BOB, CARA, DAN] {
            harness.roster(id, true).await;
        }
        Self { reads, harness }
    }

    pub async fn new() -> Self {
        Self::over(Reads::new().await).await
    }

    pub async fn sign_in(&self, id: u64) -> Browser {
        self.harness
            .sign_in_from(discord_user(id, "Member"), IP)
            .await
    }

    /// Rows written behind the API, as an admin's seed.
    pub async fn seed(&self, changes: Vec<Change>) {
        let revision = self.reads.store.load(&Scope::All).await.unwrap().revision;
        self.reads
            .store
            .commit(
                revision,
                ChangeSet { changes },
                ChangeMeta {
                    origin: Origin::new(Actor::admin("seed"), Surface::AdminPortal),
                    at: utc(9, 28, 0, 0),
                    notices: Vec::new(),
                    refs: Vec::new(),
                    request_digest: None,
                    expect: Default::default(),
                    outbox: Vec::new(),
                },
            )
            .await
            .unwrap();
    }

    pub async fn version(&self) -> u64 {
        self.reads.version().await
    }

    /// The newest history record.
    pub async fn head(&self) -> ChangeRecord {
        let seq = self.version().await;
        self.reads.store.load_change(seq).await.unwrap().unwrap()
    }

    pub async fn get(&self, browser: Option<&Browser>, path: &str) -> Reply {
        let cookie = browser.map(Browser::cookie);
        let mut headers = vec![("CF-Connecting-IP", IP)];
        if let Some((name, value)) = &cookie {
            headers.push((name, value));
        }
        self.harness.get(path, &headers).await
    }

    /// A signed-in JSON read: 200, `no-store`, valid against `target`.
    pub async fn read(&self, browser: &Browser, path: &str, target: &str) -> Value {
        let reply = self.get(Some(browser), path).await;
        assert_eq!(reply.status, 200, "{path}: {}", reply.text());
        assert_eq!(reply.header("cache-control"), Some("no-store"), "{path}");
        let value = reply.json();
        assert_valid(target, path, &value);
        value
    }

    /// A request with exactly `headers` besides the cookie and client address.
    pub async fn send(
        &self,
        browser: Option<&Browser>,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Reply {
        let cookie = browser.map(Browser::cookie);
        let mut all = vec![("CF-Connecting-IP", IP)];
        if let Some((name, value)) = &cookie {
            all.push((name, value));
        }
        all.extend_from_slice(headers);
        if body.is_none() {
            all.push(("Content-Length", "0"));
        }
        send(self.harness.public, method, PUBLIC_HOST, path, &all, body).await
    }

    /// A member write with every marker: same origin, CSRF token and `key`.
    pub async fn write(
        &self,
        browser: &Browser,
        method: &str,
        path: &str,
        key: &str,
        body: Option<&Value>,
    ) -> Reply {
        let body = body.map(Value::to_string);
        self.send(
            Some(browser),
            method,
            path,
            &[
                PUB_ORIGIN,
                ("X-Kanade-CSRF", &browser.csrf),
                ("Idempotency-Key", key),
            ],
            body.as_deref(),
        )
        .await
    }
}
