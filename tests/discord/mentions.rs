//! The allow-list can never be wider than the explicit user list.

use std::collections::BTreeSet;

use serde_json::json;
use twilight_model::id::{Id, marker::UserMarker};

use kanade::bot::ids::parse_id;
use kanade::bot::mentions;
use kanade::domain::notify::AllowedMentions as Policy;

/// Hostile and malformed entries mixed into generated lists.
const NOISE: &[&str] = &[
    "@everyone",
    "@here",
    "everyone",
    "<@&500>",
    "<@1001>",
    "0",
    "007",
    "+5",
    "-5",
    " 1001",
    "1001 ",
    "18446744073709551616",
    "",
    "roles",
];

/// Deterministic generator (no framework): a 64-bit LCG.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

#[test]
fn generated_lists_never_widen() {
    let mut rng = Lcg(42);
    for _ in 0..2_000 {
        let len = rng.next() % 8;
        let input: Vec<String> = (0..len)
            .map(|_| match rng.next() % 3 {
                0 => NOISE[(rng.next() as usize) % NOISE.len()].to_owned(),
                _ => (1 + rng.next() % 5).to_string(),
            })
            .collect();
        let built = mentions::allow_users(&input);

        assert!(built.parse.is_empty(), "{input:?}");
        assert!(built.roles.is_empty(), "{input:?}");
        assert!(!built.replied_user, "{input:?}");
        let allowed: BTreeSet<Id<UserMarker>> =
            input.iter().filter_map(|text| parse_id(text)).collect();
        let users: BTreeSet<_> = built.users.iter().copied().collect();
        assert!(users.is_subset(&allowed), "{input:?} -> {users:?}");
        assert_eq!(users, allowed, "every valid listed user is kept");
        let mut sorted = built.users.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, built.users, "canonical: sorted and unique");
    }
}

#[test]
fn empty_list_mentions_nobody_on_the_wire() {
    for built in [
        mentions::none(),
        mentions::allow_users::<&str>(&[]),
        mentions::for_policy(&Policy::None),
        mentions::for_policy(&Policy::Users(vec![])),
    ] {
        assert_eq!(
            serde_json::to_value(&built).unwrap(),
            json!({ "parse": [] })
        );
    }
}

#[test]
fn listed_users_only_on_the_wire() {
    let built = mentions::allow_users(&["1002", "1001", "1001", "@everyone", "<@&500>"]);
    assert_eq!(
        serde_json::to_value(&built).unwrap(),
        json!({ "parse": [], "users": ["1001", "1002"] })
    );
}

#[test]
fn policy_users_never_enable_replied_user() {
    let built = mentions::for_policy(&Policy::Users(vec!["1001".into()]));
    assert!(!built.replied_user);
    assert_eq!(built.users, vec![Id::new(1001)]);
}

#[test]
fn snowflake_parsing_is_canonical() {
    assert_eq!(parse_id::<UserMarker>("1001"), Some(Id::new(1001)));
    for alias in [
        "0",
        "01001",
        "+1001",
        "-1001",
        "1001.0",
        "",
        "18446744073709551616",
    ] {
        assert_eq!(parse_id::<UserMarker>(alias), None, "{alias:?}");
    }
}

#[test]
fn intent_allow_list_is_its_mentions_only() {
    use kanade::domain::notify::{EffectKind, IntentContent, NotificationIntent};
    let intent = NotificationIntent {
        effect: EffectKind::Reminder,
        effect_context: Vec::new(),
        channel_id: "300".into(),
        targets: Vec::new(),
        mentions: vec!["1002".into(), "1001".into()],
        content: IntentContent::DayOf {
            run_ids: vec!["run".into()],
        },
        warnings: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(mentions::for_intent(&intent)).unwrap(),
        json!({ "parse": [], "users": ["1001", "1002"] })
    );
    let quiet = NotificationIntent {
        mentions: Vec::new(),
        ..intent
    };
    assert_eq!(
        serde_json::to_value(mentions::for_intent(&quiet)).unwrap(),
        json!({ "parse": [] })
    );
}
