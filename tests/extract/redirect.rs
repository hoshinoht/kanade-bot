//! Self-service redirect: the four recorded cases, mode switching (forced
//! cards-only while the public portal is closed) and pre-filled link contents.

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::chat::persona::NudgePurpose;
use kanade::domain::notify::WeekReset;
use kanade::domain::proposals::{ChangeKind, Payload, ProposedChange};
use kanade::domain::schedule::{Run, RunSource, RunStatus};
use kanade::extract::redirect::{
    FixedChange, PortalLinks, PublicPortalLinks, RedirectCase, RedirectFacts, RedirectLink,
    SelfServiceMode, classify, effective_mode, is_link_id, plan,
};

const AUTHOR: &str = "111111111111111111";
const OTHER: &str = "222222222222222222";
const ORIGIN: &str = "https://kanade-pub.example.dev";

fn utc(month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, 0, 0).unwrap()
}

/// Thursday 00:00 UTC reset; "now" is Fri 25 Sep, so the week began Thu 24 Sep.
fn reset() -> WeekReset {
    WeekReset {
        zone: chrono_tz::UTC,
        weekday: Weekday::Thu,
        time: NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
    }
}

fn run(participants: &[&str]) -> Run {
    Run {
        id: "r-1".into(),
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start: utc(9, 24, 0),
        datetime: utc(9, 26, 13),
        bosses: vec!["HLucid".into()],
        participants: participants.iter().map(|id| (*id).to_owned()).collect(),
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn move_to(to: DateTime<Utc>) -> ProposedChange {
    ProposedChange {
        run_id: Some("r-1".into()),
        new_datetime: Some(to),
        ..ProposedChange::new(ChangeKind::Move)
    }
}

fn facts<'a>(change: &'a ProposedChange, run: Option<&'a Run>) -> RedirectFacts<'a> {
    RedirectFacts {
        author_id: AUTHOR,
        change,
        run,
        confidence: 0.9,
        is_question: false,
        min_confidence: 0.6,
        changes_in_message: 1,
        reset: reset(),
        now: utc(9, 25, 12),
    }
}

fn links() -> PublicPortalLinks {
    PublicPortalLinks::new(ORIGIN).unwrap()
}

fn fix_edit(id: Option<&str>) -> ProposedChange {
    ProposedChange {
        payload: Payload::FixEdit {
            fixed_run_id: id.map(str::to_owned),
            weekday: Some(Weekday::Thu),
            time: NaiveTime::from_hms_opt(21, 30, 0),
            participants: Vec::new(),
        },
        ..ProposedChange::new(ChangeKind::Fix)
    }
}

#[test]
fn classification_table() {
    let mine = run(&[AUTHOR, OTHER]);
    let theirs = run(&[OTHER]);
    let in_week = move_to(utc(9, 27, 20));
    let next_week = move_to(utc(10, 1, 1));
    let mut cancelled = mine.clone();
    cancelled.status = RunStatus::Cancelled;
    let mut last_week = mine.clone();
    last_week.week_start = utc(9, 17, 0);
    let no_time = ProposedChange {
        new_datetime: None,
        ..in_week.clone()
    };
    let wrong_run = ProposedChange {
        run_id: Some("r-2".into()),
        ..in_week.clone()
    };
    let remove = ProposedChange {
        payload: Payload::FixRemove {
            fixed_run_id: Some("f-1".into()),
        },
        ..ProposedChange::new(ChangeKind::Fix)
    };
    let new_timing = ProposedChange {
        payload: Payload::Fix {
            weekday: Some(Weekday::Sat),
            time: NaiveTime::from_hms_opt(20, 0, 0),
        },
        ..ProposedChange::new(ChangeKind::Fix)
    };
    let edit = fix_edit(Some("f-1"));
    let edit_unknown = fix_edit(None);

    let base = |change, run| facts(change, run);
    let cases: Vec<(&str, RedirectFacts<'_>, RedirectCase)> = vec![
        (
            "own run, same week",
            base(&in_week, Some(&mine)),
            RedirectCase::SelfService,
        ),
        (
            "someone else's run",
            base(&in_week, Some(&theirs)),
            RedirectCase::NeedsApproval,
        ),
        (
            "crosses the reset",
            base(&next_week, Some(&mine)),
            RedirectCase::NeedsApproval,
        ),
        (
            "run not this week",
            base(&in_week, Some(&last_week)),
            RedirectCase::NeedsApproval,
        ),
        (
            "cancelled run",
            base(&in_week, Some(&cancelled)),
            RedirectCase::NeedsApproval,
        ),
        (
            "multi-change message",
            RedirectFacts {
                changes_in_message: 2,
                ..base(&in_week, Some(&mine))
            },
            RedirectCase::NeedsApproval,
        ),
        (
            "low confidence",
            RedirectFacts {
                confidence: 0.5,
                ..base(&in_week, Some(&mine))
            },
            RedirectCase::Unclear,
        ),
        (
            "NaN confidence",
            RedirectFacts {
                confidence: f64::NAN,
                ..base(&in_week, Some(&mine))
            },
            RedirectCase::Unclear,
        ),
        (
            "question",
            RedirectFacts {
                is_question: true,
                ..base(&in_week, Some(&mine))
            },
            RedirectCase::Unclear,
        ),
        (
            "no new time",
            base(&no_time, Some(&mine)),
            RedirectCase::Unclear,
        ),
        ("no target run", base(&in_week, None), RedirectCase::Unclear),
        (
            "run mismatch",
            base(&wrong_run, Some(&mine)),
            RedirectCase::Unclear,
        ),
        ("weekly edit", base(&edit, None), RedirectCase::FixedRun),
        (
            "weekly removal",
            base(&remove, None),
            RedirectCase::FixedRun,
        ),
        (
            "new weekly timing",
            base(&new_timing, None),
            RedirectCase::NeedsApproval,
        ),
        (
            "weekly edit without id",
            base(&edit_unknown, None),
            RedirectCase::Unclear,
        ),
    ];
    for (name, facts, expected) in &cases {
        assert_eq!(classify(facts), *expected, "{name}");
    }
    for kind in [
        ChangeKind::Add,
        ChangeKind::Cancel,
        ChangeKind::Split,
        ChangeKind::Otot,
        ChangeKind::Sub,
        ChangeKind::Rsvp,
    ] {
        let change = ProposedChange {
            run_id: Some("r-1".into()),
            ..ProposedChange::new(kind)
        };
        assert_eq!(
            classify(&facts(&change, Some(&mine))),
            RedirectCase::NeedsApproval,
            "{kind:?}"
        );
    }
}

#[test]
fn modes_parse_and_closed_portal_forces_cards_only() {
    for mode in SelfServiceMode::ALL {
        assert_eq!(SelfServiceMode::parse(mode.as_str()), Some(mode));
        assert_eq!(effective_mode(mode, false), SelfServiceMode::CardsOnly);
        assert_eq!(effective_mode(mode, true), mode);
    }
    assert_eq!(SelfServiceMode::default(), SelfServiceMode::CardsAndLink);
    assert_eq!(SelfServiceMode::parse("link-first"), None);
}

/// The portal refuses moves of a started run and into the past, so link-first
/// never drops the card for them.
#[test]
fn started_runs_and_past_targets_keep_their_card() {
    let mut started = run(&[AUTHOR]);
    started.datetime = utc(9, 25, 11);
    let later = move_to(utc(9, 27, 20));
    let past = move_to(utc(9, 25, 10));
    let mine = run(&[AUTHOR]);
    for (change, target) in [(&later, &started), (&past, &mine)] {
        let facts = facts(change, Some(target));
        assert_eq!(classify(&facts), RedirectCase::NeedsApproval);
        let redirect = plan(&facts, SelfServiceMode::LinkFirst, &links());
        assert!(redirect.keep_card);
        assert_eq!(redirect.link, None);
    }
}

#[test]
fn a_self_service_move_gets_its_link_by_mode() {
    let mine = run(&[AUTHOR]);
    let change = move_to(utc(9, 27, 20));
    let facts = facts(&change, Some(&mine));
    let link = RedirectLink {
        purpose: NudgePurpose::SelfService,
        url: format!("{ORIGIN}/runs/r-1?move_to=2026-09-27T20:00:00Z"),
    };

    let both = plan(&facts, SelfServiceMode::CardsAndLink, &links());
    assert!(both.keep_card);
    assert_eq!(both.link.as_ref(), Some(&link));

    let first = plan(&facts, SelfServiceMode::LinkFirst, &links());
    assert!(!first.keep_card);
    assert_eq!(first.link.as_ref(), Some(&link));

    for configured in SelfServiceMode::ALL {
        let closed = plan(&facts, effective_mode(configured, false), &links());
        assert!(closed.keep_card);
        assert_eq!(closed.link, None);
        assert_eq!(closed.case, RedirectCase::SelfService);
    }
}

/// What serve posts (no lead-in there): the action and the portal link, the
/// Move page for a run and the request form for a weekly timing.
#[test]
fn posted_lines_carry_the_move_page_or_the_request_form() {
    let mine = run(&[AUTHOR]);
    let change = move_to(utc(9, 27, 20));
    let moved = plan(
        &facts(&change, Some(&mine)),
        SelfServiceMode::CardsAndLink,
        &links(),
    );
    assert_eq!(
        moved.link.expect("a move link").line(None),
        format!("→ edit the run: {ORIGIN}/runs/r-1?move_to=2026-09-27T20:00:00Z")
    );
    let edit = fix_edit(Some("f-1"));
    let asked = plan(&facts(&edit, None), SelfServiceMode::LinkFirst, &links());
    assert!(asked.keep_card);
    assert_eq!(
        asked.link.expect("a request link").line(Some("Ask away!")),
        format!(
            "Ask away! → request a change: {ORIGIN}/requests/new?fixed=f-1&change=edit&day=thu&time=21:30"
        )
    );
}

#[test]
fn weekly_timing_changes_keep_their_card_and_link_the_request_form() {
    let edit = fix_edit(Some("f-1"));
    for mode in [SelfServiceMode::CardsAndLink, SelfServiceMode::LinkFirst] {
        let redirect = plan(&facts(&edit, None), mode, &links());
        assert!(redirect.keep_card, "{mode:?}");
        assert_eq!(
            redirect.link,
            Some(RedirectLink {
                purpose: NudgePurpose::RequestForm,
                url: format!("{ORIGIN}/requests/new?fixed=f-1&change=edit&day=thu&time=21:30"),
            })
        );
    }
    let remove = ProposedChange {
        payload: Payload::FixRemove {
            fixed_run_id: Some("f-1".into()),
        },
        ..ProposedChange::new(ChangeKind::Fix)
    };
    let redirect = plan(&facts(&remove, None), SelfServiceMode::LinkFirst, &links());
    assert_eq!(
        redirect.link.map(|link| link.url),
        Some(format!("{ORIGIN}/requests/new?fixed=f-1&change=remove"))
    );
}

#[test]
fn other_cases_never_get_a_link_or_lose_their_card() {
    let theirs = run(&[OTHER]);
    let change = move_to(utc(9, 27, 20));
    for mode in SelfServiceMode::ALL {
        let mut unclear = facts(&change, Some(&theirs));
        let redirect = plan(&unclear, mode, &links());
        assert_eq!(redirect.case, RedirectCase::NeedsApproval);
        assert!(redirect.keep_card && redirect.link.is_none());
        unclear.confidence = 0.1;
        let redirect = plan(&unclear, mode, &links());
        assert_eq!(redirect.case, RedirectCase::Unclear);
        assert!(redirect.keep_card && redirect.link.is_none());
    }
}

#[test]
fn ids_outside_the_uuid_alphabet_get_no_link_and_keep_their_card() {
    let links = links();
    let at = utc(9, 27, 20);
    // `%2E` would not help: URL parsers read it as `.` and collapse the path.
    let hostile = [
        "",
        ".",
        "..",
        "...",
        "%2E%2E",
        "%2e",
        "r.1",
        "r 1",
        "r/../x",
        "r?y",
        "r#z",
        "r~1",
        "ü",
        "r\u{202E}1",
    ];
    for id in hostile {
        assert!(!is_link_id(id), "{id:?}");
        assert_eq!(links.move_run(id, at), None, "{id:?}");
        assert_eq!(
            links.request_fixed(id, FixedChange::Remove, None, None),
            None,
            "{id:?}"
        );
    }
    for id in [
        "0f8fad5b-d9cb-469f-a165-70867728950e",
        "00000106-0000-4000-8000-000000000001",
        "r-1",
        "f_1",
    ] {
        assert!(is_link_id(id), "{id:?}");
    }

    // A self-service move and a weekly edit on such ids keep today's card.
    let mut mine = run(&[AUTHOR]);
    mine.id = "..".into();
    let change = ProposedChange {
        run_id: Some("..".into()),
        ..move_to(utc(9, 27, 20))
    };
    for mode in [SelfServiceMode::CardsAndLink, SelfServiceMode::LinkFirst] {
        let redirect = plan(&facts(&change, Some(&mine)), mode, &links);
        assert_eq!(redirect.case, RedirectCase::SelfService);
        assert!(redirect.keep_card && redirect.link.is_none(), "{mode:?}");
    }
    let edit = fix_edit(Some("."));
    let redirect = plan(&facts(&edit, None), SelfServiceMode::LinkFirst, &links);
    assert_eq!(redirect.case, RedirectCase::FixedRun);
    assert!(redirect.keep_card && redirect.link.is_none());
}

#[test]
fn links_carry_only_ids_and_the_slot() {
    let links = PublicPortalLinks::new("https://Kanade-Pub.example.dev/").unwrap();
    assert_eq!(links.origin(), ORIGIN);
    let uuid = "0f8fad5b-d9cb-469f-a165-70867728950e";
    assert_eq!(
        links.move_run(uuid, utc(9, 27, 20)).as_deref(),
        Some(format!("{ORIGIN}/runs/{uuid}?move_to=2026-09-27T20:00:00Z").as_str())
    );
    let mine = run(&[AUTHOR]);
    let change = move_to(utc(9, 27, 20));
    let redirect = plan(
        &facts(&change, Some(&mine)),
        SelfServiceMode::CardsAndLink,
        &links,
    );
    let url = redirect.link.unwrap().url;
    assert!(!url.contains(AUTHOR) && !url.contains("900") && !url.contains("token"));
    assert_eq!(
        links
            .request_fixed(uuid, FixedChange::Remove, None, None)
            .as_deref(),
        Some(format!("{ORIGIN}/requests/new?fixed={uuid}&change=remove").as_str())
    );
}

#[test]
fn only_bare_https_origins_are_accepted() {
    for bad in [
        "http://kanade-pub.example.dev",
        "https://",
        "https://kanade-pub.example.dev/path",
        "https://user@kanade-pub.example.dev",
        "https://kanade-pub.example.dev?x=1",
        "kanade-pub.example.dev",
    ] {
        assert!(PublicPortalLinks::new(bad).is_err(), "{bad}");
    }
    assert!(PublicPortalLinks::new("https://127.0.0.1:8443").is_ok());
}
