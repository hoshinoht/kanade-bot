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
    EDIT_RUN_ACTION, GENTLE_MOOD, LineSource, NUDGE_REWRITE_INSTRUCTION, NoRewrite, Nudge,
    NudgeFacts, NudgeRewriter, Nudger, PLAYFUL_MOOD, RECENT_PER_CHANNEL, REQUEST_CHANGE_ACTION,
    RewritePrompt, RewriteUnavailable, SeedReason, SeedRotation, accept_rewrite, mood_for, render,
};
use kanade::chat::persona::{
    CompiledPersona, NudgeMood, NudgePurpose, NudgeSource, PersonaId, ProfileId, parse_bundle,
    parse_profile,
};
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
    ) -> Result<String, RewriteUnavailable> {
        let permit = self
            .governor
            .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
            .map_err(|_| RewriteUnavailable)?;
        let attempt = permit.try_begin_request().map_err(|_| RewriteUnavailable)?;
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
                Err(RewriteUnavailable)
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
        assert_eq!(
            nudge.line,
            LineSource::Seed(SeedReason::Rejected),
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
            "  Ehh, drag {boss} to {day} {time} yourself~ \n",
        )],
    );
    let (nudge, _) = one(&fake, &persona(SEEDS)).await;
    assert_eq!(nudge.line, LineSource::Rewritten);
    assert_eq!(nudge.lead_in, "Ehh, drag Hard Lucid to Sat 21:00 yourself~");
    assert_eq!(nudge.seeds, NudgeSource::Bundle);
    assert_eq!(fake.sent(), 1);
}

#[test]
fn accepted_rewrites_keep_exactly_the_seed_placeholders() {
    let seed = "Move {boss} yourself.";
    assert_eq!(
        accept_rewrite("Go move {boss}!", seed).as_deref(),
        Some("Go move {boss}!")
    );
    assert_eq!(accept_rewrite("Go move it!", seed), None);
    assert_eq!(accept_rewrite("Move {boss} on {day}", seed), None);
    assert_eq!(
        accept_rewrite("Plain line.", "Plain seed."),
        Some("Plain line.".into())
    );
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
    let week = utc(24, 0);
    let facts = facts(NudgeMood::Playful);
    let (a, b, c) = tokio::join!(
        nudger.tip(&store, MEMBER, week, utc(25, 9), &persona, &facts),
        nudger.tip(&store, MEMBER, week, utc(25, 9), &persona, &facts),
        nudger.tip(&store, MEMBER, week, utc(25, 9), &persona, &facts),
    );
    let granted = [a.unwrap(), b.unwrap(), c.unwrap()]
        .into_iter()
        .flatten()
        .count();
    assert_eq!(granted, 1);
    // The claim comes first, so refused tips never reach the model.
    assert_eq!(fake.sent(), 1);
    let again = nudger
        .tip(&store, MEMBER, week, utc(27, 9), &persona, &facts)
        .await
        .unwrap();
    assert!(again.is_none());
    let next_reset = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
    let next_week = nudger
        .tip(&store, MEMBER, next_reset, next_reset, &persona, &facts)
        .await;
    assert!(next_week.unwrap().is_some());
}
