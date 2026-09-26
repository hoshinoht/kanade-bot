//! v4 reminder and digest cards through the tick (fake Discord): content,
//! fields, footer, colour, thumbnail and image per kind; art uploads and
//! their absence; quiet mode; the day-of heading rewrite and its reuse by
//! retries and reaction edits.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use kanade::bot::delivery::cards::{
    ArtKind, ArtSource, COLOUR_ALL_SET, COLOUR_COUNTDOWN, COLOUR_DIGEST, CardKit, DIGEST_FOOTER,
    HeadingRewrite, PersonaSource, REACT_HINT, UNNAMED,
};
use kanade::bot::delivery::{CardRefresh, SendOutcome};
use kanade::bot::transport::{
    Call, FakeDiscord, MessageEdit, Op, OutgoingMessage, RejectionKind, Step,
};
use kanade::chat::nudge::{NudgeRewriter, RewriteFailure, RewritePrompt, SharedRewriter};
use kanade::chat::persona::{CompiledPersona, PersonaId, PersonaRoot};
use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec, GuideSpec};
use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::{RandomIds, short_id};
use kanade::domain::members::{Member, PingLevel, Roster};
use kanade::domain::schedule::{EMOJI_NO, EMOJI_YES, NewRun, RunSource, RunStatus};
use kanade::infrastructure::files::BossArt;
use kanade::infrastructure::store::MemoryScheduleStore;
use twilight_model::channel::message::Embed;

use crate::scenarios::{self, HOME, World, now, previous_week, week};
use crate::support::{self, Store, TempDir, on_both_stores, with_lease};

const STAR_COLOUR: u32 = 0xF8DD4A;

fn catalog() -> BossTable {
    let difficulty = |prefix: &str, label: &str| DifficultySpec {
        prefix: prefix.into(),
        label: label.into(),
    };
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![
            difficulty("n", "Normal"),
            difficulty("h", "Hard"),
            difficulty("x", "Extreme"),
        ],
        bosses: vec![
            BossSpec {
                short: "Kalos".into(),
                full: Some("Gatekeeper Kalos".into()),
                level: Some(265),
                ..BossSpec::default()
            },
            BossSpec {
                short: "MaleficStar".into(),
                full: Some("Radiant Malefic Star".into()),
                level: Some(280),
                guide: Some(GuideSpec {
                    colour: Some(i64::from(STAR_COLOUR)),
                }),
                ..BossSpec::default()
            },
        ],
    })
    .expect("catalog")
}

/// Art on disk with exact-case names: both Malefic Star pictures, and only
/// Kalos's portrait.
fn art_dir() -> TempDir {
    let dir = TempDir::new();
    let write = |relative: &str, bytes: &[u8]| {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    write("portraits/MaleficStar.png", b"star-portrait");
    write("artwork/entry/MaleficStar.png", b"star-entry");
    write("portraits/Kalos.webp", b"kalos-portrait");
    dir
}

fn kit(art: Option<&TempDir>) -> CardKit {
    CardKit {
        catalog: Some(Arc::new(catalog())),
        art: art.map(|dir| Arc::new(BossArt::new(dir.path())) as Arc<dyn ArtSource>),
        heading: HeadingRewrite::default(),
    }
}

/// 1001 "Aria" wants every ping; 1002 "Bex" none (named, never tagged).
fn world() -> World {
    let mut roster = Roster::new();
    for (user, name, level) in [
        ("1001", "Aria", PingLevel::All),
        ("1002", "Bex", PingLevel::Off),
    ] {
        roster.upsert(Member {
            user_id: user.into(),
            display_name: Some(name.into()),
            has_role: true,
            ping_level: level,
            ..Member::default()
        });
    }
    World {
        roster,
        ..scenarios::world()
    }
}

async fn run<S: Store>(
    store: &S,
    bosses: &[&str],
    party: &[&str],
    at: DateTime<Utc>,
    status: RunStatus,
) -> String {
    let mut ids = RandomIds;
    support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: week(),
            datetime: at,
            bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
            participants: party.iter().map(|user| (*user).to_owned()).collect(),
            status,
            source: RunSource::Amend,
        })
        .await
        .expect("run")
}

async fn due<S: Store>(store: &S, run: &str, kind: &str) {
    let mut ids = RandomIds;
    support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .add_reminder(run, kind, now() - TimeDelta::minutes(1), None)
        .await
        .expect("reminder")
        .expect("new reminder");
}

async fn answer<S: Store>(store: &S, run: &str, user: &str, yes: bool, at: DateTime<Utc>) {
    let mut ids = RandomIds;
    support::service(store, &mut ids, at)
        .as_origin(Origin::new(Actor::member(user), Surface::Discord))
        .apply_reaction(run, user, if yes { EMOJI_YES } else { EMOJI_NO }, true)
        .await
        .expect("reaction");
}

fn created(fake: &FakeDiscord) -> Vec<OutgoingMessage> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Create { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

fn edits(fake: &FakeDiscord) -> Vec<MessageEdit> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Edit { edit, .. } => Some(edit),
            _ => None,
        })
        .collect()
}

fn fields(embed: &Embed) -> Vec<(String, String)> {
    embed
        .fields
        .iter()
        .map(|field| (field.name.clone(), field.value.clone()))
        .collect()
}

fn pictures(embed: &Embed) -> (Option<String>, Option<String>) {
    (
        embed.thumbnail.as_ref().map(|thumb| thumb.url.clone()),
        embed.image.as_ref().map(|image| image.url.clone()),
    )
}

fn uploads(message: &OutgoingMessage) -> Vec<(String, Vec<u8>)> {
    message
        .attachments
        .iter()
        .map(|file| (file.filename.clone(), file.bytes.to_vec()))
        .collect()
}

fn allowed(message: &OutgoingMessage) -> Vec<String> {
    message
        .allowed_mentions
        .users
        .iter()
        .map(|id| id.get().to_string())
        .collect()
}

/// Thu 10 Sep 21:00 and 22:30 in Kuala Lumpur.
fn tonight() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 10, 13, 0, 0).unwrap()
}

/// Two runs tonight; the later is own time. Returns their ids.
async fn seed_day_of<S: Store>(store: &S) -> (String, String) {
    let star = run(
        store,
        &["HMaleficStar"],
        &["1001", "1002"],
        tonight(),
        RunStatus::Planned,
    )
    .await;
    let kalos = run(
        store,
        &["XKalos"],
        &["1001"],
        tonight() + TimeDelta::minutes(90),
        RunStatus::Otot,
    )
    .await;
    due(store, &star, "day_of").await;
    due(store, &kalos, "day_of").await;
    (star, kalos)
}

const DAY_OF_CONTENT: &str = "📅 **Today — Thu 10 Sep**\n<@1001> Bex";

async fn day_of_card_matches_v4<S: Store>(store: &S) {
    let art = art_dir();
    let world = world();
    seed_day_of(store).await;
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_cards(kit(Some(&art)));
    let report = delivery.dispatch_reminders(now()).await.expect("dispatch");
    assert!(matches!(
        report.sends.as_slice(),
        [send] if matches!(send.outcome, SendOutcome::Bound(_))
    ));
    let posts = created(&world.fake);
    let [message] = posts.as_slice() else {
        panic!("one morning card: {posts:?}");
    };
    assert_eq!(message.content.as_deref(), Some(DAY_OF_CONTENT));
    let embed = &message.embeds[0];
    assert_eq!(
        fields(embed),
        [
            (
                "🕘 21:00  ·  HMaleficStar".to_owned(),
                "**HMaleficStar** · Radiant Malefic Star (Hard, Lv280)\n⚠️ unconfirmed · 0/2 ✅\n<@1001> Bex"
                    .to_owned()
            ),
            (
                "🕒 own time  ·  XKalos".to_owned(),
                "**XKalos** · Gatekeeper Kalos (Extreme, Lv265)\n🕒 own time · 0/1 ✅\n<@1001>"
                    .to_owned()
            ),
        ]
    );
    assert_eq!(embed.description, None);
    assert_eq!(
        embed.footer.as_ref().map(|f| f.text.as_str()),
        Some(REACT_HINT)
    );
    assert_eq!(embed.color, Some(STAR_COLOUR), "the lead boss's colour");
    assert_eq!(
        pictures(embed),
        (
            Some("attachment://MaleficStar.png".to_owned()),
            Some("attachment://image-MaleficStar.png".to_owned())
        ),
        "mixed-case art keys resolve to exact-case files"
    );
    assert_eq!(
        uploads(message),
        [
            ("MaleficStar.png".to_owned(), b"star-portrait".to_vec()),
            ("image-MaleficStar.png".to_owned(), b"star-entry".to_vec()),
        ]
    );
    assert_eq!(allowed(message), ["1001"], "the allow-list is the intent's");
    assert_eq!(report.sends[0].intent.mentions, ["1001"]);
}

#[tokio::test]
async fn day_of_card_pins_v4_content_fields_and_art() {
    on_both_stores!(day_of_card_matches_v4);
}

/// A countdown for a fresh Kalos run 14 minutes out with `answers`.
async fn countdown<S: Store>(
    store: &S,
    world: &World,
    kit: CardKit,
    answers: &[(&str, bool)],
) -> OutgoingMessage {
    let id = run(
        store,
        &["XKalos"],
        &["1001", "1002"],
        now() + TimeDelta::minutes(14),
        RunStatus::Planned,
    )
    .await;
    for (user, yes) in answers {
        answer(store, &id, user, *yes, now() - TimeDelta::minutes(5)).await;
    }
    due(store, &id, "countdown_15").await;
    let mut delivery = scenarios::delivery(store, world, &world.fake).with_cards(kit);
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    created(&world.fake).pop().expect("countdown posted")
}

const KALOS_DETAIL: &str = "**XKalos** · Gatekeeper Kalos (Extreme, Lv265)";

async fn countdown_states_match_v4<S: Store>(store: &S) {
    let art = art_dir();
    let world = world();

    let pending = countdown(store, &world, kit(Some(&art)), &[]).await;
    assert_eq!(
        pending.content.as_deref(),
        Some("⏰ **XKalos** in 15m (20:14) — <@1001> Bex")
    );
    let embed = &pending.embeds[0];
    assert_eq!(
        embed.description.as_deref(),
        Some(
            format!("{KALOS_DETAIL}\n⚠️ unconfirmed · 0/2 ✅\nStill to answer: <@1001> Bex")
                .as_str()
        )
    );
    assert_eq!(
        embed.footer.as_ref().map(|f| f.text.as_str()),
        Some(REACT_HINT)
    );
    assert_eq!(embed.color, Some(COLOUR_COUNTDOWN));
    assert_eq!(
        pictures(embed),
        (Some("attachment://Kalos.webp".to_owned()), None),
        "a thumbnail only; no entry art on countdowns"
    );
    assert_eq!(
        uploads(&pending),
        [("Kalos.webp".to_owned(), b"kalos-portrait".to_vec())]
    );
    assert_eq!(allowed(&pending), ["1001"]);

    let out = countdown(
        store,
        &world,
        kit(Some(&art)),
        &[("1001", true), ("1002", false)],
    )
    .await;
    assert_eq!(
        out.content.as_deref(),
        Some("⏰ **XKalos** in 15m (20:14) — <@1001> · Bex out")
    );
    let embed = &out.embeds[0];
    assert_eq!(
        embed.description.as_deref(),
        Some(format!("{KALOS_DETAIL}\n❗ at risk · 1/2 ✅ · 1 ❌").as_str())
    );
    assert_eq!(embed.footer, None, "nothing left to ask");
    assert_eq!(embed.color, Some(COLOUR_COUNTDOWN), "not settled either");
    assert_eq!(allowed(&out), ["1001"], "the decliner is not pinged");

    let set = countdown(
        store,
        &world,
        kit(Some(&art)),
        &[("1001", true), ("1002", true)],
    )
    .await;
    assert_eq!(
        set.content.as_deref(),
        Some("⏰ **XKalos** in 15m (20:14) — everyone's confirmed ✅")
    );
    let embed = &set.embeds[0];
    assert_eq!(
        embed.description.as_deref(),
        Some(format!("{KALOS_DETAIL}\n✅ confirmed · 2/2 ✅").as_str())
    );
    assert_eq!(embed.footer, None);
    assert_eq!(embed.color, Some(COLOUR_ALL_SET));
}

#[tokio::test]
async fn countdown_cards_pin_pending_out_and_all_set() {
    on_both_stores!(countdown_states_match_v4);
}

async fn digest_matches_v4<S: Store>(store: &S) {
    let art = art_dir();
    let world = world();
    let kalos = run(store, &["XKalos"], &["1001"], tonight(), RunStatus::Planned).await;
    let star = run(
        store,
        &["HMaleficStar"],
        &["1002"],
        tonight() + TimeDelta::days(2),
        RunStatus::Otot,
    )
    .await;
    with_lease(store, now(), async |lease| {
        store
            .record_digest_week(lease, previous_week(), now())
            .await
            .expect("digest week");
    })
    .await;
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_cards(kit(Some(&art)));
    delivery.post_week_digest(now()).await.expect("digest");
    let posts = created(&world.fake);
    let [message] = posts.as_slice() else {
        panic!("one digest: {posts:?}");
    };
    assert_eq!(
        message.content.as_deref(),
        Some("🗓️ Boss week of Wed 09 Sep")
    );
    let embed = &message.embeds[0];
    assert_eq!(
        embed.description.as_deref(),
        Some("**0/2 Cleared** · 2 run(s) across 2 day(s) · **1** still unconfirmed ⚠️")
    );
    assert_eq!(
        fields(embed),
        [
            (
                "Thu 10 Sep".to_owned(),
                format!(
                    "**XKalos** · ⚠️ **Planned**\n`21:00` · 0/1 ✅ · <#{HOME}> · `#{}`",
                    short_id(&kalos)
                )
            ),
            (
                "Sat 12 Sep".to_owned(),
                format!(
                    "**HMaleficStar** · 🕒 **Own time**\n`own time` · 0/1 ✅ · <#{HOME}> · `#{}`",
                    short_id(&star)
                )
            ),
        ]
    );
    assert_eq!(
        embed.footer.as_ref().map(|f| f.text.as_str()),
        Some(DIGEST_FOOTER)
    );
    assert_eq!(embed.color, Some(COLOUR_DIGEST));
    assert_eq!(
        pictures(embed),
        (None, None),
        "v4: the digest carries no art"
    );
    assert!(message.attachments.is_empty());
    assert!(allowed(message).is_empty(), "the digest names, never pings");
}

#[tokio::test]
async fn digest_card_pins_v4_summary_and_days() {
    on_both_stores!(digest_matches_v4);
}

/// Finds art but cannot read it.
struct Unreadable;

impl ArtSource for Unreadable {
    fn locate(&self, _kind: ArtKind, basename: &str) -> Option<String> {
        Some(format!("{basename}.png"))
    }

    fn read(&self, _kind: ArtKind, _file_name: &str) -> Option<Vec<u8>> {
        None
    }
}

async fn missing_art_posts_without_pictures<S: Store>(store: &S) {
    let world = world();
    let empty = TempDir::new();
    for kit in [
        kit(None),
        kit(Some(&empty)),
        CardKit {
            art: Some(Arc::new(Unreadable)),
            ..kit(None)
        },
    ] {
        let message = countdown(store, &world, kit, &[]).await;
        assert_eq!(pictures(&message.embeds[0]), (None, None));
        assert!(message.attachments.is_empty());
    }
    let calls = world.fake.calls();
    assert!(
        calls
            .iter()
            .filter(|call| call.op() == Op::Create)
            .all(|call| matches!(call, Call::Create { outcome, .. } if outcome.is_delivered())),
        "still posted"
    );
}

#[tokio::test]
async fn missing_or_unreadable_art_never_fails_the_send() {
    on_both_stores!(missing_art_posts_without_pictures);
}

async fn quiet_cards_tag_nobody<S: Store>(store: &S) {
    let world = world();
    let id = run(
        store,
        &["XKalos"],
        &["1001", "1003"],
        now() + TimeDelta::minutes(14),
        RunStatus::Planned,
    )
    .await;
    due(store, &id, "countdown_15").await;
    let mut delivery = scenarios::delivery(store, &world, &world.fake).with_cards(kit(None));
    delivery.config.quiet_mode = true;
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let message = created(&world.fake).pop().expect("posted");
    assert_eq!(
        message.content.as_deref(),
        Some(format!("⏰ **XKalos** in 15m (20:14) — Aria {UNNAMED}").as_str())
    );
    let description = message.embeds[0].description.clone().unwrap_or_default();
    assert!(!description.contains("<@"), "{description}");
    assert_eq!(
        serde_json::to_value(&message.allowed_mentions).unwrap(),
        serde_json::json!({ "parse": [] })
    );
}

#[tokio::test]
async fn quiet_mode_cards_carry_no_mention_tags() {
    on_both_stores!(quiet_cards_tag_nobody);
}

// Day-of heading rewrite (memory store: paused time needs no real I/O).

#[derive(Clone)]
enum Script {
    Reply(&'static str),
    Fail,
    Hang,
}

struct Scripted {
    script: Script,
    calls: AtomicUsize,
    prompts: Mutex<Vec<(String, String)>>,
}

impl Scripted {
    fn new(script: Script) -> Arc<Self> {
        Arc::new(Self {
            script,
            calls: AtomicUsize::new(0),
            prompts: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl NudgeRewriter for Scripted {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        _deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.prompts
            .lock()
            .unwrap()
            .push((prompt.system().to_owned(), prompt.seed().to_owned()));
        match self.script {
            Script::Reply(text) => Ok(text.to_owned()),
            Script::Fail => Err(RewriteFailure::Unavailable),
            Script::Hang => std::future::pending().await,
        }
    }
}

/// The tracked Kanade bundle, copied into a temp persona root.
fn persona() -> PersonaSource {
    let dir = TempDir::new();
    let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("config/personas/bundles/kanade.yaml");
    std::fs::create_dir_all(dir.path().join("personas/bundles")).unwrap();
    std::fs::copy(bundle, dir.path().join("personas/bundles/kanade.yaml")).unwrap();
    let root = PersonaRoot::open(&dir.path().join("personas")).expect("persona root");
    let id = PersonaId::parse("kanade").expect("id");
    let compiled = CompiledPersona::compile(&root.load_bundle(&id).expect("bundle").value, None);
    let compiled = Arc::new(compiled);
    Arc::new(move || Some((*compiled).clone()))
}

fn rewriting(rewriter: &Arc<Scripted>) -> CardKit {
    CardKit {
        heading: HeadingRewrite {
            rewriter: Some(SharedRewriter(rewriter.clone())),
            persona: Some(persona()),
        },
        ..kit(None)
    }
}

async fn morning_content(kit: CardKit) -> Option<String> {
    let store = MemoryScheduleStore::new();
    let world = world();
    seed_day_of(&store).await;
    let mut delivery = scenarios::delivery(&store, &world, &world.fake).with_cards(kit);
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    created(&world.fake)
        .pop()
        .and_then(|message| message.content)
}

#[tokio::test]
async fn an_accepted_rewrite_replaces_the_heading_with_the_day_filled_in() {
    let rewriter = Scripted::new(Script::Reply("Rise and shine, it's {day}!"));
    let content = morning_content(rewriting(&rewriter)).await;
    assert_eq!(
        content.as_deref(),
        Some("📅 **Rise and shine, it's Thu 10 Sep!**\n<@1001> Bex")
    );
    assert_eq!(rewriter.calls(), 1);
    let prompts = rewriter.prompts.lock().unwrap().clone();
    let (system, seed) = &prompts[0];
    assert_eq!(seed, "Today — {day}", "the day is left for the model");
    for private in [
        "Kalos", "Malefic", "1001", "1002", "Aria", "Bex", "Thu", "10 Sep", "21:00", HOME,
    ] {
        assert!(
            !system.contains(private) && !seed.contains(private),
            "the prompt must not carry {private}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn failure_timeout_rejection_or_no_rewriter_keep_the_v4_heading() {
    for (script, calls) in [
        (Script::Fail, 1),
        (Script::Hang, 1),
        (Script::Reply("**Wake up**, {day}"), 1),
        (Script::Reply("Good morning"), 1),
        (Script::Reply("See you at {time} on {day}"), 1),
    ] {
        let rewriter = Scripted::new(script);
        let started = tokio::time::Instant::now();
        assert_eq!(
            morning_content(rewriting(&rewriter)).await.as_deref(),
            Some(DAY_OF_CONTENT)
        );
        assert_eq!(rewriter.calls(), calls);
        assert!(started.elapsed() <= Duration::from_millis(2_100), "bounded");
    }
    assert_eq!(
        morning_content(kit(None)).await.as_deref(),
        Some(DAY_OF_CONTENT)
    );
    let no_persona = CardKit {
        heading: HeadingRewrite {
            rewriter: Some(SharedRewriter(Scripted::new(Script::Reply("x {day}")))),
            persona: None,
        },
        ..kit(None)
    };
    assert_eq!(
        morning_content(no_persona).await.as_deref(),
        Some(DAY_OF_CONTENT)
    );
}

#[tokio::test]
async fn retries_and_reaction_edits_reuse_the_stored_heading() {
    let art = art_dir();
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(support::fake());
    let (star, _) = seed_day_of(&*store).await;
    let rewriter = Scripted::new(Script::Reply("Rise and shine, it's {day}!"));
    let cards = CardKit {
        art: Some(Arc::new(BossArt::new(art.path()))),
        ..rewriting(&rewriter)
    };
    let expected = "📅 **Rise and shine, it's Thu 10 Sep!**\n<@1001> Bex";
    // The first send never reaches Discord; the retry posts.
    fake.script(Op::Create, Step::Reject(RejectionKind::NotSent));
    let mut delivery = scenarios::delivery(&*store, &world, &*fake).with_cards(cards.clone());
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    delivery
        .dispatch_reminders(now() + TimeDelta::seconds(30))
        .await
        .expect("retry");
    let posts = created(&fake);
    assert_eq!(posts.len(), 2);
    assert!(
        posts
            .iter()
            .all(|post| post.content.as_deref() == Some(expected))
    );
    assert_eq!(rewriter.calls(), 1, "one rewrite per card");

    // 1002 answers ✅: the card is edited in place from current answers.
    answer(&*store, &star, "1002", true, now() + TimeDelta::minutes(1)).await;
    let refresh = CardRefresh {
        store: Arc::clone(&store),
        transport: Arc::clone(&fake),
        members: Arc::new(world.roster.clone()),
        cards,
        policy: scenarios::config().policy,
        quiet: Arc::new(AtomicBool::new(false)),
        now: Arc::new(|| now() + TimeDelta::minutes(1)),
    };
    assert_eq!(refresh.refresh(std::slice::from_ref(&star)).await, 1);
    let edited = edits(&fake);
    let [edit] = edited.as_slice() else {
        panic!("one edit: {edited:?}");
    };
    assert_eq!(edit.content.as_deref(), Some(expected));
    let embed = &edit.embeds.as_ref().expect("embed")[0];
    assert!(
        embed.fields[0].value.contains("⚠️ unconfirmed · 1/2 ✅"),
        "{:?}",
        embed.fields[0]
    );
    assert_eq!(
        pictures(embed),
        (
            Some("attachment://MaleficStar.png".to_owned()),
            Some("attachment://image-MaleficStar.png".to_owned())
        ),
        "the edit keeps referring to the posted files"
    );
    assert_eq!(
        serde_json::to_value(&edit.allowed_mentions).unwrap(),
        serde_json::json!({ "parse": [] }),
        "an edit notifies nobody"
    );
    assert_eq!(rewriter.calls(), 1, "the edit reuses the stored heading");

    // A run that has started keeps its card as a record.
    let later = CardRefresh {
        now: Arc::new(|| tonight() + TimeDelta::minutes(1)),
        ..refresh
    };
    assert_eq!(later.refresh(std::slice::from_ref(&star)).await, 0);
}
