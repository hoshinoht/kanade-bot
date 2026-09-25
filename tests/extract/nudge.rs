//! Self-service nudges: seed rotation, the governed rewrite with seed
//! fallback, the rewrite prompt's contents, and one tip per member per week.
//! The rewriter here admits through the real governor (`try_acquire` on the
//! rewrite role, one request) and scripts the model's answer.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, TimeZone, Utc};
use kanade::chat::nudge::{
    DENY_INSIDE, DENY_SEA, DENY_SOUNDALIKE, EDIT_RUN_ACTION, GENTLE_MOOD, LineSource, MAX_CHANNELS,
    NUDGE_REWRITE_INSTRUCTION, NoRewrite, Nudge, NudgeFacts, NudgeRewriter, Nudger, PLAYFUL_MOOD,
    RECENT_PER_CHANNEL, REQUEST_CHANGE_ACTION, Rejection, RewriteFailure, RewritePrompt,
    SeedReason, SeedRotation, accept_rewrite, denied_word, has_invite, has_markup, mood_for,
    render,
};
use kanade::chat::persona::{
    CompiledPersona, NudgeMood, NudgePurpose, NudgeSource, PersonaId, ProfileId, parse_bundle,
    parse_profile,
};
use kanade::domain::notify::WeekReset;
use kanade::infrastructure::llm::governor::{
    CallKind, Governor, GovernorConfig, GovernorPolicy, GroupConfig, Outcome, Random, Role,
    RoleConfig, XorShift,
};
use kanade::infrastructure::llm::identity::{Member, find_leaks};
use kanade::infrastructure::store::MemoryScheduleStore;
use tokio::time::Instant;

const CHANNEL: &str = "900000000000000001";
const MEMBER: &str = "123456789012345678";

struct Fixed(u64);

impl Random for Fixed {
    fn next_u64(&self) -> u64 {
        self.0
    }
}

enum Step {
    Reply(&'static str),
    Hang,
    Fail,
    /// The provider's content filter or a refusal.
    Refuse,
}

struct GovernedFake {
    governor: Arc<Governor>,
    script: Mutex<VecDeque<Step>>,
    prompts: Mutex<Vec<RewritePrompt>>,
    sent: AtomicUsize,
}

impl GovernedFake {
    fn new(governor: Arc<Governor>, script: Vec<Step>) -> Self {
        Self {
            governor,
            script: Mutex::new(script.into()),
            prompts: Mutex::new(Vec::new()),
            sent: AtomicUsize::new(0),
        }
    }

    fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }
}

impl NudgeRewriter for GovernedFake {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        _deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        let permit = self
            .governor
            .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
            .map_err(|_| RewriteFailure::Unavailable)?;
        let attempt = permit
            .try_begin_request()
            .map_err(|_| RewriteFailure::Unavailable)?;
        self.sent.fetch_add(1, Ordering::SeqCst);
        self.prompts.lock().unwrap().push(prompt.clone());
        let step = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Fail);
        match step {
            Step::Reply(text) => {
                attempt.finish(Outcome::Success);
                Ok(text.to_owned())
            }
            Step::Hang => {
                tokio::time::sleep(Duration::from_secs(60)).await;
                attempt.finish(Outcome::Success);
                Ok("far too late".to_owned())
            }
            Step::Fail => {
                attempt.finish(Outcome::TransientFailure);
                Err(RewriteFailure::Unavailable)
            }
            Step::Refuse => {
                attempt.finish(Outcome::Success);
                Err(RewriteFailure::Refused)
            }
        }
    }
}

fn governor() -> Arc<Governor> {
    let route = RoleConfig {
        alias: "tiny".into(),
        external: false,
    };
    let config = GovernorConfig {
        groups: vec![GroupConfig {
            name: "rewrite".into(),
            backend: "local small model".into(),
            permits: 1,
            requests_per_min: 6_000,
            burst: Some(1_000),
            aliases: vec!["tiny".into()],
        }],
        roles: [Role::Chat, Role::Extraction, Role::Rewrite]
            .into_iter()
            .map(|role| (role, route.clone()))
            .collect::<BTreeMap<_, _>>(),
        policy: GovernorPolicy::default(),
    };
    Arc::new(Governor::new(&config, Arc::new(Fixed(0))).expect("valid config"))
}

fn bundle_yaml(extra: &str) -> String {
    format!(
        "schema_version: 1
id: alpha
identity: |
  # Persona: Alpha

  Synthetic identity for Alpha.
behaviour:
  voice: Bright and bouncy.
  prompt: |
    Synthetic behaviour for alpha.
staging:
  schedule: alpha schedule
  guide: alpha guide
  guide_named: '{{boss}} alpha guide'
  write: alpha write
  generic: alpha generic
{extra}"
    )
}

const SEEDS: &str = "nudges:
  playful:
    - 'Move {boss} to {day} {time} yourself.'
    - 'You can shift {boss} on your own.'
    - 'Go on, {boss} is all yours.'
  gentle:
    - 'Oh no. You can fix {boss} here.'
    - 'It happens. Adjust {boss} here.'
    - 'No worries, {boss} is easy to fix.'
compact:
  nudge_rewrite: Rewrite as Alpha, a cheerful idol.
";

fn persona(extra: &str) -> CompiledPersona {
    let id = PersonaId::parse("alpha").unwrap();
    CompiledPersona::compile(&parse_bundle(&bundle_yaml(extra), &id).unwrap(), None)
}

fn facts(mood: NudgeMood) -> NudgeFacts<'static> {
    NudgeFacts {
        channel_id: CHANNEL,
        purpose: NudgePurpose::SelfService,
        mood,
        boss: "Hard Lucid",
        day: "Sat",
        time: "21:00",
    }
}

async fn one(fake: &GovernedFake, persona: &CompiledPersona) -> (Nudge, Duration) {
    let nudger = Nudger::new(Arc::new(Fixed(0)), fake);
    let started = Instant::now();
    let nudge = nudger.lead_in(persona, &facts(NudgeMood::Playful)).await;
    (nudge, started.elapsed())
}

const SEED_FILLED: &str = "Move Hard Lucid to Sat 21:00 yourself.";

#[tokio::test(start_paused = true)]
async fn a_busy_rewrite_group_gives_the_seed_at_once() {
    let governor = governor();
    let _held = governor
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "other nudge")
        .unwrap();
    let fake = GovernedFake::new(governor.clone(), vec![Step::Reply("unused")]);
    let (nudge, elapsed) = one(&fake, &persona(SEEDS)).await;
    assert_eq!(elapsed, Duration::ZERO);
    assert_eq!(nudge.lead_in, SEED_FILLED);
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Unavailable));
    assert_eq!(fake.sent(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_slow_rewrite_is_abandoned_at_two_seconds_and_frees_the_permit() {
    let governor = governor();
    let fake = GovernedFake::new(
        governor.clone(),
        vec![Step::Hang, Step::Reply("Hi {boss}, {day} {time}!")],
    );
    let persona = persona(SEEDS);
    let (nudge, elapsed) = one(&fake, &persona).await;
    assert_eq!(elapsed, Duration::from_secs(2));
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::TimedOut));
    assert_eq!(nudge.lead_in, SEED_FILLED);
    // The abandoned call released its permit; the next nudge can rewrite.
    let (next, _) = one(&fake, &persona).await;
    assert_eq!(next.line, LineSource::Rewritten);
    assert_eq!(next.lead_in, "Hi Hard Lucid, Sat 21:00!");
}

#[tokio::test(start_paused = true)]
async fn a_failed_rewrite_or_no_rewrite_role_gives_the_seed() {
    let fake = GovernedFake::new(governor(), vec![Step::Fail]);
    let (nudge, _) = one(&fake, &persona(SEEDS)).await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Unavailable));
    assert_eq!(nudge.lead_in, SEED_FILLED);

    let nudger = Nudger::new(Arc::new(Fixed(0)), NoRewrite);
    let nudge = nudger
        .lead_in(&persona(SEEDS), &facts(NudgeMood::Playful))
        .await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Unavailable));
}

#[tokio::test(start_paused = true)]
async fn a_provider_refusal_or_content_filter_gives_the_seed() {
    let fake = GovernedFake::new(governor(), vec![Step::Refuse]);
    let (nudge, _) = one(&fake, &persona(SEEDS)).await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Refused));
    assert_eq!(nudge.lead_in, SEED_FILLED);
    assert_eq!(fake.sent(), 1);
}

#[tokio::test(start_paused = true)]
async fn invalid_rewrites_fall_back_to_the_seed() {
    let long: &'static str = "a".repeat(141).leak();
    let invalid: [&'static str; 11] = [
        "Hey <@123456789012345678>, move {boss} {day} {time}",
        "Move {boss} {day} {time} at https://example.com",
        "Move {boss} {day} {time} at www.example.com",
        "Move {boss} [here](x) {day} {time}",
        "Move {boss} {day} {time}\nand another line",
        "@everyone move {boss} {day} {time}",
        "Move {boss} for {member} {day} {time}",
        "Move {boss} yourself.",
        long,
        "",
        "Move {boss} {day} {time} <#1234>",
    ];
    let persona = persona(SEEDS);
    for output in invalid {
        let fake = GovernedFake::new(governor(), vec![Step::Reply(output)]);
        let (nudge, _) = one(&fake, &persona).await;
        assert!(
            matches!(nudge.line, LineSource::Seed(SeedReason::Rejected(_))),
            "{output:?} must be rejected"
        );
        assert_eq!(nudge.lead_in, SEED_FILLED);
    }
}

#[tokio::test(start_paused = true)]
async fn a_valid_rewrite_is_trimmed_filled_and_used() {
    let fake = GovernedFake::new(
        governor(),
        vec![Step::Reply(
            "  Ehh, drag {boss} to {day} {time} yourself! \n",
        )],
    );
    let (nudge, _) = one(&fake, &persona(SEEDS)).await;
    assert_eq!(nudge.line, LineSource::Rewritten);
    assert_eq!(nudge.lead_in, "Ehh, drag Hard Lucid to Sat 21:00 yourself!");
    assert_eq!(nudge.seeds, NudgeSource::Bundle);
    assert_eq!(fake.sent(), 1);
}

#[test]
fn accepted_rewrites_keep_exactly_the_seed_placeholder_multiset() {
    let seed = "Move {boss} yourself.";
    assert_eq!(
        accept_rewrite("Go move {boss}!", seed).as_deref(),
        Ok("Go move {boss}!")
    );
    let placeholders = Err(Rejection::Placeholders);
    assert_eq!(accept_rewrite("Go move it!", seed), placeholders);
    assert_eq!(accept_rewrite("Move {boss} on {day}", seed), placeholders);
    assert_eq!(accept_rewrite("{boss}, move {boss}!", seed), placeholders);
    assert_eq!(
        accept_rewrite("{boss} and {boss} again", "{boss} or {boss}").as_deref(),
        Ok("{boss} and {boss} again")
    );
    assert_eq!(
        accept_rewrite("Plain line.", "Plain seed."),
        Ok("Plain line.".into())
    );
}

#[test]
fn markdown_format_characters_and_invites_are_rejected() {
    let seed = "Move {boss} yourself.";
    let cases = [
        "# Move {boss} yourself.",
        "-# Move {boss} yourself.",
        "> Move {boss} yourself.",
        ">>> Move {boss} yourself.",
        "- Move {boss} yourself.",
        "* Move {boss} yourself.",
        "1. Move {boss} yourself.",
        "12. Move {boss} yourself.",
        "Move `{boss}` yourself.",
        "Move *{boss}* yourself.",
        "Move __{boss}__ yourself.",
        "Move ~~{boss}~~ yourself.",
        "Move ||{boss}|| yourself.",
        "Move \\{boss} yourself.",
        "Move {boss} your\u{200B}self.",
        "Move {boss} \u{202E}yourself.",
        "Move {boss} yourself\u{2066}.",
        "Move {boss} yourself\u{FEFF}.",
        "Move {boss} at discord.gg/abc",
        "Move {boss} at DISCORD.com/invite/abc",
        "Move {boss} at discordapp.com/Invite/abc",
    ];
    for output in cases {
        assert_eq!(
            accept_rewrite(output, seed),
            Err(Rejection::Markup),
            "{output:?}"
        );
    }
    // Plain punctuation and hyphenated words are fine.
    assert!(accept_rewrite("Re-plan {boss} yourself - quick!", seed).is_ok());
    assert!(accept_rewrite("2 hours? Move {boss} yourself.", seed).is_ok());
}

#[test]
fn a_single_tilde_is_voice_but_a_double_tilde_is_strikethrough() {
    let seed = "Move {boss} yourself.";
    assert_eq!(
        accept_rewrite("Moving {boss} is on you~", seed).as_deref(),
        Ok("Moving {boss} is on you~")
    );
    assert!(accept_rewrite("Move {boss}~ yourself~", seed).is_ok());
    for output in [
        "Move ~~{boss}~~ yourself.",
        "Move {boss} yourself~~",
        "~~Move~~ {boss} yourself.",
    ] {
        assert_eq!(
            accept_rewrite(output, seed),
            Err(Rejection::Markup),
            "{output:?}"
        );
    }
    assert!(!has_markup("on you~") && has_markup("on you~~"));
}

#[test]
fn slurs_and_profanity_are_caught_inside_words_and_with_the_in_suffix() {
    let seed = "Move {boss} yourself.";
    for (output, entry) in [
        ("Move {boss} yourself, bullshit.", "shit"),
        ("Move {boss} yourself, motherfucker.", "fuck"),
        ("Move {boss} yourself, shithead.", "shit"),
        ("Move {boss} yourself, fuckin slowpoke.", "fuck"),
        ("Move {boss} yourself, fvcking... no, fuuckin.", "fvck"),
        ("Move {boss} yourself, bitchin.", "bitch"),
        ("Move {boss} yourself, sh1tty.", "shit"),
        ("Move {boss} yourself, n1ggers.", "nigger"),
        ("Move {boss} yourself, niga? no: nigerz.", "nigga"),
        ("Move {boss} yourself, fagg0ts.", "faggot"),
        ("Move {boss} yourself, fagot.", "faggot"),
        ("Move {boss} yourself, dumbfagot.", "fagot"),
    ] {
        assert_eq!(
            accept_rewrite(output, seed),
            Err(Rejection::Denied(entry)),
            "{output:?}"
        );
    }
    // `cunt` is whole-word only, so this town survives; the whole word does not.
    assert!(accept_rewrite("Move {boss} yourself, Scunthorpe.", seed).is_ok());
    assert_eq!(
        accept_rewrite("Move {boss} yourself, cunt.", seed),
        Err(Rejection::Denied("cunt"))
    );
    for clean in [
        "Move {boss} yourself, shift it.",
        "Move {boss} yourself, it's fun.",
        "Move {boss} yourself, begin now.",
    ] {
        assert!(accept_rewrite(clean, seed).is_ok(), "{clean:?}");
    }
    assert!(DENY_INSIDE.contains(&"fuck"));
}

#[test]
fn the_deny_list_matches_whole_words_through_simple_obfuscation() {
    let seed = "Move {boss} yourself.";
    for output in [
        "Move {boss} yourself, sexy.",
        "Move {boss} yourself, SEXY.",
        "Move {boss} yourself, s3xy.",
        "Move {boss} yourself, fuuuuck.",
        "Move {boss} yourself, sh1t.",
        "Move {boss} yourself, $hitty.",
        "Move {boss} yourself, f.u.c.k.i.n.g lewd.",
        "Move {boss} yourself, n00dz? no: nude.",
        "Move {boss} yourself, horny.",
    ] {
        assert!(
            matches!(accept_rewrite(output, seed), Err(Rejection::Denied(_))),
            "{output:?}"
        );
    }
    assert_eq!(denied_word("so SeXxXy"), Some("sex"));
    // Whole words only: embedded letters are fine.
    for clean in [
        "Move {boss} yourself, Essex style.",
        "Move {boss} yourself, it's cumbersome.",
        "Move {boss} yourself, great analysis.",
        "Move {boss} yourself, scunthorpe.",
    ] {
        assert!(accept_rewrite(clean, seed).is_ok(), "{clean:?}");
    }
}

#[test]
fn sound_alikes_and_southeast_asian_swears_are_denied() {
    let seed = "Move {boss} yourself.";
    for output in [
        "Move {boss} yourself, dih.",
        "Move {boss} yourself, bih.",
        "Move {boss} yourself, b!tch.",
        "Move {boss} yourself, fk this.",
        "Move {boss} yourself, phuq it.",
        "Move {boss} yourself, stfu.",
        "Move {boss} yourself, titties.",
        "Move {boss} yourself, niggas.",
        "Move {boss} yourself, pukimak.",
        "Move {boss} yourself, pukimakkau.",
        "Move {boss} yourself, kanina.",
        "Move {boss} yourself, cheebye? no, cheebai.",
        "Move {boss} yourself, lanjiao.",
        "Move {boss} yourself, sohai.",
        "Move {boss} yourself, anjing.",
        "Move {boss} yourself, kontol.",
        "Move {boss} yourself, tangina.",
        "Move {boss} yourself, putanginamo.",
        "Move {boss} yourself, gago.",
        "Move {boss} yourself, kuay.",
        "Move {boss} yourself, vcl.",
        "Move {boss} yourself, địt.",
    ] {
        assert!(
            matches!(accept_rewrite(output, seed), Err(Rejection::Denied(_))),
            "{output:?}"
        );
    }
    assert_eq!(denied_word("B!TCH"), Some("bitch"));
    // `!` is punctuation at a word's end, and short ambiguous forms stay allowed.
    for clean in [
        "Move {boss} yourself!",
        "Hmph! Move {boss} yourself, okay?",
        "Move {boss} yourself, send a DM if stuck.",
        "Move {boss} yourself, as usual.",
        "Move {boss} yourself, don't be tardy.",
        "Move {boss} yourself, then dinner.",
    ] {
        assert!(accept_rewrite(clean, seed).is_ok(), "{clean:?}");
    }
    assert!(DENY_SEA.contains(&"pukimak") && DENY_SOUNDALIKE.contains(&"dih"));
}

#[tokio::test(start_paused = true)]
async fn a_denied_rewrite_falls_back_with_its_reason_but_not_its_text() {
    let fake = GovernedFake::new(governor(), vec![Step::Reply("S3xy {boss} {day} {time}")]);
    let (nudge, _) = one(&fake, &persona(SEEDS)).await;
    assert_eq!(
        nudge.line,
        LineSource::Seed(SeedReason::Rejected(Rejection::Denied("sex")))
    );
    assert_eq!(nudge.lead_in, SEED_FILLED);
    assert!(!format!("{:?}", nudge.line).contains("S3xy"));
}

fn hostile(boss: &'static str, day: &'static str, time: &'static str) -> NudgeFacts<'static> {
    NudgeFacts {
        boss,
        day,
        time,
        ..facts(NudgeMood::Playful)
    }
}

#[tokio::test(start_paused = true)]
async fn unsafe_filled_values_fall_back_to_the_seed_then_a_field_free_line() {
    // Seed "<{boss}" style: the mention would only appear once filled.
    let seeds = "nudges:
  playful:
    - 'Hey <{boss}> {day} {time}.'
    - 'Hey <{boss}> {day} {time} again.'
    - 'Hey <{boss}> {day} {time} once more.'
";
    let nudger = Nudger::new(Arc::new(Fixed(0)), NoRewrite);
    let angled = persona(seeds);
    let nudge = nudger
        .lead_in(&angled, &hostile("@123456789012345678", "Sat", "21:00"))
        .await;
    assert_eq!(nudge.line, LineSource::FieldFree);
    assert!(!nudge.lead_in.contains('{') && !nudge.lead_in.contains('@'));

    // Each value is harmless alone; the mention only forms across the boundary.
    let at = persona(
        "nudges:
  playful:
    - 'Ping @{day} for {boss}.'
    - 'Ping @{day} for {boss} now.'
    - 'Ping @{day} for {boss} soon.'
",
    );
    for day in ["everyone", "here"] {
        let nudge = nudger.lead_in(&at, &hostile("Lucid", day, "21:00")).await;
        assert_eq!(nudge.line, LineSource::FieldFree, "{day}");
    }

    let plain = persona(SEEDS);
    for (boss, day, time) in [
        ("Lucid\nline two", "Sat", "21:00"),
        ("Lucid", "@everyone", "21:00"),
        ("Lucid", "Sat", "<@123>"),
        ("**Lucid**", "Sat", "21:00"),
        ("# Lucid", "Sat", "21:00"),
        ("Lu\u{202E}cid", "Sat", "21:00"),
        ("Lucid https://x.example", "Sat", "21:00"),
        ("discord.gg/raid", "Sat", "21:00"),
        ("{day}", "Sat", "21:00"),
        ("", "Sat", "21:00"),
        (&*"L".repeat(140).leak(), "Sat", "21:00"),
    ] {
        // Fresh rotation: the first seed uses all three fields.
        let nudger = Nudger::new(Arc::new(Fixed(0)), NoRewrite);
        let nudge = nudger.lead_in(&plain, &hostile(boss, day, time)).await;
        assert_eq!(
            nudge.line,
            LineSource::FieldFree,
            "{boss:?} {day:?} {time:?}"
        );
        assert!(
            kanade::chat::persona::check_nudge_line(&nudge.lead_in).is_ok(),
            "{:?}",
            nudge.lead_in
        );
    }

    // A safe rewrite whose extra field is unsafe drops back to the filled seed.
    let seeds = "nudges:
  playful:
    - 'Move {boss} yourself.'
    - 'Shift {boss} yourself.'
    - 'Go on, {boss} is yours.'
";
    let fake = GovernedFake::new(governor(), vec![Step::Reply("Move <{boss}> now.")]);
    let nudger = Nudger::new(Arc::new(Fixed(0)), &fake);
    let nudge = nudger
        .lead_in(&persona(seeds), &hostile("@here", "Sat", "21:00"))
        .await;
    assert_eq!(nudge.line, LineSource::FieldFree);
    let fake = GovernedFake::new(governor(), vec![Step::Reply("Move {boss} <{day}> now.")]);
    let nudger = Nudger::new(Arc::new(Fixed(0)), &fake);
    let nudge = nudger
        .lead_in(&persona(seeds), &hostile("Lucid", "@here", "21:00"))
        .await;
    // Rewrite and seed differ in placeholders, so the rewrite was rejected first.
    assert_eq!(
        nudge.line,
        LineSource::Seed(SeedReason::Rejected(Rejection::Placeholders))
    );
    assert_eq!(nudge.lead_in, "Move Lucid yourself.");
}

#[tokio::test(start_paused = true)]
async fn a_rewrite_unsafe_only_once_filled_uses_the_filled_seed() {
    let seeds = "nudges:
  playful:
    - 'Move {boss} yourself.'
    - 'Shift {boss} yourself.'
    - 'Go on, {boss} is yours.'
";
    // 131 chars of template: fine unfilled, too long with a long boss name,
    // while the short seed still fits.
    let long: &'static str = format!("{} {{boss}}.", "a".repeat(123)).leak();
    let fake = GovernedFake::new(governor(), vec![Step::Reply(long)]);
    let nudger = Nudger::new(Arc::new(Fixed(0)), &fake);
    let nudge = nudger
        .lead_in(
            &persona(seeds),
            &hostile("Chosen Seren the Very Long", "Sat", "21:00"),
        )
        .await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::UnsafeFill));
    assert_eq!(nudge.lead_in, "Move Chosen Seren the Very Long yourself.");
}

fn pool_of(line: &str) -> String {
    format!("nudges:\n  playful:\n    - '{line}'\n    - '{line} Go.'\n    - '{line} Now.'\n")
}

#[tokio::test(start_paused = true)]
async fn filling_must_not_create_markup_or_an_invite() {
    // Each value is harmless alone; the template around it completes the markup.
    for (line, boss, day) in [
        ("Join discord.{boss} to plan.", "gg/raid", "Sat"),
        ("See discordapp.com/{boss} later.", "invite/raid", "Sat"),
        ("Move ~{boss} now.", "~Lucid", "Sat"),
        ("{day}. Move {boss}.", "Lucid", "12"),
    ] {
        let nudger = Nudger::new(Arc::new(Fixed(0)), NoRewrite);
        let nudge = nudger
            .lead_in(&persona(&pool_of(line)), &hostile(boss, day, "21:00"))
            .await;
        assert_eq!(nudge.line, LineSource::FieldFree, "{line:?} + {boss:?}");
        assert!(!has_markup(&nudge.lead_in) && !has_invite(&nudge.lead_in));
    }

    // A seed's own approved markup is kept when the values add none.
    let nudger = Nudger::new(Arc::new(Fixed(0)), NoRewrite);
    let starred = persona(&pool_of("Move *{boss}* now."));
    let nudge = nudger
        .lead_in(&starred, &hostile("Lucid", "Sat", "21:00"))
        .await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Unavailable));
    assert_eq!(nudge.lead_in, "Move *Lucid* now.");

    // A rewrite that only becomes an invite once filled drops to the filled seed.
    let fake = GovernedFake::new(governor(), vec![Step::Reply("Join discord.{boss} now.")]);
    let nudger = Nudger::new(Arc::new(Fixed(0)), &fake);
    let nudge = nudger
        .lead_in(
            &persona(&pool_of("Move {boss} yourself.")),
            &hostile("gg/raid", "Sat", "21:00"),
        )
        .await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::UnsafeFill));
    assert_eq!(nudge.lead_in, "Move gg/raid yourself.");
}

#[tokio::test(start_paused = true)]
async fn the_rewrite_prompt_carries_no_member_channel_or_run_data() {
    let fake = GovernedFake::new(governor(), vec![Step::Reply("ok {boss} {day} {time}")]);
    let (nudge, _) = one(&fake, &persona(SEEDS)).await;
    assert_eq!(nudge.line, LineSource::Rewritten);
    let prompts = fake.prompts.lock().unwrap();
    let prompt = &prompts[0];
    let roster = [Member {
        user_id: MEMBER.into(),
        display_name: "Alvin".into(),
        nickname: Some("Alv".into()),
        aliases: vec!["alvintan".into()],
    }];
    for text in [prompt.system(), prompt.seed()] {
        assert!(find_leaks(text, &roster).is_empty(), "{text}");
        for value in [CHANNEL, "Hard Lucid", "Sat", "21:00"] {
            assert!(!text.contains(value), "{value} leaked into {text}");
        }
    }
    assert_eq!(prompt.seed(), "Move {boss} to {day} {time} yourself.");
    assert_eq!(prompt.messages().len(), 2);
}

fn profile(yaml_extra: &str) -> kanade::chat::persona::Profile {
    let text = format!(
        "schema_version: 1
id: senpai
label: Smug senpai
voice: Smug senpai who teases a lot.
prompt: |
  Synthetic profile senpai.
{yaml_extra}"
    );
    parse_profile(&text, &ProfileId::parse("senpai").unwrap()).unwrap()
}

#[test]
fn the_prompt_uses_the_profile_voice_and_mood_beats_it() {
    let id = PersonaId::parse("alpha").unwrap();
    let bundle = parse_bundle(&bundle_yaml(SEEDS), &id).unwrap();
    let with_profile = CompiledPersona::compile(&bundle, Some(&profile("")));

    let playful = RewritePrompt::build(&with_profile, NudgeMood::Playful, "seed");
    assert!(playful.system().starts_with(NUDGE_REWRITE_INSTRUCTION));
    assert!(playful.system().contains("safe for work"));
    assert!(
        playful
            .system()
            .contains("Rewrite as Alpha, a cheerful idol.")
    );
    assert!(
        playful
            .system()
            .contains("Voice: Smug senpai who teases a lot.")
    );
    assert!(playful.system().ends_with(PLAYFUL_MOOD));
    // Provenance never reaches the model: no profile id, label or prompt.
    for private in [
        "senpai\n",
        "Smug senpai\n",
        "Synthetic profile",
        "Synthetic identity",
    ] {
        assert!(!playful.system().contains(private), "{private}");
    }

    let gentle = RewritePrompt::build(&with_profile, NudgeMood::Gentle, "seed");
    assert!(gentle.system().ends_with(GENTLE_MOOD));
    assert!(gentle.system().contains("never teasing"));

    let bundle_only = CompiledPersona::compile(&bundle, None);
    let prompt = RewritePrompt::build(&bundle_only, NudgeMood::Playful, "seed");
    assert!(prompt.system().contains("Voice: Bright and bouncy."));
}

#[test]
fn without_nudge_rewrite_or_voice_only_the_code_instruction_and_mood_remain() {
    let text = bundle_yaml("").replace("  voice: Bright and bouncy.\n", "");
    let bundle = parse_bundle(&text, &PersonaId::parse("alpha").unwrap()).unwrap();
    let compiled = CompiledPersona::compile(&bundle, None);
    let prompt = RewritePrompt::build(&compiled, NudgeMood::Gentle, "seed");
    assert_eq!(
        prompt.system(),
        format!("{NUDGE_REWRITE_INSTRUCTION}\n\n{GENTLE_MOOD}")
    );
}

#[tokio::test(start_paused = true)]
async fn seeds_come_from_the_profile_then_the_bundle_then_built_ins() {
    let id = PersonaId::parse("alpha").unwrap();
    let bundle = parse_bundle(&bundle_yaml(SEEDS), &id).unwrap();
    let styled = profile(
        "nudges:
  gentle: ['Profile gentle one.', 'Profile gentle two.', 'Profile gentle three.']
",
    );
    let nudger = Nudger::new(Arc::new(Fixed(0)), NoRewrite);
    let with_profile = CompiledPersona::compile(&bundle, Some(&styled));
    let gentle = nudger
        .lead_in(&with_profile, &facts(NudgeMood::Gentle))
        .await;
    assert_eq!(gentle.seeds, NudgeSource::Profile);
    assert_eq!(gentle.lead_in, "Profile gentle one.");
    let playful = nudger
        .lead_in(&with_profile, &facts(NudgeMood::Playful))
        .await;
    assert_eq!(playful.seeds, NudgeSource::Bundle);
    let plain = persona("");
    let builtin = nudger.lead_in(&plain, &facts(NudgeMood::Playful)).await;
    assert_eq!(builtin.seeds, NudgeSource::BuiltIn);
}

#[test]
fn rotation_never_repeats_within_the_window_and_is_fair() {
    let rotation = SeedRotation::new(Arc::new(XorShift::new(7)));
    let lines = ["a", "b", "c", "d", "e"];
    let mut picks = Vec::new();
    for _ in 0..5_000 {
        picks.push(rotation.pick(CHANNEL, &lines).unwrap());
    }
    for window in picks.windows(RECENT_PER_CHANNEL + 1) {
        let mut sorted = window.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), window.len(), "repeat in {window:?}");
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for pick in &picks {
        *counts.entry(pick).or_default() += 1;
    }
    for line in lines {
        let count = counts[line];
        assert!((900..=1_100).contains(&count), "{line}: {count}");
    }
}

#[test]
fn short_pools_still_never_repeat_back_to_back() {
    let rotation = SeedRotation::new(Arc::new(XorShift::new(3)));
    let pool = ["x", "y", "z"];
    let picks: Vec<_> = (0..300)
        .map(|_| rotation.pick("c", &pool).unwrap())
        .collect();
    assert!(
        picks
            .windows(3)
            .all(|w| w[0] != w[1] && w[1] != w[2] && w[0] != w[2])
    );
    let two = ["p", "q"];
    let picks: Vec<_> = (0..50).map(|_| rotation.pick("d", &two).unwrap()).collect();
    assert!(picks.windows(2).all(|w| w[0] != w[1]));
    assert_eq!(rotation.pick("e", &["only"]), Some("only"));
    assert_eq!(rotation.pick("e", &["only"]), Some("only"));
    assert_eq!(rotation.pick("e", &[]), None);
}

#[test]
fn channels_rotate_independently() {
    let rotation = SeedRotation::new(Arc::new(Fixed(0)));
    let lines = ["a", "b", "c", "d"];
    assert_eq!(rotation.pick("one", &lines), Some("a"));
    // Channel two has seen nothing, so "a" is still eligible there.
    assert_eq!(rotation.pick("two", &lines), Some("a"));
    assert_eq!(rotation.pick("one", &lines), Some("b"));
}

#[test]
fn rendering_always_ends_with_the_action_and_link() {
    let link = "https://kanade-pub.example/runs/r1?move_to=2026-09-26T13:00:00Z";
    assert_eq!(
        render(Some("Hmph."), NudgePurpose::SelfService, link),
        format!("Hmph. {EDIT_RUN_ACTION}{link}")
    );
    assert_eq!(
        render(None, NudgePurpose::RequestForm, link),
        format!("{REQUEST_CHANGE_ACTION}{link}")
    );
    assert_eq!(mood_for(false, false), NudgeMood::Playful);
    assert_eq!(mood_for(true, false), NudgeMood::Gentle);
    assert_eq!(mood_for(false, true), NudgeMood::Gentle);
}

fn utc(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, 0, 0).unwrap()
}

#[tokio::test(start_paused = true)]
async fn one_tip_per_member_per_boss_week_even_when_claims_race() {
    let store = MemoryScheduleStore::new();
    let governor = governor();
    let fake = GovernedFake::new(
        governor,
        vec![
            Step::Reply("a {boss} {day} {time}"),
            Step::Reply("b {boss} {day} {time}"),
        ],
    );
    let nudger = Nudger::new(Arc::new(Fixed(0)), &fake);
    let persona = persona(SEEDS);
    let reset = reset();
    let facts = facts(NudgeMood::Playful);
    let (a, b, c) = tokio::join!(
        nudger.tip(&store, MEMBER, &reset, utc(25, 9), &persona, &facts),
        nudger.tip(&store, MEMBER, &reset, utc(25, 9), &persona, &facts),
        nudger.tip(&store, MEMBER, &reset, utc(25, 9), &persona, &facts),
    );
    let granted = [a.unwrap(), b.unwrap(), c.unwrap()]
        .into_iter()
        .flatten()
        .count();
    assert_eq!(granted, 1);
    // The claim comes first, so refused tips never reach the model.
    assert_eq!(fake.sent(), 1);
    let again = nudger
        .tip(&store, MEMBER, &reset, utc(27, 9), &persona, &facts)
        .await
        .unwrap();
    assert!(again.is_none());
}

/// Thursday 08:00 Singapore time, i.e. 00:00 UTC.
fn reset() -> WeekReset {
    WeekReset {
        zone: chrono_tz::Asia::Singapore,
        weekday: chrono::Weekday::Thu,
        time: chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
    }
}

#[tokio::test(start_paused = true)]
async fn the_tip_week_follows_the_configured_reset() {
    let store = MemoryScheduleStore::new();
    let nudger = Nudger::new(Arc::new(Fixed(0)), NoRewrite);
    let persona = persona(SEEDS);
    let facts = facts(NudgeMood::Playful);
    let reset = reset();
    let tip = |now| nudger.tip(&store, MEMBER, &reset, now, &persona, &facts);
    // Thu 24 Sep 07:59 local is still the week that began Thu 17 Sep.
    let before_first = Utc.with_ymd_and_hms(2026, 9, 23, 23, 59, 0).unwrap();
    assert!(tip(before_first).await.unwrap().is_some());
    assert!(tip(utc(24, 0)).await.unwrap().is_some());
    let before = Utc.with_ymd_and_hms(2026, 9, 30, 23, 59, 59).unwrap();
    assert!(tip(before).await.unwrap().is_none());
    let after = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
    assert!(tip(after).await.unwrap().is_some());
    assert!(tip(after).await.unwrap().is_none());
}

#[test]
fn the_rotation_remembers_a_bounded_number_of_channels() {
    let rotation = SeedRotation::new(Arc::new(Fixed(0)));
    let lines = ["a", "b", "c", "d"];
    for channel in 0..MAX_CHANNELS + 50 {
        rotation.pick(&channel.to_string(), &lines);
    }
    assert_eq!(rotation.channels(), MAX_CHANNELS);
    // Channel 0 was the least recently used and was forgotten: it starts over.
    assert_eq!(rotation.pick("0", &lines), Some("a"));
    // A recently used channel still remembers its pick.
    let recent = (MAX_CHANNELS + 49).to_string();
    assert_eq!(rotation.pick(&recent, &lines), Some("b"));
    assert_eq!(rotation.channels(), MAX_CHANNELS);
}
