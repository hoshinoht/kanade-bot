//! Admin presence as the server keeps it (`src/api/presence.rs`): in
//! memory, gone 60 s after a check-in, the caller never listed. The mock has
//! one admin (the signed-in session), so the others are simulated: e2e and
//! dev put them at a place (`POST /__mock/presence`), and
//! `POST /__mock/presence/board` seeds the presence boards. A held entry keeps
//! its age (dev captures); any other ages in real time.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A tab is gone this long after its last check-in.
pub const LIFETIME: Duration = Duration::from_secs(60);
pub const MAX_HERE: usize = 8;

/// `<kind>:<id>` when `kind` is a place kind and `id` is row-shaped.
pub fn place(kind: &str, id: &str) -> Option<String> {
    let shaped = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    (matches!(kind, "request" | "run") && shaped).then(|| format!("{kind}:{id}"))
}

/// `<kind>:<id>` as the read and leave queries send it.
pub fn parse(at: &str) -> Option<String> {
    let (kind, id) = at.split_once(':')?;
    place(kind, id)
}

struct Other {
    place: String,
    id: String,
    name: String,
    editing: bool,
    seen: Instant,
    held: Option<u64>,
}

impl Other {
    fn age(&self) -> u64 {
        self.held.unwrap_or_else(|| self.seen.elapsed().as_secs())
    }
}

#[derive(Default)]
pub struct Board {
    others: Vec<Other>,
}

impl Board {
    /// The simulated admins at `place`: editors first, then the freshest.
    pub fn here(&mut self, place: &str) -> Value {
        self.others
            .retain(|o| o.held.is_some() || o.seen.elapsed() < LIFETIME);
        let mut rows: Vec<&Other> = self.others.iter().filter(|o| o.place == place).collect();
        rows.sort_by_key(|o| (!o.editing, o.age()));
        Value::Array(
            rows.into_iter()
                .take(MAX_HERE)
                .map(|o| {
                    json!({
                        "admin": {"id": o.id, "name": o.name},
                        "mode": if o.editing { "editing" } else { "viewing" },
                        "seen_s": o.age(),
                    })
                })
                .collect(),
        )
    }

    /// Puts (or moves) a simulated admin at `place`, seen `seen_s` ago.
    pub fn put(
        &mut self,
        place: String,
        id: String,
        name: String,
        editing: bool,
        seen_s: u64,
        held: bool,
    ) {
        self.others.retain(|o| !(o.place == place && o.id == id));
        self.others.push(Other {
            place,
            id,
            name,
            editing,
            seen: Instant::now()
                .checked_sub(Duration::from_secs(seen_s))
                .unwrap_or_else(Instant::now),
            held: held.then_some(seen_s),
        });
    }

    /// A simulated admin left `place`; whether they were there.
    pub fn take(&mut self, place: &str, id: &str) -> bool {
        let before = self.others.len();
        self.others.retain(|o| !(o.place == place && o.id == id));
        self.others.len() != before
    }

    pub fn clear(&mut self) {
        self.others.clear();
    }

    /// The presence boards: Ren editing the Inbox request `p-kalos-fixed`
    /// with Kaito viewing it 40 s ago (B_PresenceInbox), and Ren editing
    /// HCarling + HStar (B_PresenceRunSheet). Held, so they keep those ages.
    pub fn board(&mut self) {
        self.clear();
        let ren = || ("discord:1002".to_owned(), "Ren".to_owned());
        let (id, name) = ren();
        self.put("request:p-kalos-fixed".into(), id, name, true, 0, true);
        self.put(
            "request:p-kalos-fixed".into(),
            "discord:1009".into(),
            "Kaito".into(),
            false,
            40,
            true,
        );
        let (id, name) = ren();
        self.put("run:r-carling".into(), id, name, true, 0, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_known_kinds_with_row_shaped_ids() {
        assert_eq!(parse("run:r-carling").as_deref(), Some("run:r-carling"));
        for bad in [
            "draft:d-weekend",
            "run:",
            "run:a b",
            "request",
            &format!("run:{}", "a".repeat(65)),
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_board_seed_lists_editors_first_and_keeps_its_ages() {
        let mut board = Board::default();
        board.board();
        let here = board.here("request:p-kalos-fixed");
        assert_eq!(here[0]["admin"]["name"], "Ren");
        assert_eq!(here[0]["mode"], "editing");
        assert_eq!(here[1]["admin"]["name"], "Kaito");
        assert_eq!(here[1]["seen_s"], 40);
        assert_eq!(board.here("run:r-carling").as_array().unwrap().len(), 1);
        assert!(board.take("run:r-carling", "discord:1002"));
        assert_eq!(board.here("run:r-carling"), json!([]));
    }

    #[test]
    fn an_unheld_admin_is_gone_after_a_minute() {
        let mut board = Board::default();
        board.put(
            "run:r-x".into(),
            "discord:1002".into(),
            "Ren".into(),
            false,
            61,
            false,
        );
        assert_eq!(board.here("run:r-x"), json!([]));
    }
}
