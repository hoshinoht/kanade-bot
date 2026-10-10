//! `/pings`, `/style` and staff-only `/nick`.

use serde_json::json;

use kanade::domain::members::{MemberStore, PingLevel};

use super::super::support::{ADMIN_ROLE, BOSSING_ROLE};
use super::{ALICE, BOB, CARA, DAN, KALOS, PILOT_ROLE, Slash, focused, opt, user_opt};

#[tokio::test]
async fn pings_reads_and_sets_the_invokers_level() {
    let slash = Slash::new().await;
    assert_eq!(
        slash.run(DAN, "pings", json!([])).await,
        "🔔 You're on **essential** — only when you need to answer — the morning card, the \
         countdowns you haven't ✅'d, a card waiting on your ✅, and someone dropping out of your \
         run.\n`/pings level:` to change it (`all` · `off`)."
    );
    assert_eq!(
        slash.run(DAN, "pings", json!([opt("level", "all")])).await,
        "🔔 Pings set to **all** — everything that lists you, including moves, swaps and \
         weekly-timing changes."
    );
    let dan = slash
        .store
        .load_member(&DAN.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dan.member.ping_level, PingLevel::All);

    // A role holder the roster has not synced yet gets a row first (v4).
    let newcomer = 1_010;
    assert_eq!(
        slash
            .run(newcomer, "pings", json!([opt("level", "off")]))
            .await,
        "🔔 Pings set to **off** — never — you'll still be named in every post, just not \
         notified."
    );
    let row = slash
        .store
        .load_member(&newcomer.to_string())
        .await
        .unwrap()
        .unwrap();
    assert!(row.member.has_role);
    assert_eq!(row.member.ping_level, PingLevel::Off);
}

#[tokio::test]
async fn pings_choices_are_v4_labels() {
    let slash = Slash::new().await;
    let pings = serde_json::to_value(&slash.dispatcher.definitions()[8]).unwrap();
    // v4 cut each label to Discord's 100 characters.
    assert_eq!(
        pings["options"][0]["choices"][0]["name"],
        "essential — only when you need to answer — the morning card, the countdowns you \
         haven't ✅'d, a card "
    );
}

#[tokio::test]
async fn style_needs_chatbot_access() {
    let slash = Slash::new().await;
    let reply = slash.run_in(CARA, &[], KALOS, "style", json!([])).await;
    assert_eq!(
        reply.content,
        "❌ You need chatbot access to set a reply style."
    );
    // Alice's saved style is no longer offered.
    let reply = slash
        .run_in(ALICE, &[PILOT_ROLE], KALOS, "style", json!([]))
        .await;
    assert_eq!(
        reply.content,
        "Your saved reply style is **retired** (currently unavailable). Available: `default`, \
         `terse`."
    );
    let reply = slash
        .run_in(
            ALICE,
            &[PILOT_ROLE],
            KALOS,
            "style",
            json!([opt("profile", " Terse ")]),
        )
        .await;
    assert_eq!(reply.content, "Reply style preference saved as **terse**.");
    let reply = slash
        .run_in(
            ALICE,
            &[PILOT_ROLE],
            KALOS,
            "style",
            json!([opt("profile", "loud")]),
        )
        .await;
    assert_eq!(
        reply.content,
        "That reply style is not available. Use `/style` to see the public choices."
    );
    let reply = slash
        .run_in(
            ALICE,
            &[ADMIN_ROLE],
            KALOS,
            "style",
            json!([opt("profile", "default")]),
        )
        .await;
    assert_eq!(
        reply.content,
        "Reply style preference saved as **default**."
    );
    let alice = slash
        .store
        .load_member(&ALICE.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alice.reply_style, None);

    assert_eq!(
        slash
            .suggest(
                ALICE,
                &[PILOT_ROLE],
                "style",
                json!([focused("profile", "")])
            )
            .await,
        [
            ("default".to_owned(), "default".to_owned()),
            ("terse".to_owned(), "terse".to_owned())
        ]
    );
    assert!(
        slash
            .suggest(CARA, &[], "style", json!([focused("profile", "")]))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn nick_is_staff_only_and_adds_an_alias() {
    let slash = Slash::new().await;
    let add = |alias: &str| json!([user_opt("user", BOB), opt("alias", alias)]);
    let reply = slash
        .run_in(ALICE, &[BOSSING_ROLE], KALOS, "nick", add("MY"))
        .await;
    assert_eq!(
        reply.content,
        "❌ `/nick` is for server admins, the server owner and the admin role."
    );
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "nick", add("MY"))
        .await;
    assert_eq!(reply.content, "✅ <@1002> is now also known as: `my`");
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "nick", add("  "))
        .await;
    assert_eq!(reply.content, "❌ Alias can't be empty.");
    // Aliases are unique across members.
    let reply = slash
        .run_in(
            ALICE,
            &[ADMIN_ROLE],
            KALOS,
            "nick",
            json!([user_opt("user", DAN), opt("alias", "my")]),
        )
        .await;
    assert_eq!(reply.content, "❌ That alias already names someone.");
    // Typed names now resolve through the alias (v4 `/nick`'s purpose).
    let reply = slash
        .run(
            ALICE,
            "fixed",
            super::sub(
                "add",
                json!([
                    opt("bosses", "xkalos"),
                    opt("day", "sun"),
                    opt("time", "20:00"),
                    opt("participants", "my"),
                ]),
            ),
        )
        .await;
    // "my" resolved to Bob, who as the only member also owns the timing.
    assert!(reply.contains("· <@1002> · owner <@1002>"), "{reply}");
}
