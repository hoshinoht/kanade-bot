//! `/character link|unlink|data|share` (module `character_data`) through the
//! real dispatcher, `FakeDiscord`, the temp SQLite roster, a temp SQLite
//! character-data file and a fake Open API on loopback with invented
//! fixtures. Texts are the approved copy boards' (D_LinkPick, D_LinkNotice,
//! D_LinkReplies, D_Data, D_Share, D_ShareStripped), updated for admin
//! links that are active at once and the roster's copy of NEXON's class and
//! level (user decisions 2026-10-13). The service clock is the test's, so a
//! share card's day 29, day 30 and a waiting link's day 14 can be reached.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::Router;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use twilight_model::application::interaction::message_component::MessageComponentInteractionData;
use twilight_model::application::interaction::{InteractionData, InteractionType};
use twilight_model::channel::message::Component;
use twilight_model::id::Id;

use kanade::bot::chardata::DiscordPosts;
use kanade::bot::commands::CharacterParts;
use kanade::bot::transport::{Call, InteractionReply, Op, RejectionKind, Step};
use kanade::domain::characters::{
    Character, CharacterChange, CharacterRecord, CharacterRole, CharacterStore,
};
use kanade::domain::chardata::{
    CharDataStore, Link, LinkPut, LinkStatus, NOTICE_VERSION, Post, PostState,
};
use kanade::domain::history::{Actor, Surface};
use kanade::domain::settings::{SettingsChangeQuery, SettingsStore};
use kanade::infrastructure::character_data::{
    AdminLinkOutcome, CharacterData, FetchState, RosterSide, Timing,
};
use kanade::infrastructure::nexon::{ApiKey, NexonClient};
use kanade::infrastructure::store::SqliteCharData;

use super::super::support::{
    BOSSING_ROLE, message_json, parse, parse_message, user_json, v2_buttons, v2_texts as texts,
};
use super::{ALICE, BOB, KALOS, Ports, Slash, focused, now, opt, sub};

/// The bot's own user id (its posts are found by ref only when it wrote them).
const BOT: u64 = 1_999;
const BISH: &str = "c-bish";
const NL: &str = "c-nl";
const KMS: &str = "c-kms";
const OCID: &str = "ocid-invented-s3";
const ATTR: &str = "Data based on NEXON Open API";
const PRIVACY: &str = "https://kanade-pub.example/privacy";

/// The fake Open API: invented MikaBish data, or no such character.
#[derive(Default)]
struct FakeNexon {
    calls: AtomicUsize,
    missing: AtomicBool,
}

async fn fake_nexon() -> (SocketAddr, Arc<FakeNexon>) {
    let fake = Arc::new(FakeNexon::default());
    let state = Arc::clone(&fake);
    let app = Router::new().fallback(move |request: axum::extract::Request| {
        let fake = Arc::clone(&state);
        async move {
            fake.calls.fetch_add(1, Ordering::SeqCst);
            let path = request.uri().path().to_owned();
            if path.ends_with("/v1/id") {
                if fake.missing.load(Ordering::SeqCst) {
                    r#"{"ocid":null}"#.to_owned()
                } else {
                    format!(r#"{{"ocid":"{OCID}"}}"#)
                }
            } else if path.ends_with("/character/basic") {
                r#"{"date":"2026-09-28T00:00+08:00","character_name":"MikaBish","world_name":"Aquila","character_class":"Bishop","character_level":283,"character_image":"https://open.api.nexon.com/static/invented-s3"}"#.to_owned()
            } else {
                r#"{"final_stat":[{"stat_name":"Combat Power","stat_value":"48203114"}]}"#
                    .to_owned()
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (address, fake)
}

struct Rig {
    slash: Slash,
    service: Arc<CharacterData>,
    file: Arc<SqliteCharData>,
    nexon: Arc<FakeNexon>,
    clock: Arc<Mutex<DateTime<Utc>>>,
}

fn character(id: &str, ign: &str, world: &str, class: &str, role: CharacterRole) -> Character {
    Character {
        id: id.into(),
        member_id: ALICE.to_string(),
        ign: ign.into(),
        world: world.into(),
        class: Some(class.into()),
        level: Some(280),
        role,
        retired_at: None,
        created_at: now(),
        updated_at: now(),
        updated_by: Actor::admin("seed"),
        version: 1,
    }
}

async fn seed_character(store: &kanade::infrastructure::store::SqliteStore, row: Character) {
    let record = CharacterRecord {
        at: now(),
        actor: Actor::admin("seed"),
        surface: Surface::AdminPortal,
    };
    let written = store
        .apply_character(CharacterChange::Create(row), record)
        .await
        .unwrap();
    assert!(written.applied(), "{written:?}");
}

async fn rig_with(connected: bool, character_module: bool) -> Rig {
    let (address, nexon) = fake_nexon().await;
    let file = Arc::new(SqliteCharData::open_temp().await);
    let clock = Arc::new(Mutex::new(now()));
    let slot: Arc<OnceLock<Arc<CharacterData>>> = Arc::new(OnceLock::new());
    let maker = {
        let (file, clock, slot) = (Arc::clone(&file), Arc::clone(&clock), Arc::clone(&slot));
        Box::new(
            move |store: Arc<kanade::infrastructure::store::SqliteStore>,
                  discord: Arc<kanade::bot::transport::FakeDiscord>| {
                let service = Arc::new(CharacterData::new(
                    file as Arc<dyn CharDataStore>,
                    store as Arc<dyn RosterSide>,
                    connected.then(|| NexonClient::plain(address, ApiKey::new("invented-key"))),
                    Arc::new(move || *clock.lock().unwrap()),
                    Timing {
                        spacing: Duration::from_millis(1),
                        rate_limit_unit: Duration::from_millis(1),
                    },
                ));
                let posts = Arc::new(DiscordPosts {
                    transport: discord,
                    self_id: Arc::new(|| Some(Id::new(BOT))),
                    search_pause: Duration::ZERO,
                });
                service.attach_posts(Arc::clone(&posts) as _);
                slot.set(Arc::clone(&service)).ok().unwrap();
                CharacterParts {
                    service,
                    posts,
                    privacy_url: Some(PRIVACY.into()),
                }
            },
        ) as super::CharacterMaker
    };
    let slash = Slash::with(Ports {
        character: character_module.then_some(maker),
        ..Ports::default()
    })
    .await;
    for row in [
        character(BISH, "MikaBish", "msea", "Bishop", CharacterRole::Main),
        character(NL, "MikaNL", "msea", "Night Lord", CharacterRole::Alt),
        character(KMS, "MikaKms", "kms", "Hero", CharacterRole::Alt),
    ] {
        seed_character(&slash.store, row).await;
    }
    let service = slot.get().cloned().unwrap_or_else(|| {
        Arc::new(CharacterData::new(
            Arc::clone(&file) as Arc<dyn CharDataStore>,
            slash.store.clone() as Arc<dyn RosterSide>,
            None,
            Arc::new(now),
            Timing::default(),
        ))
    });
    Rig {
        slash,
        service,
        file,
        nexon,
        clock,
    }
}

async fn rig() -> Rig {
    rig_with(true, true).await
}

/// A reply's words: its text displays, else its content.
fn words(reply: &InteractionReply) -> Vec<String> {
    if reply.components.is_empty() {
        vec![reply.content.clone()]
    } else {
        texts(&reply.components)
    }
}

impl Rig {
    fn set_now(&self, at: DateTime<Utc>) {
        *self.clock.lock().unwrap() = at;
    }

    async fn run(&self, leaf: &str, options: Value) -> InteractionReply {
        self.slash
            .run_in(
                ALICE,
                &[BOSSING_ROLE],
                KALOS,
                "character",
                sub(leaf, options),
            )
            .await
    }

    async fn press(&self, custom_id: &str) -> InteractionReply {
        let mut interaction =
            self.slash
                .interaction(2, ALICE, &[BOSSING_ROLE], KALOS, "press", json!([]));
        interaction.kind = InteractionType::MessageComponent;
        interaction.data = Some(InteractionData::MessageComponent(Box::new(parse::<
            MessageComponentInteractionData,
        >(
            json!({
            "custom_id": custom_id,
            "component_type": 2,
        })
        ))));
        self.slash.send(&interaction).await.1
    }

    /// MikaBish waits for Alice to confirm (the sweeper's re-pend after a
    /// new notice: the one pending flow left).
    async fn waiting(&self) {
        self.waiting_for(BISH).await;
    }

    async fn waiting_for(&self, id: &str) {
        let character = CharacterStore::character(&*self.slash.store, id)
            .await
            .unwrap()
            .unwrap();
        let pending = Link {
            character_id: id.into(),
            member_id: character.member_id.clone(),
            ign_key: character.ign_key(),
            region: "msea".into(),
            status: LinkStatus::Pending,
            requested_by: Actor::system("chardata"),
            requested_at: now(),
            consent_version: None,
            consented_at: None,
            linked_at: None,
            consent_surface: None,
            version: 1,
        };
        assert_eq!(
            self.file.put_link(pending, 0).await.unwrap(),
            LinkPut::Written
        );
    }

    /// `user`'s `/character <leaf>` picker for an empty query.
    async fn choices(&self, user: u64, leaf: &str) -> Vec<(String, String)> {
        let options = sub(leaf, json!([focused("character", "")]));
        self.slash
            .suggest(user, &[BOSSING_ROLE], "character", options)
            .await
    }

    /// `/character link character:MikaBish`; the notice's Link custom id.
    async fn notice(&self) -> (InteractionReply, String) {
        let reply = self.run("link", json!([opt("character", BISH)])).await;
        let link = v2_buttons(&reply.components)
            .into_iter()
            .find(|(label, ..)| label.starts_with("Link"))
            .expect("a Link button")
            .1;
        (reply, link)
    }

    async fn link(&self) {
        let (_, link) = self.notice().await;
        let reply = self.press(&link).await;
        assert!(
            words(&reply)[0].starts_with("-# Linked · MikaBish"),
            "{reply:?}"
        );
    }

    async fn status(&self, id: &str) -> Option<LinkStatus> {
        self.file
            .link(id.into())
            .await
            .unwrap()
            .map(|link| link.status)
    }

    async fn posts(&self) -> Vec<Post> {
        self.file.posts().await.unwrap()
    }

    /// History rows naming `link.<id>`: (from, to, actor), oldest first.
    async fn history(&self) -> Vec<(String, String, String)> {
        let mut rows: Vec<_> = self
            .slash
            .store
            .settings_changes(SettingsChangeQuery::default())
            .await
            .unwrap()
            .into_iter()
            .flat_map(|change| {
                let actor = format!("{}:{}", change.actor.kind(), change.actor.id());
                change
                    .values
                    .into_iter()
                    .filter(|(key, _)| key.starts_with("link."))
                    .map(move |(_, diff)| (diff.from, diff.to, actor.clone()))
            })
            .collect();
        rows.reverse();
        rows
    }

    /// The last share card's create call.
    fn created(&self) -> Vec<Component> {
        self.slash
            .discord
            .calls()
            .into_iter()
            .rev()
            .find_map(|call| match call {
                Call::Create { message, .. } => Some(message.components),
                _ => None,
            })
            .expect("a create")
    }

    fn edits(&self) -> Vec<Vec<Component>> {
        self.slash
            .discord
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Edit { edit, .. } => edit.components,
                _ => None,
            })
            .collect()
    }
}

const STUB: &str = "Character data removed (expired or unlinked).";

/// History and settings rows hold no Nexon field but the class and level
/// copied into the roster row (its own History row, by `system:nexon`).
async fn nexon_free(rig: &Rig) {
    let rows = rig
        .slash
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap();
    let text = format!("{rows:?}");
    for fixture in [OCID, "Aquila", "48203114", "48,203,114", "invented-s3"] {
        assert!(!text.contains(fixture), "{fixture} reached History: {text}");
    }
}

#[tokio::test]
async fn the_command_is_absent_while_the_module_is_off() {
    let off = rig_with(true, false).await;
    let names: Vec<String> = off
        .slash
        .dispatcher
        .definitions()
        .into_iter()
        .map(|command| command.name)
        .collect();
    assert!(!names.contains(&"character".to_owned()), "{names:?}");
    let on = rig().await;
    let definitions = on.slash.dispatcher.definitions();
    let character = definitions
        .iter()
        .find(|command| command.name == "character")
        .expect("registered while on");
    assert_eq!(
        definitions.last().map(|command| command.name.as_str()),
        Some("character"),
        "after the retained commands"
    );
    let leaves: Vec<&str> = character
        .options
        .iter()
        .map(|option| option.name.as_str())
        .collect();
    assert_eq!(leaves, ["link", "unlink", "data", "share"]);
}

#[tokio::test]
async fn the_picker_lists_waiting_links_first_and_only_msea_characters() {
    let rig = rig().await;
    rig.waiting_for(NL).await;
    let choices = rig.choices(ALICE, "link").await;
    assert_eq!(
        choices,
        [
            (
                "MikaNL · Night Lord · waiting for you".to_owned(),
                NL.to_owned()
            ),
            ("MikaBish · Bishop · not linked".to_owned(), BISH.to_owned()),
        ]
    );
    // Someone else's characters are never offered.
    let theirs = rig.choices(BOB, "link").await;
    assert!(theirs.is_empty(), "{theirs:?}");
}

#[tokio::test]
async fn the_notice_is_version_one_as_drawn_and_calls_nothing() {
    let rig = rig().await;
    rig.waiting().await;
    let (reply, link) = rig.notice().await;
    assert!(reply.ephemeral);
    assert_eq!(
        words(&reply),
        [
            "### Link MikaBish to its NEXON Open API data?",
            "Nothing is fetched until you press Link.",
            "**What Kanade fetches**\nName, world, class, level, combat power and the character image link, for MikaBish only.",
            "**Why**\nSo guild admins can check boss readiness (level and combat power) when they plan runs.",
            "**How long**\nRefreshed once a day and kept 30 days at most. `/character unlink` deletes it at once; the class and level copied to your roster character stay.",
            "**Who sees it**\nGuild admins in Kanade’s admin app, and the channel when you post a `/character share` card. Its class and level are copied to your roster character, which guild members see. It is never shared with anyone else and never sent to chat models.",
            &format!(
                "-# The data is shown as is and may be out of date. Kanade is not affiliated with or endorsed by NEXON. [Privacy notice]({PRIVACY})"
            ),
            &format!("-# {ATTR}"),
        ]
    );
    assert_eq!(NOTICE_VERSION, 2);
    let issued = now().timestamp();
    assert_eq!(
        v2_buttons(&reply.components),
        [
            (
                "Link MikaBish".to_owned(),
                format!("chardata:link:2:{issued}:{BISH}"),
                false
            ),
            (
                "Cancel".to_owned(),
                format!("chardata:cancel:{BISH}"),
                false
            ),
        ]
    );
    assert_eq!(link, format!("chardata:link:2:{issued}:{BISH}"));
    assert_eq!(rig.nexon.calls.load(Ordering::SeqCst), 0);
    assert_eq!(rig.status(BISH).await, Some(LinkStatus::Pending));

    let plain = rig.run("link", json!([opt("character", "mikanl")])).await;
    assert_eq!(words(&plain)[1], "Nothing is fetched until you press Link.");
    assert_eq!(
        words(&plain)[0],
        "### Link MikaNL to its NEXON Open API data?"
    );
}

#[tokio::test]
async fn link_makes_a_waiting_link_active_with_the_first_fetch() {
    let rig = rig().await;
    rig.waiting().await;
    let (_, link) = rig.notice().await;
    let reply = rig.press(&link).await;
    assert_eq!(
        words(&reply),
        [
            "-# Linked · MikaBish",
            "**Class** Bishop · **Level** 283 · **Combat power** 48,203,114",
            "**World** Aquila · **As of** 28 Sep 2026 (SGT)",
            "-# Refreshed daily, kept 30 days at most; its class and level are copied to your roster character. See all of it with `/character data`; remove it with `/character unlink`.",
            &format!("-# {ATTR}"),
        ]
    );
    // The notice was replaced in place (deferred update, then the edit).
    let calls = rig.slash.discord.calls();
    assert!(
        calls
            .iter()
            .any(|call| matches!(call, Call::DeferUpdate { .. }))
    );
    assert_eq!(
        rig.nexon.calls.load(Ordering::SeqCst),
        3,
        "id + basic + stat"
    );
    let stored = rig.file.link(BISH.into()).await.unwrap().unwrap();
    assert_eq!(stored.status, LinkStatus::Active);
    assert_eq!(stored.consent_version, Some(NOTICE_VERSION));
    assert_eq!(
        rig.history().await,
        [("pending".into(), "linked".into(), format!("member:{ALICE}"))]
    );
    let row = CharacterStore::character(&*rig.slash.store, BISH)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.level, row.updated_by),
        (Some(283), Actor::system("nexon")),
        "NEXON's level is copied into the roster"
    );
    nexon_free(&rig).await;
    // A second notice for it says so.
    let again = rig.run("link", json!([opt("character", BISH)])).await;
    assert_eq!(
        again.content,
        "**MikaBish** is already linked. See it with `/character data`."
    );
}

#[tokio::test]
async fn cancel_changes_nothing_and_an_old_notice_expires() {
    let rig = rig().await;
    rig.waiting().await;
    let reply = rig.press(&format!("chardata:cancel:{BISH}")).await;
    assert_eq!(words(&reply), ["Nothing was linked."]);
    assert_eq!(rig.status(BISH).await, Some(LinkStatus::Pending));
    let reply = rig.press(&format!("chardata:cancel:{NL}")).await;
    assert_eq!(words(&reply), ["Nothing was linked."]);
    // An old notice expires; one "issued" in the future (past the 60 s
    // skew) is refused too: it would otherwise stay valid for as long as it
    // claims.
    for issued in [
        now() - TimeDelta::minutes(16),
        now() + TimeDelta::minutes(2),
    ] {
        let issued = issued.timestamp();
        let reply = rig.press(&format!("chardata:link:2:{issued}:{BISH}")).await;
        assert_eq!(
            words(&reply),
            ["This notice expired. Run /character link again."]
        );
        assert_eq!(rig.status(BISH).await, Some(LinkStatus::Pending));
    }
    assert_eq!(rig.nexon.calls.load(Ordering::SeqCst), 0);
}

/// `/character data` is the access right: every stored item is shown, in as
/// many ephemeral messages as Discord's limits need, each with the
/// attribution.
#[tokio::test]
async fn data_shows_every_entry_across_messages() {
    let rig = rig().await;
    let igns: Vec<String> = (1..=35).map(|n| format!("MikaAlt{n:02}")).collect();
    for (n, ign) in igns.iter().enumerate() {
        let id = format!("c-alt-{n:02}");
        seed_character(
            &rig.slash.store,
            character(&id, ign, "msea", "Bishop", CharacterRole::Alt),
        )
        .await;
        rig.waiting_for(&id).await;
    }
    let before = rig.slash.discord.calls().len();
    rig.run("data", json!([])).await;
    let messages: Vec<Vec<String>> = rig.slash.discord.calls()[before..]
        .iter()
        .filter_map(|call| match call {
            Call::CompleteDeferred { reply, .. } | Call::Followup { reply, .. } => {
                assert!(reply.ephemeral);
                Some(texts(&reply.components))
            }
            _ => None,
        })
        .collect();
    assert!(
        messages.len() > 1,
        "split across messages: {}",
        messages.len()
    );
    let all = messages.concat().join("\n");
    for ign in &igns {
        assert!(
            all.contains(&format!("**{ign} · waiting for you**")),
            "{ign} missing"
        );
    }
    for message in &messages {
        assert_eq!(
            message.last().map(String::as_str),
            Some(&*format!("-# {ATTR}"))
        );
    }
    assert_eq!(messages[0][0], "### What Kanade keeps for you");
}

#[tokio::test]
async fn refusals_read_as_drawn() {
    let rig = rig().await;
    let reply = rig.run("link", json!([opt("character", "MikaKain")])).await;
    assert_eq!(
        reply.content,
        "MikaKain isn’t one of your characters on the roster. An admin adds characters on Members."
    );
    let reply = rig.run("link", json!([opt("character", KMS)])).await;
    assert_eq!(reply.content, "Only MapleSEA characters can be linked.");
    let reply = rig.run("share", json!([opt("character", NL)])).await;
    assert_eq!(
        reply.content,
        "MikaNL isn’t linked. Link it first with `/character link`."
    );

    let offline = rig_with(false, true).await;
    let reply = offline.run("link", json!([opt("character", BISH)])).await;
    assert_eq!(
        reply.content,
        "Character data isn’t connected yet. Nothing was fetched or kept."
    );

    // NEXON has no such character: nothing is kept, not even the link.
    rig.nexon.missing.store(true, Ordering::SeqCst);
    let (_, link) = rig.notice().await;
    let reply = rig.press(&link).await;
    assert_eq!(
        words(&reply),
        [
            "NEXON couldn’t find **MikaBish** in MapleSEA. Check the IGN on the roster with an admin, then try again. Nothing was kept."
        ]
    );
    assert_eq!(rig.status(BISH).await, None);
}

#[tokio::test]
async fn data_shows_everything_kept_and_unlink_all_clears_it() {
    let rig = rig().await;
    let reply = rig.run("data", json!([])).await;
    assert_eq!(reply.content, "Nothing from NEXON is kept for you.");
    rig.link().await;
    let reply = rig.run("data", json!([])).await;
    let shown = words(&reply);
    let fetched = now().timestamp();
    let expires = (now() + TimeDelta::days(30)).timestamp();
    assert_eq!(
        shown,
        [
            "### What Kanade keeps for you".to_owned(),
            "Linked characters, from the NEXON Open API; their class and level are also copied to your roster characters, where they stay. Nothing else from NEXON is kept."
                .to_owned(),
            format!(
                "**MikaBish · linked**\nBishop · Lv. 283 · CP 48,203,114 · Aquila\nAs of 28 Sep 2026 (SGT) · fetched <t:{fetched}:f> · deleted by <t:{expires}:D> unless refreshed\nLinked <t:{fetched}:f>, notice 2"
            ),
            "**MikaNL · not linked**\nNothing fetched or kept.".to_owned(),
            format!("-# Remove one or all with `/character unlink`. [Privacy notice]({PRIVACY})"),
            format!("-# {ATTR}"),
        ]
    );
    assert!(!shown.join("\n").contains(OCID), "the ocid is never shown");
    let choices = rig.choices(ALICE, "unlink").await;
    assert_eq!(
        choices,
        [
            ("MikaBish · Bishop · linked".to_owned(), BISH.to_owned()),
            ("All".to_owned(), "all".to_owned()),
        ]
    );
    let reply = rig.run("unlink", json!([opt("character", "all")])).await;
    assert_eq!(
        reply.content,
        "Unlinked everything left: **MikaBish**. Nothing from NEXON is kept for you now; your roster characters keep their class and level."
    );
    assert_eq!(rig.status(BISH).await, None);
    let reply = rig.run("data", json!([])).await;
    assert_eq!(reply.content, "Nothing from NEXON is kept for you.");
}

/// An admin's link is active at once with no member confirmation; the
/// member's view names no notice for it, and `/character link` says it is
/// linked already.
#[tokio::test]
async fn an_admin_link_is_active_at_once_without_a_notice() {
    let rig = rig().await;
    let bob = Actor::admin(format!("discord:{BOB}"));
    let outcome = rig
        .service
        .admin_link(BISH, 0, &bob, Surface::AdminPortal)
        .await
        .unwrap();
    assert!(
        matches!(outcome, AdminLinkOutcome::Linked(_, FetchState::Stored)),
        "{outcome:?}"
    );
    assert_eq!(rig.status(BISH).await, Some(LinkStatus::Active));
    let again = rig.run("link", json!([opt("character", BISH)])).await;
    assert_eq!(
        again.content,
        "**MikaBish** is already linked. See it with `/character data`."
    );
    let shown = words(&rig.run("data", json!([])).await);
    let fetched = now().timestamp();
    assert!(
        shown[2].ends_with(&format!("unless refreshed\nLinked <t:{fetched}:f>")),
        "{shown:?}"
    );
    assert_eq!(
        rig.history().await,
        [(
            "unlinked".into(),
            "linked".into(),
            format!("admin:discord:{BOB}")
        )]
    );
    nexon_free(&rig).await;
}

#[tokio::test]
async fn share_posts_a_tracked_card_and_unlink_strips_it_at_once() {
    let rig = rig().await;
    rig.link().await;
    let reply = rig.run("share", json!([opt("character", BISH)])).await;
    assert!(reply.ephemeral);
    assert_eq!(
        reply.content,
        format!(
            "Posted in <#{KALOS}>. It turns into “Character data removed” after 29 days, or at once if you unlink."
        )
    );
    let posts = rig.posts().await;
    assert_eq!(posts.len(), 1);
    let post = &posts[0];
    assert_eq!(post.channel_id, KALOS.to_string());
    assert_eq!(post.strip_due_at, now() + TimeDelta::days(29));
    assert!(
        post.message_id.is_some(),
        "the card is bound to its message"
    );
    let card = texts(&rig.created());
    assert_eq!(
        card,
        [
            "-# user1001’s character".to_owned(),
            "### MikaBish".to_owned(),
            "**Class** Bishop · **Level** 283 · **Combat power** 48,203,114".to_owned(),
            "**As of** 28 Sep 2026 (SGT)".to_owned(),
            format!("-# {ATTR} · ref {}", post.reference),
        ]
    );

    let reply = rig.run("unlink", json!([opt("character", BISH)])).await;
    assert_eq!(
        reply.content,
        format!(
            "Unlinked **MikaBish**. Its NEXON data is deleted, and your 1 shared card in <#{KALOS}> now reads “Character data removed”. Your roster character keeps its class and level."
        )
    );
    assert_eq!(rig.status(BISH).await, None);
    assert!(rig.posts().await[0].is_due(now()), "due at once");
    rig.service.sweep().await;
    let edits = rig.edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(texts(&edits[0]), [STUB]);
    assert!(rig.posts().await.is_empty(), "forgotten once stripped");
    assert_eq!(
        rig.history().await.last().unwrap(),
        &(
            "linked".into(),
            "unlinked".into(),
            format!("member:{ALICE}")
        )
    );
    nexon_free(&rig).await;
}

#[tokio::test]
async fn an_ambiguous_create_is_found_again_by_its_ref() {
    let rig = rig().await;
    rig.link().await;
    rig.slash
        .discord
        .script(Op::Create, crate::support::timed_out(true));
    let hold = rig.slash.discord.hold(Op::ChannelMessages);
    let seed = async {
        hold.entered().await;
        let reference = rig.posts().await[0].reference.clone();
        let mut message = message_json(77_001, KALOS, Some(super::super::support::GUILD), "");
        message["author"] = user_json(BOT, "kanade", None, true);
        message["flags"] = json!(1 << 15);
        message["components"] = json!([{
            "type": 17,
            "components": [{ "type": 10, "content": format!("-# {ATTR} · ref {reference}") }],
        }]);
        rig.slash.discord.seed_history(vec![parse_message(message)]);
        hold.release();
    };
    let (reply, ()) = tokio::join!(rig.run("share", json!([opt("character", BISH)])), seed);
    assert!(reply.content.starts_with("Posted in"), "{reply:?}");
    let posts = rig.posts().await;
    assert_eq!(posts[0].message_id.as_deref(), Some("77001"));
    assert_eq!(rig.slash.discord.count(Op::Create), 1, "never sent twice");

    // Not found after two reads: unknown, for an admin.
    rig.slash
        .discord
        .script(Op::Create, crate::support::timed_out(false));
    let reply = rig.run("share", json!([opt("character", BISH)])).await;
    assert_eq!(
        reply.content,
        format!(
            "Kanade couldn’t confirm the card in <#{KALOS}>. If it was posted, the admins will remove it; nothing else was shared."
        )
    );
    assert_eq!(rig.slash.discord.count(Op::ChannelMessages), 3);
    let unknown = rig
        .posts()
        .await
        .into_iter()
        .find(|post| post.state == PostState::Unknown)
        .expect("an unknown post");
    let health = rig.service.health().await;
    assert_eq!(health.posts.unwrap().unknown, 1);
    assert!(
        health
            .post_issues
            .iter()
            .any(|issue| issue.state == "unknown"
                && issue.channel_id == KALOS.to_string()
                && issue.reference == unknown.reference)
    );

    // A channel Kanade can't post in: nothing kept.
    rig.slash
        .discord
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let reply = rig.run("share", json!([opt("character", BISH)])).await;
    assert_eq!(
        reply.content,
        format!("Kanade can’t post in <#{KALOS}>; nothing was shared.")
    );
    assert_eq!(rig.posts().await.len(), 2, "the refused card left no row");
}

/// A due post with a message, posted 29 days before the rig's clock.
async fn due_post(rig: &Rig, reference: &str, message: u64, exists: bool) {
    let mut post = Post::new(
        reference.into(),
        KALOS.to_string(),
        BISH.into(),
        now() - TimeDelta::days(29),
    );
    post.message_id = Some(message.to_string());
    rig.file.create_post(post).await.unwrap();
    if exists {
        rig.slash
            .discord
            .seed_v2_message(Id::new(KALOS), Id::new(message));
    }
}

#[tokio::test]
async fn strips_finish_on_404_delete_on_403_and_stick_when_both_are_refused() {
    let rig = rig().await;
    // Gone already: done, nothing edited.
    due_post(&rig, "aaaaa1", 5_001, false).await;
    rig.service.strip_posts().await;
    assert!(rig.posts().await.is_empty());

    // Edit refused: deleted instead.
    due_post(&rig, "aaaaa2", 5_002, true).await;
    rig.slash
        .discord
        .script(Op::Edit, Step::Reject(RejectionKind::MissingPermissions));
    rig.service.strip_posts().await;
    assert!(rig.posts().await.is_empty());
    assert!(
        !rig.slash
            .discord
            .messages()
            .iter()
            .any(|(_, id)| id.get() == 5_002),
        "deleted"
    );

    // Both refused: stuck, with a jump link for an admin.
    due_post(&rig, "aaaaa3", 5_003, true).await;
    rig.slash
        .discord
        .script(Op::Edit, Step::Reject(RejectionKind::MissingAccess));
    rig.slash
        .discord
        .script(Op::Delete, Step::Reject(RejectionKind::MissingAccess));
    rig.service.strip_posts().await;
    let posts = rig.posts().await;
    assert_eq!(posts[0].state, PostState::Stuck);
    let health = rig.service.health().await;
    assert_eq!(health.posts.unwrap().stuck, 1);
    let dto = kanade::api::dto::character_data::health(&health, Some("100"));
    let item = &dto.posts.unwrap().items[0];
    assert_eq!(item.state, "stuck");
    assert_eq!(
        item.jump_url.as_deref(),
        Some(format!("https://discord.com/channels/100/{KALOS}/5003").as_str())
    );
    // A stuck post is not tried again.
    let edits = rig.edits().len();
    rig.service.strip_posts().await;
    assert_eq!(rig.edits().len(), edits);
}

#[tokio::test]
async fn an_unconfirmed_strip_is_tried_again_in_5_minutes_then_hourly() {
    let rig = rig().await;
    due_post(&rig, "bbbbb1", 6_001, true).await;
    rig.slash
        .discord
        .set_default(Op::Edit, Some(crate::support::timed_out(false)));
    rig.service.strip_posts().await;
    assert_eq!(
        rig.service.next_strip_retry(),
        Some(now() + TimeDelta::minutes(5))
    );
    rig.service.strip_posts().await;
    assert_eq!(rig.edits().len(), 1, "waits for its retry");
    rig.set_now(now() + TimeDelta::minutes(5));
    rig.service.strip_posts().await;
    assert_eq!(rig.edits().len(), 2);
    assert_eq!(
        rig.service.next_strip_retry(),
        Some(now() + TimeDelta::minutes(65)),
        "then hourly"
    );
    rig.slash.discord.set_default(Op::Edit, None);
    rig.set_now(now() + TimeDelta::minutes(65));
    rig.service.strip_posts().await;
    assert!(rig.posts().await.is_empty());
    assert_eq!(rig.service.next_strip_retry(), None);
}

#[tokio::test]
async fn a_card_turns_into_the_stub_on_day_29_and_shows_overdue_on_day_30() {
    let rig = rig().await;
    rig.link().await;
    rig.run("share", json!([opt("character", BISH)])).await;
    rig.set_now(now() + TimeDelta::days(29) - TimeDelta::minutes(1));
    rig.service.sweep().await;
    assert!(rig.edits().is_empty(), "not due yet");
    rig.set_now(now() + TimeDelta::days(29));
    rig.service.sweep().await;
    assert_eq!(texts(&rig.edits()[0]), [STUB]);
    assert!(rig.posts().await.is_empty());

    // A strip that keeps failing is overdue in health from day 30.
    rig.set_now(now());
    rig.run("share", json!([opt("character", BISH)])).await;
    rig.slash
        .discord
        .set_default(Op::Edit, Some(Step::Reject(RejectionKind::NotSent)));
    rig.set_now(now() + TimeDelta::days(29));
    rig.service.sweep().await;
    assert_eq!(rig.service.health().await.posts.unwrap().overdue, 0);
    rig.set_now(now() + TimeDelta::days(30));
    let health = rig.service.health().await;
    assert_eq!(health.posts.unwrap().overdue, 1);
    assert_eq!(health.post_issues[0].state, "overdue");
}

#[tokio::test]
async fn a_waiting_link_expires_after_14_days() {
    let rig = rig().await;
    rig.waiting().await;
    rig.set_now(now() + TimeDelta::days(14) - TimeDelta::minutes(1));
    rig.service.sweep().await;
    assert_eq!(rig.status(BISH).await, Some(LinkStatus::Pending));
    rig.set_now(now() + TimeDelta::days(14));
    let report = rig.service.sweep().await;
    assert_eq!(report.expired, 1);
    assert_eq!(rig.status(BISH).await, None);
    assert_eq!(
        rig.history().await,
        [(
            "pending".into(),
            "unlinked".into(),
            "system:chardata".into()
        )]
    );
    assert_eq!(rig.nexon.calls.load(Ordering::SeqCst), 0);
}
