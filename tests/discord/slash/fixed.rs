//! `/fixed add|list|edit|remove`.

use chrono::Timelike;
use serde_json::json;

use kanade::domain::history::{ChangeHistory, Surface};
use kanade::domain::schedule::RunStatus;
use kanade::domain::scheduler::{ScheduleStore, Scope};

use super::super::support::{ADMIN_ROLE, BOSSING_ROLE};
use super::{
    ALICE, BOB, CARA, DAN, F_KALOS, KALOS, LOUNGE, R_KALOS, R_NEXT, Slash, focused, opt, sub,
    user_opt,
};

/// v4 `formatting.fixed_run_line(fixed, table)` for `F_KALOS`, printed by the
/// rollback tree.
const KALOS_LINE: &str = "`#ffff0001` **XKalos** · Tue 22:00 · <@1001> <@1002> · owner <@1001> · \
                          <#301>\n   ↳ Gatekeeper Kalos (Extreme, Lv265)";

#[tokio::test]
async fn list_shows_v4_lines_for_mine_or_all() {
    let slash = Slash::new().await;
    assert_eq!(
        slash.run(BOB, "fixed", sub("list", json!([]))).await,
        KALOS_LINE
    );
    assert_eq!(
        slash.run(DAN, "fixed", sub("list", json!([]))).await,
        "You're not on any fixed run. `/fixed list scope:all` shows every party's."
    );
    assert_eq!(
        slash
            .run(DAN, "fixed", sub("list", json!([opt("scope", "all")])))
            .await,
        KALOS_LINE
    );
}

/// "Mine" means on the party: owning a timing (here, creating it for others)
/// is not enough (user decision 2026-10-09).
#[tokio::test]
async fn owning_a_timing_without_being_on_it_is_not_mine() {
    let slash = Slash::new().await;
    slash
        .run(
            DAN,
            "fixed",
            sub(
                "add",
                json!([
                    opt("bosses", "hstar"),
                    opt("day", "wed"),
                    opt("time", "2130"),
                    user_opt("member1", BOB),
                ]),
            ),
        )
        .await;
    assert_eq!(
        slash.run(DAN, "fixed", sub("list", json!([]))).await,
        "You're not on any fixed run. `/fixed list scope:all` shows every party's."
    );
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], LOUNGE, "schedule", json!([]))
        .await;
    assert_eq!(
        reply.embeds[0].description.as_deref(),
        Some("You have nothing left this week. `/schedule scope:all` shows everyone's.")
    );
    // Bob, on the new run, does see it as his.
    let reply = slash
        .run_in(BOB, &[BOSSING_ROLE], LOUNGE, "schedule", json!([]))
        .await;
    let lines: Vec<&str> = reply.embeds[0]
        .fields
        .iter()
        .map(|f| f.value.as_str())
        .collect();
    assert!(
        lines.iter().any(|line| line.contains("HMaleficStar")),
        "{lines:?}"
    );
}

#[tokio::test]
async fn add_creates_the_timing_here_and_materialises_it() {
    let slash = Slash::new().await;
    let reply = slash
        .run(
            DAN,
            "fixed",
            sub(
                "add",
                json!([
                    opt("bosses", "hstar"),
                    opt("day", "wed"),
                    opt("time", "2130"),
                    user_opt("member1", BOB),
                    opt("participants", "Alice"),
                ]),
            ),
        )
        .await;
    let snapshot = slash.store.load(&Scope::All).await.unwrap();
    let fixed = snapshot
        .fixed_runs
        .iter()
        .find(|fixed| fixed.id != F_KALOS)
        .expect("created");
    assert_eq!(fixed.owner_id, DAN.to_string());
    assert_eq!(fixed.channel_id.as_deref(), Some(&*KALOS.to_string()));
    assert_eq!(fixed.participants, [BOB.to_string(), ALICE.to_string()]);
    let short = &fixed.id.replace('-', "")[..8];
    assert_eq!(
        reply,
        format!(
            "✅ Fixed run `#{short}` added — this channel is its home channel, so its pings land \
             here.\n`#{short}` **HMaleficStar** · Wed 21:30 · <@1002> <@1001> · owner <@1002> · \
             <#301>\n   ↳ Radiant Malefic Star (Hard, Lv280)\n(you're not on this run, so it \
             won't ping you; <@1002> owns it and can `/fixed edit` you in)"
        )
    );
    // Materialised: this week's Wednesday is still ahead.
    assert!(snapshot.runs.iter().any(
        |run| run.fixed_run_id.as_ref() == Some(&fixed.id) && run.status == RunStatus::Planned
    ));
    let head = slash.store.history_head().await.unwrap().seq;
    let created = slash.store.load_change(head).await.unwrap().unwrap();
    assert_eq!(created.origin.surface, Surface::Discord);
}

#[tokio::test]
async fn add_refuses_with_v4_texts() {
    let slash = Slash::new().await;
    let add = |bosses: &str, time: &str, extra: serde_json::Value| {
        let mut options = vec![opt("bosses", bosses), opt("day", "wed"), opt("time", time)];
        if let serde_json::Value::Array(more) = extra {
            options.extend(more);
        }
        sub("add", serde_json::Value::Array(options))
    };
    let catalog = super::catalog();
    let refusal = catalog.parse("kalos").unwrap_err().to_string();
    assert_eq!(
        slash
            .run(ALICE, "fixed", add("kalos", "21:30", json!([])))
            .await,
        format!("❌ {refusal}")
    );
    assert_eq!(
        slash
            .run(ALICE, "fixed", add("xkalos", "25:99", json!([])))
            .await,
        "❌ expected a time like 21:30, 2130 or 9:30pm, got '25:99'"
    );
    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            LOUNGE,
            "fixed",
            add("xkalos", "21:30", json!([user_opt("member1", ALICE)])),
        )
        .await;
    assert_eq!(
        reply.content,
        "❌ This channel isn't watched, so a run here would never get its pings. Run `/fixed \
         add` in your party's channel, or add this channel to `CHAT_CHANNEL_IDS` / its category \
         to `CHAT_CATEGORY_IDS`."
    );
    assert_eq!(
        slash
            .run(
                ALICE,
                "fixed",
                add("xkalos", "21:30", json!([user_opt("member1", CARA)]))
            )
            .await,
        format!("❌ not in the bossing role: <@{CARA}>")
    );
    assert_eq!(
        slash
            .run(ALICE, "fixed", add("xkalos", "21:30", json!([])))
            .await,
        "❌ a run needs at least one participant"
    );
    assert_eq!(
        slash
            .store
            .load(&Scope::All)
            .await
            .unwrap()
            .fixed_runs
            .len(),
        1
    );
}

#[tokio::test]
async fn edit_updates_the_timing_and_its_runs() {
    let slash = Slash::new().await;
    assert_eq!(
        slash
            .run(BOB, "fixed", sub("edit", json!([opt("id", "ffff")])))
            .await,
        "Nothing to change."
    );
    assert_eq!(
        slash
            .run(
                DAN,
                "fixed",
                sub("edit", json!([opt("id", "ffff"), opt("time", "23:00")]))
            )
            .await,
        "❌ You're not on fixed run `#ffff0001`."
    );
    assert_eq!(
        slash
            .run(
                BOB,
                "fixed",
                sub("edit", json!([opt("id", "ffff"), opt("time", "23:00")]))
            )
            .await,
        "✅ Updated.\n`#ffff0001` **XKalos** · Tue 23:00 · <@1001> <@1002> · owner <@1001> · \
         <#301>\n   ↳ Gatekeeper Kalos (Extreme, Lv265)"
    );
    // Both live runs follow (v4's slash edit), this week's too.
    for run in [R_KALOS, R_NEXT] {
        let run = slash.run_row(run).await;
        let local = run.datetime.with_timezone(&chrono_tz::Asia::Kuala_Lumpur);
        assert_eq!((local.hour(), local.minute()), (23, 0));
    }
    assert_eq!(
        slash
            .run(
                BOB,
                "fixed",
                sub(
                    "edit",
                    json!([
                        opt("id", "ffff"),
                        json!({ "name": "channel", "type": 7, "value": LOUNGE.to_string() })
                    ])
                )
            )
            .await,
        format!("❌ <#{LOUNGE}> isn't a watched channel, so its runs would never get their pings.")
    );
}

#[tokio::test]
async fn remove_cancels_the_live_runs() {
    let slash = Slash::new().await;
    assert_eq!(
        slash
            .run(DAN, "fixed", sub("remove", json!([opt("id", F_KALOS)])))
            .await,
        "❌ You're not on fixed run `#ffff0001`."
    );
    let reply = slash
        .run_in(
            DAN,
            &[BOSSING_ROLE, ADMIN_ROLE],
            KALOS,
            "fixed",
            sub("remove", json!([opt("id", "#FFFF0001")])),
        )
        .await;
    assert_eq!(
        reply.content,
        "🗑️ Fixed run `#ffff0001` removed (2 upcoming run(s) cancelled)."
    );
    let snapshot = slash.store.load(&Scope::All).await.unwrap();
    assert!(snapshot.fixed_runs.is_empty());
    for id in [R_KALOS, R_NEXT] {
        assert_eq!(slash.run_row(id).await.status, RunStatus::Cancelled);
    }
}

#[tokio::test]
async fn timing_picker_lists_the_invokers_timings() {
    let slash = Slash::new().await;
    let choices = slash
        .suggest(
            BOB,
            &[BOSSING_ROLE],
            "fixed",
            sub("edit", json!([focused("id", "")])),
        )
        .await;
    assert_eq!(
        choices,
        [(
            "XKalos · Tue 22:00 · #kalos-four · ffff0001".to_owned(),
            F_KALOS.to_owned()
        )]
    );
    assert!(
        slash
            .suggest(
                DAN,
                &[BOSSING_ROLE],
                "fixed",
                sub("remove", json!([focused("id", "")]))
            )
            .await
            .is_empty()
    );
}

/// The live bug: twelve timings with full parties are ~2,400 characters,
/// over Discord's 2,000, so the deferred reply was never completed.
#[tokio::test]
async fn a_long_list_is_split_into_follow_ups_at_line_boundaries() {
    use kanade::bot::transport::Call;
    use kanade::domain::attendance::AttendanceDefault;
    use kanade::domain::history::{Actor, ChangeMeta, Origin};
    use kanade::domain::schedule::{Change, ChangeSet, FixedRun};

    let slash = Slash::new().await;
    let party: Vec<String> = [ALICE, BOB, DAN, 1_010, 1_011, 1_012]
        .iter()
        .map(u64::to_string)
        .collect();
    let changes = (2..=12)
        .map(|index| {
            Change::PutFixedRun(FixedRun {
                owner_pinned: false,
                id: format!("ffff{index:04}-0000-4000-8000-000000000000"),
                owner_id: ALICE.to_string(),
                channel_id: Some(KALOS.to_string()),
                bosses: vec!["XKalos".into(), "HMaleficStar".into()],
                weekday: chrono::Weekday::Sat,
                time: chrono::NaiveTime::from_hms_opt(20, index, 0).unwrap(),
                participants: party.clone(),
                note: None,
                attendance_default: AttendanceDefault::default(),
                standing: Vec::new(),
            })
        })
        .collect();
    let revision = slash.store.load(&Scope::All).await.unwrap().revision;
    slash
        .store
        .commit(
            revision,
            ChangeSet { changes },
            ChangeMeta {
                origin: Origin::new(Actor::admin("seed"), Surface::AdminPortal),
                at: super::now(),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Default::default(),
                outbox: Vec::new(),
            },
        )
        .await
        .unwrap();

    let before = slash.discord.calls().len();
    slash
        .run(DAN, "fixed", sub("list", json!([opt("scope", "all")])))
        .await;
    let sent: Vec<(String, bool)> = slash.discord.calls()[before..]
        .iter()
        .filter_map(|call| match call {
            Call::CompleteDeferred { reply, .. } | Call::Followup { reply, .. } => {
                Some((reply.content.clone(), reply.ephemeral))
            }
            _ => None,
        })
        .collect();
    assert!(sent.len() >= 2, "{sent:?}");
    let whole: Vec<&str> = sent.iter().map(|(content, _)| content.as_str()).collect();
    let whole = whole.join("\n");
    assert!(whole.encode_utf16().count() > 2_000);
    assert_eq!(whole.matches("`#ffff").count(), 12);
    assert!(whole.starts_with(KALOS_LINE));
    for (content, ephemeral) in &sent {
        assert!(*ephemeral);
        assert!(content.encode_utf16().count() <= 2_000);
        // Never mid-line: every message starts a timing's line.
        assert!(content.starts_with("`#ffff"), "{content}");
    }
}
