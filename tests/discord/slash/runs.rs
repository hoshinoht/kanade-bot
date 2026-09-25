//! `/status` (with the merged cancel/otot/done/restore), `/amend`, `/swap`
//! and `/rsvp`.

use serde_json::json;

use kanade::domain::history::{Actor, ChangeHistory, Surface};
use kanade::domain::notify::NoticeOutbox;
use kanade::domain::schedule::{NoticeChange, RsvpState, RunStatus};
use kanade::domain::scheduler::{ScheduleStore, Scope};

use super::super::support::{ADMIN_ROLE, BOSSING_ROLE};
use super::{ALICE, BOB, DAN, KALOS, R_DONE, R_KALOS, R_NEXT, Slash, focused, opt, user_opt};

fn status(run: &str, state: &str) -> serde_json::Value {
    json!([opt("run_id", run), opt("state", state)])
}

#[tokio::test]
async fn status_sets_each_state_as_the_member_through_discord() {
    let slash = Slash::new().await;
    let head = slash.store.history_head().await.unwrap().seq;
    assert_eq!(
        slash
            .run(ALICE, "status", status("1111", "cancelled"))
            .await,
        "🚫 cancelled — run `#11111111` (XKalos, Tue 29 Sep 22:00)."
    );
    let record = slash.store.load_change(head + 1).await.unwrap().unwrap();
    assert_eq!(record.origin.actor, Actor::member(ALICE.to_string()));
    assert_eq!(record.origin.surface, Surface::Discord);
    assert!(
        record
            .origin
            .request_id
            .as_deref()
            .is_some_and(|id| id.starts_with("discord:"))
    );
    assert_eq!(record.notices, ["notice.run.status.cancelled"]);

    // The notice is in the outbox for the tick, without the portal mark.
    let pending = slash.store.pending_notices().await.unwrap().notices;
    let [notice] = pending.as_slice() else {
        panic!("one notice: {pending:?}");
    };
    assert!(!notice.notice.via_portal);
    assert!(matches!(
        &notice.notice.change,
        NoticeChange::RunStatus { run_id, to: RunStatus::Cancelled, .. } if run_id == R_KALOS
    ));

    assert_eq!(
        slash
            .run(ALICE, "status", status(R_KALOS, "cancelled"))
            .await,
        "Run `#11111111` is already 🚫 cancelled."
    );
    // Own time is labelled as members say it; restore puts it back.
    assert_eq!(
        slash.run(BOB, "status", status(R_KALOS, "otot")).await,
        "🕒 own time — run `#11111111` (XKalos, Tue 29 Sep 22:00)."
    );
    assert_eq!(
        slash.run(BOB, "status", status(R_KALOS, "planned")).await,
        "⚠️ unconfirmed — run `#11111111` (XKalos, Tue 29 Sep 22:00)."
    );
    // A finished run is reachable (v4 `/restore`, `/done`).
    assert_eq!(
        slash.run(ALICE, "status", status(R_DONE, "planned")).await,
        "⚠️ unconfirmed — run `#22222222` (XKalos, Sat 26 Sep 21:00)."
    );
    assert_eq!(slash.run_row(R_DONE).await.status, RunStatus::Planned);
}

#[tokio::test]
async fn only_participants_owners_and_admins_change_a_run() {
    let slash = Slash::new().await;
    let head = slash.store.history_head().await.unwrap().seq;
    assert_eq!(
        slash.run(DAN, "status", status(R_KALOS, "done")).await,
        "❌ You're not on run `#11111111`, so you can't change it."
    );
    assert_eq!(slash.store.history_head().await.unwrap().seq, head);
    let reply = slash
        .run_in(
            DAN,
            &[BOSSING_ROLE, ADMIN_ROLE],
            KALOS,
            "status",
            status(R_KALOS, "done"),
        )
        .await;
    assert_eq!(
        reply.content,
        "🏁 done — run `#11111111` (XKalos, Tue 29 Sep 22:00)."
    );
    assert!(reply.ephemeral);
    // Ids: too short, unknown and ambiguous prefixes (v4 wording).
    assert_eq!(
        slash.run(ALICE, "status", status("11", "done")).await,
        "❌ `11` is too short - give at least 4 characters - pick a run from the dropdown or \
         check `/schedule`"
    );
    assert_eq!(
        slash.run(ALICE, "status", status("9999", "done")).await,
        "❌ nothing matches `9999` - pick a run from the dropdown or check `/schedule`"
    );
}

#[tokio::test]
async fn run_pickers_list_the_invokers_runs() {
    let slash = Slash::new().await;
    // `/amend` lists live runs of the materialised weeks.
    assert_eq!(
        slash
            .suggest(
                ALICE,
                &[BOSSING_ROLE],
                "amend",
                json!([focused("run_id", "")])
            )
            .await,
        [
            (
                "XKalos · Tue 29 Sep 22:00 · #kalos-four · 11111111".to_owned(),
                R_KALOS.to_owned()
            ),
            (
                "XKalos · Tue 06 Oct 22:00 · #kalos-four · 33333333".to_owned(),
                R_NEXT.to_owned()
            ),
        ]
    );
    // `/status` also reaches finished runs, labelled with their status.
    let choices = slash
        .suggest(
            ALICE,
            &[BOSSING_ROLE],
            "status",
            json!([focused("run_id", "#2222")]),
        )
        .await;
    assert_eq!(
        choices,
        [(
            "XKalos · Sat 26 Sep 21:00 · #lounge · 22222222 · done".to_owned(),
            R_DONE.to_owned()
        )]
    );
    // Dan is on nothing; an admin sees everyone's; no role lists nothing.
    assert!(
        slash
            .suggest(
                DAN,
                &[BOSSING_ROLE],
                "amend",
                json!([focused("run_id", "")])
            )
            .await
            .is_empty()
    );
    assert_eq!(
        slash
            .suggest(
                DAN,
                &[BOSSING_ROLE, ADMIN_ROLE],
                "amend",
                json!([focused("run_id", "")])
            )
            .await
            .len(),
        2
    );
    assert!(
        slash
            .suggest(ALICE, &[], "amend", json!([focused("run_id", "")]))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn amend_moves_the_run_and_queues_the_notice() {
    let slash = Slash::new().await;
    assert_eq!(
        slash
            .run(
                ALICE,
                "amend",
                json!([opt("run_id", R_KALOS), opt("to", "wed 21:30")])
            )
            .await,
        "✅ Run `#11111111` moved to Wed 30 Sep 21:30."
    );
    let pending = slash.store.pending_notices().await.unwrap().notices;
    assert!(matches!(
        pending.as_slice(),
        [notice] if matches!(notice.notice.change, NoticeChange::RunMoved { .. })
    ));
    assert_eq!(
        slash
            .run(
                ALICE,
                "amend",
                json!([opt("run_id", R_KALOS), opt("to", "whenever")])
            )
            .await,
        "❌ couldn't read `whenever` as a date - try `wed 21:30` or `2026-09-02 21:30`"
    );
    // Into next week, where the weekly already has its run (v4 slash wording).
    assert_eq!(
        slash
            .run(
                ALICE,
                "amend",
                json!([opt("run_id", R_KALOS), opt("to", "2026-10-05 21:00")])
            )
            .await,
        "❌ That weekly already has a run in the week of Thu 01 Oct (`#33333333` on Tue 06 Oct \
         22:00). Edit that existing run instead, or keep this move within its current boss week."
    );
}

#[tokio::test]
async fn swap_changes_this_week_only() {
    let slash = Slash::new().await;
    assert_eq!(
        slash
            .run(ALICE, "swap", json!([opt("run_id", R_KALOS)]))
            .await,
        "❌ Pick someone to swap out, in, or both."
    );
    assert_eq!(
        slash
            .run(
                ALICE,
                "swap",
                json!([
                    opt("run_id", R_KALOS),
                    user_opt("out", BOB),
                    user_opt("in", DAN)
                ])
            )
            .await,
        "✅ Run `#11111111` this week: Alice, Dan\nThe weekly timing is unchanged — use \
         `/fixed edit` for that."
    );
    let snapshot = slash.store.load(&Scope::All).await.unwrap();
    let weekly = &snapshot.fixed_runs[0];
    assert_eq!(weekly.participants, [ALICE.to_string(), BOB.to_string()]);
    assert_eq!(
        slash.run_row(R_KALOS).await.participants,
        [ALICE.to_string(), DAN.to_string()]
    );
    // A bot cannot stand in (the scheduler's rule).
    assert_eq!(
        slash
            .run(
                ALICE,
                "swap",
                json!([opt("run_id", R_KALOS), user_opt("in", super::BOTTY)])
            )
            .await,
        format!("❌ bots can't be participants: {}", super::BOTTY)
    );
}

#[tokio::test]
async fn rsvp_answers_for_the_invoker_only() {
    let slash = Slash::new().await;
    assert_eq!(
        slash
            .run(
                BOB,
                "rsvp",
                json!([opt("run_id", R_KALOS), opt("answer", "no")])
            )
            .await,
        "Noted: **no** for run `#11111111` (❗ at risk)."
    );
    let snapshot = slash.store.load(&Scope::Run(R_KALOS.into())).await.unwrap();
    assert!(
        snapshot
            .rsvps
            .iter()
            .any(|rsvp| rsvp.user_id == BOB.to_string() && rsvp.state == RsvpState::No)
    );
    assert_eq!(
        slash
            .run(
                DAN,
                "rsvp",
                json!([opt("run_id", R_KALOS), opt("answer", "yes")])
            )
            .await,
        "❌ You're not on run `#11111111`."
    );
    assert_eq!(
        slash
            .run(
                ALICE,
                "rsvp",
                json!([opt("run_id", R_KALOS), opt("answer", "yes")])
            )
            .await,
        "Noted: **yes** for run `#11111111` (❗ at risk)."
    );
}
