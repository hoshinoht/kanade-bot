//! In-memory [`CharDataStore`] (test support): one lock over every table, so
//! each call is atomic like the SQLite file's transactions.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};

use crate::domain::chardata::{
    CALL_WINDOW, CharDataStore, FUTURE_SKEW, Link, LinkPut, LinkStatus, Post, PostState,
    RefreshState, Snapshot, StoreFuture,
};
use crate::domain::scheduler::StoreError;

#[derive(Default)]
struct Tables {
    links: BTreeMap<String, Link>,
    snapshots: BTreeMap<String, Snapshot>,
    refresh: BTreeMap<String, RefreshState>,
    calls: Vec<DateTime<Utc>>,
    /// In creation order.
    posts: Vec<Post>,
}

#[derive(Default)]
pub struct MemoryCharData {
    tables: Mutex<Tables>,
    /// Test hook: `unlink` fails as a backend error.
    fail_unlinks: std::sync::atomic::AtomicBool,
    /// Test hook: `put_link` fails as a backend error.
    fail_put_links: std::sync::atomic::AtomicBool,
}

impl MemoryCharData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test hook: every `unlink` fails (nothing written) while `on`.
    pub fn fail_unlinks(&self, on: bool) {
        self.fail_unlinks
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Test hook: every `put_link` fails (nothing written) while `on`.
    pub fn fail_put_links(&self, on: bool) {
        self.fail_put_links
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    fn tables(&self) -> MutexGuard<'_, Tables> {
        self.tables.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn ready<'a, T: Send + 'a>(value: Result<T, StoreError>) -> StoreFuture<'a, T> {
    Box::pin(std::future::ready(value))
}

impl CharDataStore for MemoryCharData {
    fn link(&self, character_id: String) -> StoreFuture<'_, Option<Link>> {
        ready(Ok(self.tables().links.get(&character_id).cloned()))
    }

    fn links(&self) -> StoreFuture<'_, Vec<Link>> {
        ready(Ok(self.tables().links.values().cloned().collect()))
    }

    fn put_link(&self, link: Link, expected: u32) -> StoreFuture<'_, LinkPut> {
        if self
            .fail_put_links
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return ready(Err(StoreError::Backend(
                "put_link failed (test hook)".into(),
            )));
        }
        let result = (|| {
            link.check()?;
            let mut tables = self.tables();
            let stored = tables.links.get(&link.character_id);
            if stored.map_or(0, |stored| stored.version) != expected {
                return Ok(LinkPut::Stale(stored.cloned()));
            }
            if link.version != expected + 1 {
                return Err(StoreError::Constraint("link takes the next version".into()));
            }
            if link.status != LinkStatus::Active {
                tables.snapshots.remove(&link.character_id);
                tables.refresh.remove(&link.character_id);
            }
            tables.links.insert(link.character_id.clone(), link);
            Ok(LinkPut::Written)
        })();
        ready(result)
    }

    fn unlink(&self, character_id: String, at: DateTime<Utc>) -> StoreFuture<'_, Option<Link>> {
        if self.fail_unlinks.load(std::sync::atomic::Ordering::SeqCst) {
            return ready(Err(StoreError::Backend("unlink failed (test hook)".into())));
        }
        let mut tables = self.tables();
        let removed = tables.links.remove(&character_id);
        if removed.is_some() {
            tables.snapshots.remove(&character_id);
            tables.refresh.remove(&character_id);
        }
        for post in &mut tables.posts {
            if post.character_id == character_id && post.strip_due_at > at {
                post.strip_due_at = at;
            }
        }
        ready(Ok(removed))
    }

    fn snapshot(
        &self,
        character_id: String,
        now: DateTime<Utc>,
    ) -> StoreFuture<'_, Option<Snapshot>> {
        let found = self
            .tables()
            .snapshots
            .get(&character_id)
            .filter(|snapshot| snapshot.is_fresh(now))
            .cloned();
        ready(Ok(found))
    }

    fn put_snapshot(&self, snapshot: Snapshot) -> StoreFuture<'_, bool> {
        let result = (|| {
            snapshot.check()?;
            let mut tables = self.tables();
            let active = tables
                .links
                .get(&snapshot.character_id)
                .is_some_and(|link| link.status == LinkStatus::Active);
            if active {
                tables
                    .snapshots
                    .insert(snapshot.character_id.clone(), snapshot);
            }
            Ok(active)
        })();
        ready(result)
    }

    fn sweep_snapshots(&self, now: DateTime<Utc>) -> StoreFuture<'_, u64> {
        let mut tables = self.tables();
        let before = tables.snapshots.len();
        tables.snapshots.retain(|_, snapshot| {
            now < snapshot.expires_at
                && snapshot.fetched_at <= now + FUTURE_SKEW
                && snapshot.ocid_fetched_at <= now + FUTURE_SKEW
        });
        ready(Ok((before - tables.snapshots.len()) as u64))
    }

    fn refresh_states(&self) -> StoreFuture<'_, Vec<RefreshState>> {
        ready(Ok(self.tables().refresh.values().cloned().collect()))
    }

    fn put_refresh_state(&self, state: RefreshState) -> StoreFuture<'_, bool> {
        let mut tables = self.tables();
        let linked = tables.links.contains_key(&state.character_id);
        if linked {
            tables.refresh.insert(state.character_id.clone(), state);
        }
        ready(Ok(linked))
    }

    fn reserve_call(&self, at: DateTime<Utc>, cap: u32) -> StoreFuture<'_, bool> {
        let mut tables = self.tables();
        let used = tables
            .calls
            .iter()
            .filter(|call| **call > at - CALL_WINDOW)
            .count();
        let free = used < cap as usize;
        if free {
            tables.calls.push(at);
        }
        ready(Ok(free))
    }

    fn calls_since(&self, since: DateTime<Utc>) -> StoreFuture<'_, u32> {
        let count = self
            .tables()
            .calls
            .iter()
            .filter(|call| **call > since)
            .count();
        ready(Ok(u32::try_from(count).unwrap_or(u32::MAX)))
    }

    fn prune_calls(&self, before: DateTime<Utc>) -> StoreFuture<'_, u64> {
        let mut tables = self.tables();
        let count = tables.calls.len();
        tables.calls.retain(|call| *call > before);
        ready(Ok((count - tables.calls.len()) as u64))
    }

    fn create_post(&self, post: Post) -> StoreFuture<'_, bool> {
        let result = (|| {
            post.check()?;
            let mut tables = self.tables();
            if tables
                .posts
                .iter()
                .any(|stored| stored.reference == post.reference)
            {
                return Ok(false);
            }
            tables.posts.push(post);
            Ok(true)
        })();
        ready(result)
    }

    fn posts(&self) -> StoreFuture<'_, Vec<Post>> {
        let mut posts = self.tables().posts.clone();
        posts.sort_by(|a, b| (a.posted_at, &a.reference).cmp(&(b.posted_at, &b.reference)));
        ready(Ok(posts))
    }

    fn set_post_message(&self, reference: String, message_id: String) -> StoreFuture<'_, bool> {
        let mut tables = self.tables();
        let found = tables
            .posts
            .iter_mut()
            .find(|post| post.reference == reference);
        let set = found.is_some();
        if let Some(post) = found {
            post.message_id = Some(message_id);
        }
        ready(Ok(set))
    }

    fn set_post_state(&self, reference: String, state: PostState) -> StoreFuture<'_, bool> {
        let mut tables = self.tables();
        let found = tables
            .posts
            .iter_mut()
            .find(|post| post.reference == reference);
        let set = found.is_some();
        if let Some(post) = found {
            post.state = state;
        }
        ready(Ok(set))
    }

    fn delete_post(&self, reference: String) -> StoreFuture<'_, bool> {
        let mut tables = self.tables();
        let before = tables.posts.len();
        tables.posts.retain(|post| post.reference != reference);
        ready(Ok(tables.posts.len() != before))
    }
}
