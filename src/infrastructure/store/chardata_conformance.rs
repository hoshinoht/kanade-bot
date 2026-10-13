//! The character-data file every store must keep: versioned links, a link
//! written pending or stale dropping its data, unlink deleting the link,
//! snapshot and refresh state together and making posts due at once,
//! snapshots only for active links and never read expired or stamped in the
//! future, the rolling call budget, and tracked posts. Invented data only.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::chardata::{
    CharDataStore, ConsentSurface, Link, LinkPut, LinkStatus, NOTICE_VERSION, Post, PostState,
    RefreshState, STRIP_AFTER, Snapshot,
};
use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

pub async fn run_suite<S: CharDataStore>(make: impl AsyncFn() -> S) {
    links_are_versioned(make().await).await;
    pending_and_stale_links_hold_no_data(make().await).await;
    unlink_deletes_everything_and_makes_posts_due(make().await).await;
    snapshots_need_an_active_link_and_are_never_read_stale(make().await).await;
    the_call_budget_rolls_over_24_hours(make().await).await;
    posts_are_tracked_once(make().await).await;
    malformed_rows_are_refused(make().await).await;
}

pub fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

/// An active link on Wisteria (character `c1`) for member 1001.
pub fn active(character: &str) -> Link {
    Link {
        character_id: character.into(),
        member_id: "1001".into(),
        ign_key: "wisteria".into(),
        region: "msea".into(),
        status: LinkStatus::Active,
        requested_by: Actor::member("1001"),
        requested_at: at(1, 0),
        consent_version: Some(NOTICE_VERSION),
        consented_at: Some(at(1, 0)),
        linked_at: Some(at(1, 0)),
        consent_surface: Some(ConsentSurface::Discord),
        version: 1,
    }
}

/// Invented Nexon data fetched at `fetched`.
pub fn snapshot(character: &str, fetched: DateTime<Utc>) -> Snapshot {
    Snapshot {
        character_id: character.into(),
        ocid: "ocid-invented-f00d".into(),
        ocid_fetched_at: fetched,
        character_name: Some("Wisteria".into()),
        world_name: Some("Aquila".into()),
        character_class: Some("Bishop".into()),
        level: Some(271),
        combat_power: Some(31_415_926),
        image_url: Some("https://images.invalid/character/wisteria.png".into()),
        data_date: chrono::NaiveDate::from_ymd_opt(2026, 10, 1),
        fetched_at: fetched,
        expires_at: Snapshot::expiry(fetched, fetched),
    }
}

async fn links_are_versioned<S: CharDataStore>(store: S) {
    let link = active("c1");
    assert_eq!(
        store.put_link(link.clone(), 0).await.unwrap(),
        LinkPut::Written
    );
    assert_eq!(
        store.put_link(link.clone(), 0).await.unwrap(),
        LinkPut::Stale(Some(link.clone())),
        "a second create is stale"
    );
    assert_eq!(store.link("c1".into()).await.unwrap(), Some(link.clone()));
    let stale = link.clone().with_status(LinkStatus::Stale);
    assert_eq!(
        store.put_link(stale.clone(), 1).await.unwrap(),
        LinkPut::Written
    );
    assert_eq!(
        store
            .put_link(link.clone().with_status(LinkStatus::Active), 1)
            .await
            .unwrap(),
        LinkPut::Stale(Some(stale.clone()))
    );
    assert!(
        matches!(
            store.put_link(stale.clone(), 2).await,
            Err(StoreError::Constraint(_))
        ),
        "a replacement takes the next version"
    );
    let other = Link {
        character_id: "c2".into(),
        ..active("c2")
    };
    store.put_link(other.clone(), 0).await.unwrap();
    assert_eq!(store.links().await.unwrap(), [stale, other]);
}

async fn pending_and_stale_links_hold_no_data<S: CharDataStore>(store: S) {
    for status in [LinkStatus::Pending, LinkStatus::Stale] {
        let link = active("c1");
        store.unlink("c1".into(), at(1, 0)).await.unwrap();
        store.put_link(link.clone(), 0).await.unwrap();
        assert!(store.put_snapshot(snapshot("c1", at(2, 0))).await.unwrap());
        assert!(
            store
                .put_refresh_state(RefreshState {
                    character_id: "c1".into(),
                    next_attempt_at: Some(at(2, 1)),
                    last_error_code: Some("OPENAPI00009".into()),
                })
                .await
                .unwrap()
        );
        let mut next = link.with_status(status);
        if status == LinkStatus::Pending {
            next.consent_version = None;
            next.consented_at = None;
            next.linked_at = None;
            next.consent_surface = None;
        }
        store.put_link(next, 1).await.unwrap();
        assert_eq!(store.snapshot("c1".into(), at(2, 1)).await.unwrap(), None);
        assert!(store.refresh_states().await.unwrap().is_empty());
        assert!(
            !store.put_snapshot(snapshot("c1", at(2, 0))).await.unwrap(),
            "no data while {status:?}"
        );
    }
}

async fn unlink_deletes_everything_and_makes_posts_due<S: CharDataStore>(store: S) {
    store.put_link(active("c1"), 0).await.unwrap();
    store.put_snapshot(snapshot("c1", at(2, 0))).await.unwrap();
    store
        .put_refresh_state(RefreshState {
            character_id: "c1".into(),
            next_attempt_at: None,
            last_error_code: Some("OPENAPI00004".into()),
        })
        .await
        .unwrap();
    let post = Post::new("a1b2c3".into(), "900".into(), "c1".into(), at(2, 0));
    assert_eq!(post.strip_due_at, at(2, 0) + STRIP_AFTER);
    store.create_post(post).await.unwrap();
    let later = Post::new("d4e5f6".into(), "900".into(), "c2".into(), at(2, 0));
    store.create_post(later).await.unwrap();

    let removed = store.unlink("c1".into(), at(3, 0)).await.unwrap();
    assert_eq!(removed, Some(active("c1")));
    assert_eq!(store.link("c1".into()).await.unwrap(), None);
    assert_eq!(store.snapshot("c1".into(), at(3, 0)).await.unwrap(), None);
    assert!(store.refresh_states().await.unwrap().is_empty());
    let posts = store.posts().await.unwrap();
    assert_eq!(posts.len(), 2, "posts stay until stripped");
    assert_eq!(posts[0].strip_due_at, at(3, 0), "due at once");
    assert!(posts[0].is_due(at(3, 0)));
    assert_eq!(
        posts[1].strip_due_at,
        at(2, 0) + STRIP_AFTER,
        "other characters keep theirs"
    );
    // A link made again starts with no data.
    store.put_link(active("c1"), 0).await.unwrap();
    assert_eq!(store.snapshot("c1".into(), at(3, 0)).await.unwrap(), None);
    assert_eq!(store.unlink("c9".into(), at(3, 0)).await.unwrap(), None);
}

async fn snapshots_need_an_active_link_and_are_never_read_stale<S: CharDataStore>(store: S) {
    assert!(
        !store.put_snapshot(snapshot("c1", at(2, 0))).await.unwrap(),
        "no link"
    );
    store.put_link(active("c1"), 0).await.unwrap();
    let data = snapshot("c1", at(2, 0));
    assert!(store.put_snapshot(data.clone()).await.unwrap());
    assert_eq!(
        store.snapshot("c1".into(), at(3, 0)).await.unwrap(),
        Some(data.clone())
    );
    // A refresh replaces it.
    let newer = snapshot("c1", at(3, 0));
    assert!(store.put_snapshot(newer.clone()).await.unwrap());
    assert_eq!(
        store.snapshot("c1".into(), at(3, 1)).await.unwrap(),
        Some(newer.clone())
    );
    // Expired: refused before the sweeper runs, then deleted by it.
    let expired = newer.expires_at;
    assert_eq!(store.snapshot("c1".into(), expired).await.unwrap(), None);
    assert_eq!(
        store
            .snapshot("c1".into(), expired - TimeDelta::seconds(1))
            .await
            .unwrap(),
        Some(newer.clone())
    );
    // Stamped in the future (the wall clock stepped back two days).
    let back = at(3, 0) - TimeDelta::days(2);
    assert_eq!(store.snapshot("c1".into(), back).await.unwrap(), None);
    assert_eq!(store.sweep_snapshots(back).await.unwrap(), 1);
    assert_eq!(store.snapshot("c1".into(), at(3, 1)).await.unwrap(), None);
    // Inside the skew, a row is kept.
    store.put_snapshot(newer.clone()).await.unwrap();
    assert_eq!(
        store
            .sweep_snapshots(at(3, 0) - TimeDelta::minutes(4))
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.sweep_snapshots(expired).await.unwrap(), 1, "expired");
    // An ocid older than the data ends it sooner.
    let mut old_ocid = snapshot("c1", at(5, 0));
    old_ocid.ocid_fetched_at = at(4, 0);
    old_ocid.expires_at = Snapshot::expiry(old_ocid.fetched_at, old_ocid.ocid_fetched_at);
    assert_eq!(old_ocid.expires_at, at(4, 0) + TimeDelta::days(30));
    store.put_snapshot(old_ocid).await.unwrap();
    assert_eq!(
        store
            .snapshot("c1".into(), at(4, 0) + TimeDelta::days(30))
            .await
            .unwrap(),
        None
    );
}

async fn the_call_budget_rolls_over_24_hours<S: CharDataStore>(store: S) {
    for minute in 0..3 {
        assert!(
            store
                .reserve_call(at(2, 0) + TimeDelta::minutes(minute), 3)
                .await
                .unwrap()
        );
    }
    assert!(!store.reserve_call(at(2, 12), 3).await.unwrap(), "full");
    assert_eq!(store.calls_since(at(1, 0)).await.unwrap(), 3);
    // A day after the first call, it leaves the window.
    assert!(
        store
            .reserve_call(at(3, 0) + TimeDelta::seconds(1), 3)
            .await
            .unwrap()
    );
    assert_eq!(store.prune_calls(at(2, 0)).await.unwrap(), 1);
    assert_eq!(store.calls_since(at(1, 0)).await.unwrap(), 3);
}

async fn posts_are_tracked_once<S: CharDataStore>(store: S) {
    let post = Post::new("0a0b0c".into(), "900".into(), "c1".into(), at(2, 0));
    assert!(store.create_post(post.clone()).await.unwrap());
    assert!(!store.create_post(post.clone()).await.unwrap(), "ref taken");
    assert!(
        store
            .set_post_message("0a0b0c".into(), "5550".into())
            .await
            .unwrap()
    );
    assert!(
        store
            .set_post_state("0a0b0c".into(), PostState::Stuck)
            .await
            .unwrap()
    );
    let stored = store.posts().await.unwrap();
    assert_eq!(stored[0].message_id.as_deref(), Some("5550"));
    assert_eq!(stored[0].state, PostState::Stuck);
    assert!(!stored[0].is_overdue(at(2, 0) + TimeDelta::days(29)));
    assert!(stored[0].is_overdue(at(2, 0) + TimeDelta::days(30)));
    assert!(store.delete_post("0a0b0c".into()).await.unwrap());
    assert!(!store.delete_post("0a0b0c".into()).await.unwrap());
    assert!(
        !store
            .set_post_state("0a0b0c".into(), PostState::Live)
            .await
            .unwrap()
    );
}

async fn malformed_rows_are_refused<S: CharDataStore>(store: S) {
    let refused = |result: Result<_, StoreError>| matches!(result, Err(StoreError::Constraint(_)));
    let other_region = Link {
        region: "kms".into(),
        ..active("c1")
    };
    assert!(refused(store.put_link(other_region, 0).await.map(drop)));
    let unconsented = Link {
        consent_version: None,
        consented_at: None,
        ..active("c1")
    };
    assert!(refused(store.put_link(unconsented, 0).await.map(drop)));
    let pending_with_surface = Link {
        status: LinkStatus::Pending,
        consent_version: None,
        consented_at: None,
        linked_at: None,
        ..active("c1")
    };
    assert!(refused(
        store.put_link(pending_with_surface, 0).await.map(drop)
    ));
    store.put_link(active("c1"), 0).await.unwrap();
    let mut wrong_expiry = snapshot("c1", at(2, 0));
    wrong_expiry.expires_at = at(2, 0) + TimeDelta::days(60);
    assert!(refused(store.put_snapshot(wrong_expiry).await.map(drop)));
    let bad_ref = Post::new("XYZ".into(), "900".into(), "c1".into(), at(2, 0));
    assert!(refused(store.create_post(bad_ref).await.map(drop)));
}
