//! Ownership requests every store must keep: one open request per member
//! and timing, closed exactly once, listed oldest first while open.

use chrono::{DateTime, TimeZone, Utc};

use crate::domain::ownership::{
    OWNER_REQUEST_TTL, OwnerRequest, OwnerRequestStatus, OwnerRequestStore,
};
use crate::domain::scheduler::StoreError;

pub async fn run_suite<S: OwnerRequestStore>(make: impl AsyncFn() -> S) {
    requests_round_trip_and_list_open_oldest_first(make().await).await;
    one_open_request_per_member_and_timing(make().await).await;
    a_request_closes_exactly_once(make().await).await;
    only_posted_closed_messages_await_settling(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, hour, 0, 0).unwrap()
}

fn request(id: &str, fixed: &str, requester: &str, hour: u32) -> OwnerRequest {
    OwnerRequest::open(
        id.into(),
        fixed.into(),
        requester.into(),
        Some("700".into()),
        at(hour),
    )
}

async fn requests_round_trip_and_list_open_oldest_first<S: OwnerRequestStore>(store: S) {
    let later = request("b", "f-1", "22", 3);
    let first = request("a", "f-2", "11", 1);
    store.create_owner_request(later.clone()).await.unwrap();
    store.create_owner_request(first.clone()).await.unwrap();
    assert_eq!(first.expires_at - first.created_at, OWNER_REQUEST_TTL);
    assert_eq!(store.owner_request("a").await.unwrap(), Some(first.clone()));
    assert_eq!(store.owner_request("x").await.unwrap(), None);
    store
        .set_owner_request_message("a", "701", "9001")
        .await
        .unwrap();
    let open = store.open_owner_requests().await.unwrap();
    assert_eq!(
        open.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"],
        "owner requests: open ones oldest first"
    );
    assert_eq!(
        (open[0].channel_id.as_deref(), open[0].message_id.as_deref()),
        (Some("701"), Some("9001"))
    );
}

async fn one_open_request_per_member_and_timing<S: OwnerRequestStore>(store: S) {
    store
        .create_owner_request(request("a", "f-1", "11", 1))
        .await
        .unwrap();
    assert!(
        matches!(
            store
                .create_owner_request(request("b", "f-1", "11", 2))
                .await,
            Err(StoreError::Constraint(_))
        ),
        "owner requests: a second open one by the same member is refused"
    );
    // Another member, or the same member once theirs closed, may ask.
    store
        .create_owner_request(request("c", "f-1", "22", 2))
        .await
        .unwrap();
    store
        .close_owner_request("a", OwnerRequestStatus::Withdrawn, "member:11", at(3))
        .await
        .unwrap();
    store
        .create_owner_request(request("d", "f-1", "11", 4))
        .await
        .unwrap();
}

async fn a_request_closes_exactly_once<S: OwnerRequestStore>(store: S) {
    store
        .create_owner_request(request("a", "f-1", "11", 1))
        .await
        .unwrap();
    assert!(
        store
            .close_owner_request("a", OwnerRequestStatus::Accepted, "member:22", at(2))
            .await
            .unwrap()
    );
    assert!(
        !store
            .close_owner_request("a", OwnerRequestStatus::Declined, "member:33", at(3))
            .await
            .unwrap(),
        "owner requests: a decided request is never decided again"
    );
    let closed = store.owner_request("a").await.unwrap().unwrap();
    assert_eq!(
        (
            closed.status,
            closed.decided_by.as_deref(),
            closed.decided_at
        ),
        (OwnerRequestStatus::Accepted, Some("member:22"), Some(at(2)))
    );
    assert!(store.open_owner_requests().await.unwrap().is_empty());
    assert!(
        !store
            .close_owner_request(
                "missing",
                OwnerRequestStatus::Expired,
                "system:expiry",
                at(4)
            )
            .await
            .unwrap()
    );
}

async fn only_posted_closed_messages_await_settling<S: OwnerRequestStore>(store: S) {
    for (id, hour) in [("posted", 1), ("unposted", 2), ("open", 3)] {
        store
            .create_owner_request(request(id, &format!("f-{id}"), "11", hour))
            .await
            .unwrap();
    }
    for id in ["posted", "open"] {
        store
            .set_owner_request_message(id, "701", "9001")
            .await
            .unwrap();
    }
    for id in ["posted", "unposted"] {
        store
            .close_owner_request(id, OwnerRequestStatus::Expired, "system:expiry", at(5))
            .await
            .unwrap();
    }
    let ids = async || {
        store
            .unsettled_owner_requests()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids().await,
        ["posted"],
        "owner requests: only a closed request with a posted message awaits its edit"
    );
    store.settle_owner_request_message("posted").await.unwrap();
    assert!(ids().await.is_empty());
    assert!(
        store
            .owner_request("posted")
            .await
            .unwrap()
            .unwrap()
            .message_settled
    );
}
