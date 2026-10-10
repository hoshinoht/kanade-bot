//! `GET /api/admin/auth/tonight`, the signed-out sign-in strip, over the
//! seeded store of `reads.rs` (Tue 29 Sep 12:00 KL; `r-kalos` is Tue 22:00 with
//! Alice yes, Bob no and Dan waiting).

use serde_json::{Value, json};

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, request},
};

const PATH: &str = "/api/admin/auth/tonight";
const TONIGHT: &str = "week.json#/$defs/Tonight";

async fn signed_out(reads: &Reads) -> Reply {
    let reply = request(reads.admin, "GET", ADMIN_HOST, PATH, &[]).await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_valid(TONIGHT, PATH, &reply.json());
    reply
}

#[tokio::test]
async fn signed_out_sees_tonights_time_bosses_and_tally_only() {
    let reads = Reads::new().await;
    let reply = signed_out(&reads).await;
    assert_eq!(
        reply.json(),
        json!({"run": {"time": "22:00", "bosses": ["Gatekeeper Kalos"], "tally": {"on": 1, "total": 3}}})
    );
    // Nothing private: no member names or ids, answers, run or timing ids,
    // channel, party or version, whatever the casing.
    let text = reply.text().to_lowercase();
    for private in [
        "alice",
        "bob",
        "dan",
        "1001",
        "1002",
        "1004",
        "r-kalos",
        "f-kalos",
        "kalos-four",
        "channel",
        "party",
        "participants",
        "answer",
        "yes",
        "version",
        "run_id",
        "900",
    ] {
        assert!(!text.contains(private), "{private} in {text}");
    }
}

#[tokio::test]
async fn nothing_when_the_next_run_is_not_today() {
    let reads = Reads::new().await;
    let version = reads.version().await;
    let cancelled = reads
        .ok(
            "PATCH",
            "/api/admin/runs/r-kalos/status",
            json!({"status": "cancelled", "version": version}),
            "week.json#/$defs/RunResult",
        )
        .await;
    assert_eq!(cancelled["run"]["status"], "cancelled");
    // n-kalos (Tue 6 Oct) is next now; n-star has its own time.
    assert_eq!(signed_out(&reads).await.json(), json!({"run": Value::Null}));
}
