//! Expired public sessions are pruned at startup and then hourly, so the
//! rows of members who never come back do not wait for someone's next
//! sign-in (which also prunes). Stopped before the store closes.

use std::{sync::Arc, time::Duration};

use serde_json::json;
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    api::auth::member::MemberAuth, domain::scheduler::StoreError,
    infrastructure::store::web_sessions::SessionOrigin, runtime::logging,
};

pub(super) const PRUNE_EVERY: Duration = Duration::from_secs(60 * 60);

/// Delete the public sessions past their absolute expiry, idle past the
/// member policy or superseded with their grace over; admin rows stay.
pub(super) async fn prune(member: &MemberAuth) -> Result<u64, StoreError> {
    let now = member.now();
    member
        .sessions()
        .prune_sessions(SessionOrigin::Public, now, now - member.policy().idle)
        .await
}

/// The running prune loop.
pub(super) struct SessionPrune {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl SessionPrune {
    /// Prune now, then every `every`; `None` without a member realm.
    pub(super) fn start(member: Option<Arc<MemberAuth>>, every: Duration) -> Option<Self> {
        let member = member?;
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                match prune(&member).await {
                    Ok(0) => {}
                    Ok(count) => logging::event("INFO", "sessions_pruned", json!({"count": count})),
                    // Store error text may carry paths.
                    Err(_) => logging::event("WARN", "sessions_prune_failed", json!({})),
                }
                tokio::select! {
                    () = tokio::time::sleep(every) => {}
                    _ = stopped.changed() => return,
                }
            }
        });
        Some(Self { stop, task })
    }

    /// Stop the loop; a prune already running finishes first, so no write is
    /// cut off before the store closes.
    pub(super) async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use chrono::{DateTime, TimeDelta, TimeZone, Utc};

    use super::*;
    use crate::{
        api::auth::member::StoreEligibility,
        infrastructure::store::{
            MemoryScheduleStore,
            web_sessions::{LoginMethod, WebSession, WebSessionStore},
        },
    };

    fn at(minutes: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 11, 0, 0, 0).unwrap() + TimeDelta::minutes(minutes)
    }

    fn session(id: &str, origin: SessionOrigin, last_seen: i64, expires: i64) -> WebSession {
        WebSession {
            id_hash: format!("{id:0>64}"),
            origin,
            method: match origin {
                SessionOrigin::Public => LoginMethod::Discord,
                SessionOrigin::Admin => LoginMethod::Token,
            },
            subject: "1001".into(),
            display: "Member".into(),
            created_at: at(0),
            last_seen_at: at(last_seen),
            checked_at: at(last_seen),
            expires_at: at(expires),
            avatar_hash: None,
            device: None,
            client_tag: None,
            superseded_until: None,
        }
    }

    /// The memory store with an idle and an expired public session, a live
    /// one, and an admin row past both, read by a member realm whose clock
    /// reads `at(minute)`.
    async fn realm(minute: Arc<AtomicI64>) -> (Arc<MemoryScheduleStore>, Arc<MemberAuth>) {
        let store = Arc::new(MemoryScheduleStore::new());
        for row in [
            session("a", SessionOrigin::Public, 0, 480),
            session("b", SessionOrigin::Public, 50, 60),
            session("c", SessionOrigin::Public, 55, 480),
            session("d", SessionOrigin::Admin, 0, 60),
        ] {
            store.put_session(&row, None).await.unwrap();
        }
        let member = MemberAuth::new(
            store.clone(),
            Arc::new(StoreEligibility::new(store.clone())),
            Arc::new(|| true),
        )
        .with_clock(Arc::new(move || at(minute.load(Ordering::SeqCst))));
        (store, Arc::new(member))
    }

    async fn live(store: &MemoryScheduleStore, id: &str) -> bool {
        store
            .load_session(&format!("{id:0>64}"))
            .await
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn only_dead_public_sessions_are_pruned() {
        let (store, member) = realm(Arc::new(AtomicI64::new(61))).await;
        assert_eq!(prune(&member).await.unwrap(), 2, "idle and expired");
        assert!(!live(&store, "a").await, "idle past 30 minutes");
        assert!(!live(&store, "b").await, "past its absolute expiry");
        assert!(live(&store, "c").await, "still live");
        assert!(live(&store, "d").await, "admin rows are the admin realm's");
        assert_eq!(prune(&member).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_loop_prunes_at_start_and_on_each_period_until_stopped() {
        let minute = Arc::new(AtomicI64::new(31));
        let (store, member) = realm(minute.clone()).await;
        let running = SessionPrune::start(Some(member), Duration::from_millis(20)).unwrap();
        let gone = |id: &'static str| {
            let store = store.clone();
            async move {
                for _ in 0..500 {
                    if !live(&store, id).await {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                false
            }
        };
        assert!(gone("a").await, "pruned at startup");
        assert!(live(&store, "b").await, "not yet expired");
        minute.store(61, Ordering::SeqCst);
        assert!(gone("b").await, "pruned on a later period");
        running.stop().await;
        assert!(live(&store, "c").await);
        assert!(SessionPrune::start(None, PRUNE_EVERY).is_none());
    }
}
