//! Policy invariants: mentions never exceed the policy, suppressed rows never
//! produce an intent, and planning is deterministic and never replays a row.

use std::collections::BTreeSet;

use kanade::domain::{
    members::{Directory, Member, PingLevel, Roster},
    notify::{
        DeliveryTarget, DispatchInput, DispatchPlan, IntentContent, PingKind, SendDisposition,
        allowed_mentions, plan_dispatch, resolve_mentions, swap_audience, wants_mention,
    },
};

/// Checks every dispatch plan the replays produce.
pub fn check_dispatch_plan(input: &DispatchInput<'_>, plan: &DispatchPlan) {
    assert_eq!(&plan_dispatch(input), plan, "planning is not deterministic");
    let retired: BTreeSet<DeliveryTarget> = plan
        .retire
        .iter()
        .map(|row| DeliveryTarget::Reminder(row.reminder_id.clone()))
        .collect();
    let queued: BTreeSet<DeliveryTarget> = plan
        .queued
        .iter()
        .flat_map(|row| {
            row.reminder_ids
                .iter()
                .cloned()
                .map(DeliveryTarget::Reminder)
        })
        .collect();
    let mut seen = BTreeSet::new();
    for send in &plan.sends {
        let intent = &send.intent;
        for target in &intent.targets {
            assert!(
                !retired.contains(target),
                "intent for a suppressed row {target:?}"
            );
            assert!(
                !queued.contains(target),
                "intent for a queued row {target:?}"
            );
            assert!(seen.insert(target.clone()), "{target:?} planned twice");
        }
        let (run_ids, kind) = match &intent.content {
            IntentContent::DayOf { run_ids } => (run_ids.clone(), PingKind::DayOf),
            IntentContent::Countdown { run_id, .. } => (vec![run_id.clone()], PingKind::Countdown),
            other => panic!("dispatch planned {other:?}"),
        };
        let allowed: BTreeSet<&String> = input
            .schedule
            .runs
            .iter()
            .filter(|run| run_ids.contains(&run.id))
            .flat_map(|run| &run.participants)
            .collect();
        let unique: BTreeSet<&String> = intent.mentions.iter().collect();
        assert_eq!(unique.len(), intent.mentions.len(), "duplicate mentions");
        for user in &intent.mentions {
            assert!(allowed.contains(user), "{user} is not on the intent's runs");
            assert!(
                wants_mention(input.members.ping_level(user), &kind),
                "{user} mentioned against their level"
            );
        }
    }
    // Quiet mode changes nothing but the allow-lists, which it empties.
    let mut quiet = *input;
    quiet.settings.quiet_mode = true;
    let mut silenced = plan.clone();
    for send in &mut silenced.sends {
        send.intent.mentions.clear();
    }
    assert_eq!(plan_dispatch(&quiet), silenced, "quiet mode");
}

/// After a plan is applied, planning again never repeats a retired or sent row.
pub fn check_nothing_replayed(before: &DispatchPlan, after: &DispatchPlan) {
    let settled: BTreeSet<DeliveryTarget> = before
        .retire
        .iter()
        .map(|row| DeliveryTarget::Reminder(row.reminder_id.clone()))
        .chain(
            before
                .sends
                .iter()
                .flat_map(|send| send.intent.targets.iter().cloned()),
        )
        .collect();
    for row in &after.retire {
        let target = DeliveryTarget::Reminder(row.reminder_id.clone());
        assert!(!settled.contains(&target), "{target:?} retired twice");
    }
    for send in after
        .sends
        .iter()
        .filter(|send| send.disposition == SendDisposition::Send)
    {
        for target in &send.intent.targets {
            assert!(!settled.contains(target), "{target:?} attempted twice");
        }
    }
}

fn kinds() -> Vec<PingKind> {
    let mut kinds: Vec<PingKind> = PingKind::ESSENTIAL
        .iter()
        .chain(PingKind::INFORMATIONAL)
        .cloned()
        .collect();
    kinds.push(PingKind::parse("nonsense"));
    kinds
}

/// Every level assignment for three members, over lists with repeats and a stranger.
fn rosters() -> Vec<Roster> {
    let levels = PingLevel::ALL;
    let mut out = Vec::new();
    for a in levels {
        for b in levels {
            for c in levels {
                let mut roster = Roster::new();
                for (user_id, level) in [("1", a), ("2", b), ("3", c)] {
                    roster.upsert(Member {
                        user_id: user_id.into(),
                        display_name: Some(format!("member {user_id}")),
                        ping_level: *level,
                        ..Member::default()
                    });
                }
                out.push(roster);
            }
        }
    }
    out
}

#[test]
fn resolved_mentions_are_an_ordered_unique_subset_the_policy_allows() {
    let lists: [&[&str]; 5] = [
        &[],
        &["1", "2", "3"],
        &["3", "1", "3", "2", "1"],
        &["404", "2"],
        &["2", "2", "404", "404"],
    ];
    for roster in rosters() {
        for kind in kinds() {
            for candidates in lists {
                let resolved = resolve_mentions(&roster, candidates, &kind);
                let again = resolve_mentions(&roster, candidates, &kind);
                assert_eq!(resolved, again, "not deterministic");
                let mut positions = Vec::new();
                for user in &resolved {
                    let level = roster.ping_level(user);
                    assert_ne!(level, PingLevel::Off, "{user} is off");
                    assert!(
                        level == PingLevel::All || kind.is_essential(),
                        "{user} ({level:?}) mentioned by {kind:?}"
                    );
                    let first = candidates.iter().position(|c| c == user).expect("subset");
                    positions.push(first);
                }
                assert!(
                    positions.windows(2).all(|w| w[0] < w[1]),
                    "order or duplicates"
                );
                let complete = candidates.iter().filter(|user| {
                    let level = roster.ping_level(user);
                    level != PingLevel::Off && (level == PingLevel::All || kind.is_essential())
                });
                let expected: BTreeSet<&str> = complete.copied().collect();
                let got: BTreeSet<&str> = resolved.iter().map(String::as_str).collect();
                assert_eq!(got, expected, "a wanted mention was dropped");
            }
        }
    }
}

#[test]
fn swap_mentions_stay_within_the_listed_people_and_quiet_mode_clears_all() {
    for roster in rosters() {
        let who = swap_audience(&roster, &["1", "3"], &["2", "404"]);
        for user in &who.mentioned {
            assert!(["1", "3", "2", "404"].contains(&user.as_str()));
            assert_eq!(
                roster.ping_level(user),
                PingLevel::All,
                "swap is informational"
            );
        }
        let gate = allowed_mentions(&who.mentioned, None, true);
        assert!(gate.users().is_empty() && !gate.replied_user());
        let digest = allowed_mentions(&who.mentioned, Some(&[]), false);
        assert!(digest.users().is_empty() && digest.replied_user());
    }
}

#[test]
fn notices_mention_only_listed_members_on_all_and_fall_back_or_drop() {
    use kanade::domain::{
        notify::{DeliverySettings, DeliveryWarning, EffectKind, plan_notice},
        schedule::{FixedField, Notice, NoticeChange, RunStatus},
    };
    let notice = |channel: Option<&str>| Notice {
        change: NoticeChange::RunStatus {
            run_id: "run-1".into(),
            from: RunStatus::Planned,
            to: RunStatus::Cancelled,
        },
        channel_id: channel.map(str::to_owned),
        listed: vec!["3".into(), "2".into(), "404".into(), "1".into(), "2".into()],
        via_portal: true,
    };
    let channels: BTreeSet<String> = ["222".to_owned(), "555".to_owned()].into();
    let loud = DeliverySettings {
        post_channel_id: Some("555"),
        quiet_mode: false,
        attendance: kanade::domain::attendance::AttendancePolicy::V4_COMPAT,
    };
    for roster in rosters() {
        let intent = plan_notice(&notice(Some("222")), &roster, &channels, loud).expect("home");
        assert_eq!(intent.channel_id, "222");
        assert!(intent.targets.is_empty(), "notices are operation-scoped");
        assert_eq!(
            intent.effect,
            EffectKind::Notice("notice.run.status.cancelled".into())
        );
        assert_eq!(
            intent.effect_context,
            ["run-1", "planned", "cancelled", "portal"]
        );
        let wanted: Vec<String> = ["1", "2", "3"]
            .into_iter()
            .filter(|user| roster.ping_level(user) == PingLevel::All)
            .map(str::to_owned)
            .collect();
        assert_eq!(intent.mentions, wanted, "status is informational");

        assert!(intent.warnings.is_empty(), "home channel used");
        // Fallbacks warn exactly as reminder dispatch does, unset home included.
        let warned = |home: Option<&str>| {
            vec![DeliveryWarning::HomeChannelUnavailable {
                home_channel_id: home.map(str::to_owned),
                run_ids: vec!["run-1".into()],
            }]
        };
        let moved = plan_notice(&notice(Some("444")), &roster, &channels, loud).expect("fallback");
        assert_eq!(moved.channel_id, "555");
        assert_eq!(moved.warnings, warned(Some("444")));
        let homeless = plan_notice(&notice(None), &roster, &channels, loud).expect("fallback");
        assert_eq!(homeless.channel_id, "555");
        assert_eq!(homeless.warnings, warned(None));
        let weekly = Notice {
            change: NoticeChange::FixedChanged {
                fixed_id: "fixed-1".into(),
                fields: vec![FixedField::ChannelId],
                weekday: chrono::Weekday::Mon,
                time: chrono::NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
                participants: vec!["1".into()],
            },
            ..notice(Some("444"))
        };
        let weekly = plan_notice(&weekly, &roster, &channels, loud).expect("fallback");
        assert_eq!(
            weekly.warnings,
            [DeliveryWarning::HomeChannelUnavailable {
                home_channel_id: Some("444".into()),
                run_ids: Vec::new(),
            }],
            "a weekly-timing notice names no run"
        );
        let nowhere = DeliverySettings {
            post_channel_id: None,
            ..loud
        };
        assert_eq!(
            plan_notice(&notice(None), &roster, &channels, nowhere),
            None
        );
        let quiet = DeliverySettings {
            quiet_mode: true,
            ..loud
        };
        let silenced = plan_notice(&notice(Some("222")), &roster, &channels, quiet).unwrap();
        assert!(silenced.mentions.is_empty(), "quiet mode");
    }
}
