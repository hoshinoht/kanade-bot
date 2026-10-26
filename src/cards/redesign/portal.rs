//! "Open in portal" under the redesigned run cards (day-of, countdown): a
//! link button in a legacy action row beside the text and embeds, never a
//! Components V2 layout, since these cards ping (a V2 ping arrives as an
//! empty push notification). Only while the public portal is open; cards
//! are edited in place, so the next re-render adds or drops it.

use twilight_model::channel::message::Component;

use crate::extract::redirect::is_link_id;

use crate::cards::redesign::live::V2Kit;
use crate::cards::redesign::v2::{action_row, link_button};

pub const OPEN_IN_PORTAL: &str = "Open in portal";

/// Discord's legacy limits: action rows per message, buttons per row.
pub const MAX_ROWS: usize = 5;
pub const MAX_ROW_BUTTONS: usize = 5;

/// The run's portal page for one run; the root for several (or none) and
/// for an id unsafe in a URL (the notices' rule, [`is_link_id`]).
fn run_page(origin: &str, run_ids: &[&str]) -> String {
    match run_ids {
        [id] if is_link_id(id) => format!("{origin}/runs/{id}"),
        _ => format!("{origin}/"),
    }
}

/// `rows` with "Open in portal" for the card's `run_ids` while the portal is
/// open (`kit` is `None` where nothing links, such as the admin preview):
/// appended to the last row when it has room, else in a new row while
/// Discord takes one, else left off.
pub fn add_portal_button(rows: &mut Vec<Component>, kit: Option<&V2Kit>, run_ids: &[&str]) {
    let Some(origin) = kit.and_then(V2Kit::portal_url) else {
        return;
    };
    let button = link_button(OPEN_IN_PORTAL, &run_page(origin, run_ids));
    if let Some(Component::ActionRow(row)) = rows.last_mut()
        && row.components.len() < MAX_ROW_BUTTONS
    {
        row.components.push(button);
    } else if rows.len() < MAX_ROWS {
        rows.push(action_row(vec![button]));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::v2::button;
    use super::*;
    use twilight_model::channel::message::component::{Button, ButtonStyle};

    const ORIGIN: &str = "https://portal.example";
    const RUN: &str = "a1b2c3d4-0000-4000-8000-000000000001";

    fn kit(origin: Option<&str>, open: bool) -> V2Kit {
        V2Kit {
            portal: origin.map(str::to_owned),
            portal_open: Some(Arc::new(move || open)),
            ..V2Kit::default()
        }
    }

    fn links(rows: &[Component]) -> Vec<(String, String)> {
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
                }) => Some((label.clone(), url.clone())),
                _ => None,
            })
            .collect()
    }

    fn rendered(kit: Option<&V2Kit>, run_ids: &[&str]) -> Vec<Component> {
        let mut rows = Vec::new();
        add_portal_button(&mut rows, kit, run_ids);
        rows
    }

    #[test]
    fn one_run_links_its_page_while_the_portal_is_open() {
        let open = kit(Some(ORIGIN), true);
        assert_eq!(
            links(&rendered(Some(&open), &[RUN])),
            [(OPEN_IN_PORTAL.to_owned(), format!("{ORIGIN}/runs/{RUN}"))]
        );
    }

    #[test]
    fn closed_or_unconfigured_portals_add_nothing() {
        assert!(rendered(Some(&kit(Some(ORIGIN), false)), &[RUN]).is_empty());
        assert!(rendered(Some(&kit(None, true)), &[RUN]).is_empty());
        assert!(rendered(Some(&V2Kit::default()), &[RUN]).is_empty());
        assert!(rendered(None, &[RUN]).is_empty());
    }

    #[test]
    fn unsafe_ids_and_several_runs_open_the_root() {
        let open = kit(Some(ORIGIN), true);
        let root = format!("{ORIGIN}/");
        for run_ids in [
            &["../admin"][..],
            &["a b"],
            &["%2E%2E"],
            &[""],
            &[RUN, "e5f6a7b8-0000-4000-8000-000000000002"],
            &[],
        ] {
            assert_eq!(
                links(&rendered(Some(&open), run_ids)),
                [(OPEN_IN_PORTAL.to_owned(), root.clone())],
                "{run_ids:?}"
            );
        }
    }

    #[test]
    fn the_button_joins_a_row_with_room_else_a_new_row_within_limits() {
        let open = kit(Some(ORIGIN), true);
        let press = || button(ButtonStyle::Secondary, "x", "x".to_owned(), false);
        let mut rows = vec![action_row(vec![press(), press()])];
        add_portal_button(&mut rows, Some(&open), &[RUN]);
        assert_eq!(rows.len(), 1);
        assert_eq!(links(&rows).len(), 1);

        let mut rows = vec![action_row((0..MAX_ROW_BUTTONS).map(|_| press()).collect())];
        add_portal_button(&mut rows, Some(&open), &[RUN]);
        assert_eq!(rows.len(), 2);
        assert_eq!(links(&rows).len(), 1);

        let full = || action_row((0..MAX_ROW_BUTTONS).map(|_| press()).collect());
        let mut rows: Vec<Component> = (0..MAX_ROWS).map(|_| full()).collect();
        add_portal_button(&mut rows, Some(&open), &[RUN]);
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(links(&rows).is_empty(), "no room left: left off");
    }
}
