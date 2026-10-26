//! "Open in portal" under the day-of and countdown cards: a legacy link
//! row beside the text and embeds while the public portal is open, to the
//! run's page (the root for several runs or an unsafe id); nothing when it
//! is closed or unconfigured.

use std::sync::Arc;

use kanade::bot::delivery::cards::redesign::{
    MAX_COMPONENTS, MAX_EMBEDS, MAX_ROW_BUTTONS, MAX_ROWS, OPEN_IN_PORTAL, component_count,
};
use kanade::bot::delivery::cards::{self, Card, CardArt, CardContext, V2Kit, fetch_art};
use kanade::bot::transport::{COMPONENTS_V2, OutgoingMessage, legacy_rows, v2_body_valid};
use kanade::domain::catalog::BossTable;
use kanade::domain::members::Roster;
use kanade::domain::notify::IntentContent;
use kanade::domain::schedule::ScheduleSnapshot;
use twilight_model::channel::message::component::{Button, ButtonStyle};
use twilight_model::channel::message::{Component, MessageFlags};

use super::{KALOS, MALEFIC, art, countdown, day_of, kalos_copies, members, redesigned_v5, week};
use crate::cards::catalog;

const ORIGIN: &str = "https://portal.example";

fn portal(origin: Option<&str>, open: bool) -> V2Kit {
    V2Kit {
        portal: origin.map(str::to_owned),
        portal_open: Some(Arc::new(move || open)),
        ..V2Kit::default()
    }
}

fn styled<'a>(
    schedule: &'a ScheduleSnapshot,
    roster: &'a Roster,
    table: &'a BossTable,
    kit: Option<&'a V2Kit>,
) -> CardContext<'a> {
    CardContext {
        v2: kit,
        ..redesigned_v5(schedule, roster, table)
    }
}

async fn post(card: &Card) -> OutgoingMessage {
    let pictures = fetch_art(Some(&art()), card, true).await;
    card.message(&[], &pictures)
}

/// Every link button in `rows`: (label, url).
fn links(rows: &[Component]) -> Vec<(&str, &str)> {
    rows.iter()
        .flat_map(|row| match row {
            Component::ActionRow(row) => row.components.as_slice(),
            _ => &[],
        })
        .filter_map(|component| match component {
            Component::Button(Button {
                style: ButtonStyle::Link,
                label: Some(label),
                url: Some(url),
                ..
            }) => Some((label.as_str(), url.as_str())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_open_portal_links_the_run_from_the_post_and_every_edit() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let kit = portal(Some(ORIGIN), true);
    let ctx = styled(&schedule, &roster, &table, Some(&kit));
    let page = format!("{ORIGIN}/runs/{MALEFIC}");
    for content in [countdown(MALEFIC), day_of(&[MALEFIC])] {
        let card = cards::build(&content, &ctx, None, &[]).unwrap();
        assert!(card.components.is_empty(), "never V2: the card pings");
        let message = post(&card).await;
        assert_eq!(
            links(&message.components),
            [(OPEN_IN_PORTAL, page.as_str())]
        );
        assert!(!message.embeds.is_empty(), "the embeds stay");
        // Discord takes it as a legacy post: text, embeds and button rows.
        assert!(legacy_rows(message.content.as_deref(), &message.components));
        assert!(v2_body_valid(
            message.content.as_deref(),
            &message.embeds,
            &message.components,
            MessageFlags::empty(),
        ));
        assert!(!v2_body_valid(
            message.content.as_deref(),
            &message.embeds,
            &message.components,
            COMPONENTS_V2,
        ));
        let edit = card.edit(&CardArt::default());
        assert!(edit.valid() && edit.is_legacy() && edit.v2_components().is_none());
        assert!(edit.embeds.is_some());
        assert_eq!(
            links(edit.components.as_deref().unwrap()),
            [(OPEN_IN_PORTAL, page.as_str())]
        );
    }
}

/// The rendered tree of one countdown: a single row holding the link.
#[tokio::test]
async fn the_countdown_row_as_discord_receives_it() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let kit = portal(Some(ORIGIN), true);
    let ctx = styled(&schedule, &roster, &table, Some(&kit));
    let card = cards::build(&countdown(MALEFIC), &ctx, None, &[]).unwrap();
    let message = post(&card).await;
    assert_eq!(
        serde_json::to_value(&message.components).unwrap(),
        serde_json::json!([{
            "type": 1,
            "components": [{
                "type": 2,
                "style": 5,
                "label": OPEN_IN_PORTAL,
                "url": format!("{ORIGIN}/runs/{MALEFIC}"),
            }],
        }])
    );
}

#[tokio::test]
async fn a_closed_or_unconfigured_portal_adds_nothing_and_an_edit_clears_the_row() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let closed = portal(Some(ORIGIN), false);
    let unconfigured = portal(None, true);
    let no_switch = V2Kit {
        portal: Some(ORIGIN.to_owned()),
        ..V2Kit::default()
    };
    for kit in [Some(&closed), Some(&unconfigured), Some(&no_switch), None] {
        let ctx = styled(&schedule, &roster, &table, kit);
        for content in [countdown(MALEFIC), day_of(&[MALEFIC, KALOS])] {
            let card = cards::build(&content, &ctx, None, &[]).unwrap();
            assert_eq!(card.rows, Some(Vec::new()));
            assert!(post(&card).await.components.is_empty());
            let edit = card.edit(&CardArt::default());
            assert_eq!(edit.components, Some(Vec::new()), "a stale row is cleared");
            assert!(edit.valid() && edit.v2_components().is_none());
        }
    }
}

#[tokio::test]
async fn several_runs_or_an_unsafe_id_open_the_portal_root() {
    let (mut schedule, roster, table) = (week(), members(), catalog());
    let unsafe_id = "../admin";
    let mut odd = schedule
        .runs
        .iter()
        .find(|run| run.id == MALEFIC)
        .unwrap()
        .clone();
    odd.id = unsafe_id.to_owned();
    schedule.runs.push(odd);
    let kit = portal(Some(ORIGIN), true);
    let ctx = styled(&schedule, &roster, &table, Some(&kit));
    let root = format!("{ORIGIN}/");
    for content in [
        day_of(&[MALEFIC, KALOS]),
        countdown(unsafe_id),
        day_of(&[unsafe_id]),
    ] {
        let card = cards::build(&content, &ctx, None, &[]).unwrap();
        assert_eq!(
            links(&post(&card).await.components),
            [(OPEN_IN_PORTAL, root.as_str())],
            "{content:?}"
        );
    }
}

/// The largest card (more runs than embeds) still carries one row with
/// one button, inside Discord's limits.
#[tokio::test]
async fn the_largest_card_stays_within_the_component_limits() {
    let (mut schedule, roster, table) = (week(), members(), catalog());
    let run_ids = kalos_copies(&mut schedule, 25, |_| {});
    let kit = portal(Some(ORIGIN), true);
    let ctx = styled(&schedule, &roster, &table, Some(&kit));
    let content = IntentContent::DayOf { run_ids };
    let card = cards::build(&content, &ctx, None, &[]).unwrap();
    let message = post(&card).await;
    assert_eq!(message.embeds.len(), MAX_EMBEDS);
    assert!(message.components.len() <= MAX_ROWS);
    assert!(message.components.iter().all(|row| matches!(
        row,
        Component::ActionRow(row) if row.components.len() <= MAX_ROW_BUTTONS
    )));
    assert!(component_count(&message.components) <= MAX_COMPONENTS);
    assert_eq!(
        links(&message.components),
        [(OPEN_IN_PORTAL, "https://portal.example/")]
    );
}
