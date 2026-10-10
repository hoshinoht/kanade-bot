//! `/fixed owner` and the ownership request's Accept/Decline buttons (user
//! decision 2026-10-10): only party members give or take a timing, the owner
//! or staff decide, and a pin follows every change.

use serde_json::json;

use kanade::domain::ownership::{OwnerRequestStatus, OwnerRequestStore};
use kanade::domain::scheduler::{ScheduleStore, Scope};

use super::{ALICE, BOB, DAN, F_KALOS, Slash, opt, sub, user_opt};

async fn owner_of(slash: &Slash) -> (String, bool) {
    let fixed = slash
        .store
        .load(&Scope::All)
        .await
        .unwrap()
        .fixed_runs
        .into_iter()
        .find(|fixed| fixed.id == F_KALOS)
        .unwrap();
    (fixed.owner().to_owned(), fixed.owner_pinned)
}

#[tokio::test]
async fn the_owner_hands_a_timing_to_another_party_member_only() {
    let slash = Slash::new().await;
    let hand = |to| sub("owner", json!([opt("id", F_KALOS), user_opt("to", to)]));
    assert_eq!(
        slash.run(BOB, "fixed", hand(DAN)).await,
        "❌ Only the timing's owner or an admin can do that."
    );
    assert_eq!(
        slash.run(ALICE, "fixed", hand(DAN)).await,
        "❌ Ownership only moves between members of the party."
    );
    slash
        .run(BOB, "fixed", sub("owner", json!([opt("id", F_KALOS)])))
        .await;
    assert_eq!(
        slash.run(ALICE, "fixed", hand(BOB)).await,
        "👑 <@1002> now owns weekly timing `#ffff0001`."
    );
    assert_eq!(owner_of(&slash).await, (BOB.to_string(), true));
    assert!(
        slash.store.open_owner_requests().await.unwrap().is_empty(),
        "a hand-off closes the timing's open requests"
    );
}

#[tokio::test]
async fn a_party_member_asks_and_the_owner_accepts_by_button() {
    let slash = Slash::new().await;
    let ask = sub("owner", json!([opt("id", F_KALOS)]));
    assert_eq!(
        slash.run(DAN, "fixed", ask.clone()).await,
        "❌ Ownership only moves between members of the party."
    );
    let reply = slash.run(BOB, "fixed", ask.clone()).await;
    assert!(
        reply.starts_with("📨 Asked <@1001> to hand you weekly timing `#ffff0001`."),
        "{reply}"
    );
    assert_eq!(
        slash.run(BOB, "fixed", ask).await,
        "❌ You already asked to own this timing."
    );
    let request = slash.store.open_owner_requests().await.unwrap().remove(0);
    let accept = format!("owner:accept:{}", request.id);
    assert_eq!(
        slash.press(BOB, &accept).await,
        "❌ Only the timing's owner or an admin can do that."
    );
    assert_eq!(
        slash.press(ALICE, &accept).await,
        "👑 <@1002> now owns weekly timing `#ffff0001`."
    );
    assert_eq!(owner_of(&slash).await, (BOB.to_string(), true));
    let decided = slash
        .store
        .owner_request(&request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (decided.status, decided.decided_by.as_deref()),
        (OwnerRequestStatus::Accepted, Some("member:1001"))
    );
    assert_eq!(
        slash
            .press(ALICE, &format!("owner:decline:{}", request.id))
            .await,
        "❌ That request was already answered."
    );
}

#[tokio::test]
async fn the_requester_s_own_decline_withdraws_it() {
    let slash = Slash::new().await;
    slash
        .run(BOB, "fixed", sub("owner", json!([opt("id", F_KALOS)])))
        .await;
    let request = slash.store.open_owner_requests().await.unwrap().remove(0);
    assert_eq!(
        slash
            .press(BOB, &format!("owner:decline:{}", request.id))
            .await,
        "↩️ Request withdrawn."
    );
    let closed = slash
        .store
        .owner_request(&request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closed.status, OwnerRequestStatus::Withdrawn);
    assert_eq!(owner_of(&slash).await, (ALICE.to_string(), false));
}

/// F1: naming the current owner is no replay. A former owner is refused
/// like any non-owner and the open ask stays open.
#[tokio::test]
async fn a_former_owner_naming_the_new_owner_changes_nothing() {
    let slash = Slash::new().await;
    let hand = |to| sub("owner", json!([opt("id", F_KALOS), user_opt("to", to)]));
    assert_eq!(
        slash.run(ALICE, "fixed", hand(BOB)).await,
        "👑 <@1002> now owns weekly timing `#ffff0001`."
    );
    slash
        .run(ALICE, "fixed", sub("owner", json!([opt("id", F_KALOS)])))
        .await;
    let open = slash.store.open_owner_requests().await.unwrap();
    assert_eq!(open.len(), 1, "Alice asked it back");
    assert_eq!(
        slash.run(ALICE, "fixed", hand(BOB)).await,
        "❌ Only the timing's owner or an admin can do that."
    );
    assert_eq!(owner_of(&slash).await, (BOB.to_string(), true));
    assert_eq!(
        slash.store.open_owner_requests().await.unwrap(),
        open,
        "nothing superseded"
    );
}
