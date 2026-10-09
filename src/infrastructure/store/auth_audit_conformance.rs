//! The sign-in audit log every store must keep: rows round trip with their
//! assigned order, filters combine and page newest first, appends drop rows
//! past retention, and malformed rows are refused.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::auth_audit::{
    AUDIT_RETENTION, AuditFilter, AuditKind, AuditRealm, AuditRow, AuthAuditStore,
};

pub async fn run_suite<S: AuthAuditStore>(make: impl AsyncFn() -> S) {
    rows_round_trip_in_append_order(make().await).await;
    filters_combine_and_page_newest_first(make().await).await;
    appends_drop_rows_past_retention(make().await).await;
    malformed_rows_are_refused(make().await).await;
}

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

fn row(at: DateTime<Utc>, realm: AuditRealm, event: AuditKind, actor: &str) -> AuditRow {
    AuditRow {
        seq: 0,
        at,
        realm,
        event,
        actor: Some(actor.into()),
        method: Some("discord".into()),
        reason: None,
        request: None,
        client: None,
        device: None,
        request_id: "req".into(),
    }
}

fn all() -> AuditFilter {
    AuditFilter {
        limit: 100,
        ..AuditFilter::default()
    }
}

async fn rows_round_trip_in_append_order<S: AuthAuditStore>(store: S) {
    let mut first = row(
        at(9, 1),
        AuditRealm::Admin,
        AuditKind::LoginSucceeded,
        "discord:1",
    );
    first.client = Some("100.64.0.7".into());
    first.device = Some("Firefox on macOS".into());
    let mut second = row(at(9, 1), AuditRealm::Member, AuditKind::LoginRefused, "2");
    second.reason = Some("not_eligible".into());
    second.client = Some("a".repeat(64));
    let one = store.append_audit(first.clone()).await.expect("append");
    let two = store.append_audit(second.clone()).await.expect("append");
    assert!(two > one, "audit: seq grows with each append");
    first.seq = one;
    second.seq = two;
    assert_eq!(
        store.audit_page(&all()).await.unwrap(),
        vec![second, first],
        "audit: rows read back newest first, same instant ordered by seq"
    );
}

async fn filters_combine_and_page_newest_first<S: AuthAuditStore>(store: S) {
    let rows = [
        (
            at(1, 0),
            AuditRealm::Admin,
            AuditKind::LoginSucceeded,
            "discord:1",
        ),
        (
            at(2, 0),
            AuditRealm::Member,
            AuditKind::LoginSucceeded,
            "discord:2",
        ),
        (
            at(3, 0),
            AuditRealm::Member,
            AuditKind::SessionEnded,
            "discord:2",
        ),
        (
            at(4, 0),
            AuditRealm::Admin,
            AuditKind::BreakGlassUsed,
            "token",
        ),
        (
            at(5, 0),
            AuditRealm::Member,
            AuditKind::LoginSucceeded,
            "discord:3",
        ),
    ];
    let mut seqs = Vec::new();
    for (when, realm, event, actor) in rows {
        seqs.push(
            store
                .append_audit(row(when, realm, event, actor))
                .await
                .unwrap(),
        );
    }
    let store = &store;
    let page = |filter: AuditFilter| async move {
        store
            .audit_page(&filter)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.seq)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        page(AuditFilter {
            realm: Some(AuditRealm::Member),
            event: Some(AuditKind::LoginSucceeded),
            ..all()
        })
        .await,
        vec![seqs[4], seqs[1]],
        "audit: realm and event combine"
    );
    assert_eq!(
        page(AuditFilter {
            actor: Some("discord:2".into()),
            ..all()
        })
        .await,
        vec![seqs[2], seqs[1]],
        "audit: actor is exact"
    );
    assert_eq!(
        page(AuditFilter {
            from: Some(at(2, 0)),
            to: Some(at(4, 0)),
            ..all()
        })
        .await,
        vec![seqs[2], seqs[1]],
        "audit: from is inclusive, to exclusive"
    );
    let first = page(AuditFilter { limit: 2, ..all() }).await;
    assert_eq!(first, vec![seqs[4], seqs[3]], "audit: limit pages");
    assert_eq!(
        page(AuditFilter {
            limit: 2,
            before_seq: Some(seqs[3]),
            ..all()
        })
        .await,
        vec![seqs[2], seqs[1]],
        "audit: the cursor continues below the last seq"
    );
}

async fn appends_drop_rows_past_retention<S: AuthAuditStore>(store: S) {
    let now = at(20, 0);
    let cutoff = now - AUDIT_RETENTION;
    for when in [cutoff - TimeDelta::seconds(1), cutoff] {
        store
            .append_audit(row(when, AuditRealm::Admin, AuditKind::LoginSucceeded, "a"))
            .await
            .unwrap();
    }
    store
        .append_audit(row(now, AuditRealm::Admin, AuditKind::LoginSucceeded, "b"))
        .await
        .unwrap();
    let kept: Vec<DateTime<Utc>> = store
        .audit_page(&all())
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.at)
        .collect();
    assert_eq!(
        kept,
        vec![now, cutoff],
        "audit: rows strictly older than retention are dropped on append"
    );
}

async fn malformed_rows_are_refused<S: AuthAuditStore>(store: S) {
    let good = row(at(9, 1), AuditRealm::Admin, AuditKind::RateLimited, "x");
    let cases = [
        AuditRow {
            actor: Some(String::new()),
            ..good.clone()
        },
        AuditRow {
            device: Some("d".repeat(65)),
            ..good.clone()
        },
        AuditRow {
            client: Some("c".repeat(65)),
            ..good.clone()
        },
        AuditRow {
            request_id: "r".repeat(129),
            ..good.clone()
        },
    ];
    for bad in cases {
        assert!(
            matches!(
                store.append_audit(bad.clone()).await,
                Err(StoreError::Constraint(_))
            ),
            "audit: {bad:?} is refused"
        );
    }
    assert!(
        store.audit_page(&all()).await.unwrap().is_empty(),
        "audit: a refused row writes nothing"
    );
}
