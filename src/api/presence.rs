//! The presence board behind `/api/admin/presence`: which admins have an
//! Inbox member request or a run sheet open, and whether they are editing it.
//! It lives in this process's memory only: nothing here is written to the
//! store, History, backups or logs, no write path reads it (version checks
//! stay the only guard), and a restart starts it empty. It is bounded:
//! [`MAX_PLACES`] places of at most [`MAX_HERE`] tabs each (over a cap the
//! oldest goes), and a tab is gone [`LIFETIME`] after its last check-in.
//! Expired entries are swept on every call, so nothing needs a timer.

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use tokio::time::Instant;

use super::dto::{
    Named,
    presence::{Mode, PlaceKind, PresenceEntry},
};

/// How often an open, visible page checks in.
pub const CHECK_IN: Duration = Duration::from_secs(15);
/// A tab is gone this long after its last check-in.
pub const LIFETIME: Duration = Duration::from_secs(60);
/// Places held at once.
pub const MAX_PLACES: usize = 64;
/// Tabs held per place.
pub const MAX_HERE: usize = 8;
/// The longest place id accepted (row ids are 36-character UUIDs).
pub const MAX_ID: usize = 64;

/// A page: an Inbox member request or a run, by id. Unknown ids are fine
/// (they simply match nobody); only their shape is checked.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    kind: PlaceKind,
    id: String,
}

impl Place {
    /// `None` for an id that could name no row: empty, longer than
    /// [`MAX_ID`], or anything but ASCII letters, digits, `-` and `_`.
    pub fn new(kind: PlaceKind, id: &str) -> Option<Self> {
        let shaped = !id.is_empty()
            && id.len() <= MAX_ID
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        shaped.then(|| Self {
            kind,
            id: id.to_owned(),
        })
    }

    /// `<kind>:<id>`, as the read and leave queries name a place.
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, id) = text.split_once(':')?;
        Self::new(PlaceKind::parse(kind)?, id)
    }
}

/// Who checks in: their tab (one per sign-in session, so two tabs of one
/// browser are one entry) and the admin it belongs to.
#[derive(Clone, Copy, Debug)]
pub struct Visitor<'a> {
    pub tab: &'a str,
    pub admin: &'a str,
    pub name: &'a str,
}

/// One tab at one place. A tab that left stays as a tombstone (`left`) for
/// the same lifetime, so a check-in sent before the leave but arriving after
/// it (its `seq` no newer) cannot bring the tab back.
struct Entry {
    tab: String,
    admin: String,
    name: String,
    mode: Mode,
    seen: Instant,
    /// The tab's latest call: check-ins and leaves are ordered by it.
    seq: u64,
    left: bool,
}

#[derive(Default)]
pub struct Presence {
    places: Mutex<HashMap<Place, Vec<Entry>>>,
}

impl std::fmt::Debug for Presence {
    // Counts only: who is where never reaches a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Presence")
            .field("places", &self.lock().len())
            .finish()
    }
}

impl Presence {
    fn lock(&self) -> MutexGuard<'_, HashMap<Place, Vec<Entry>>> {
        self.places.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The visitor's tab is at `place` in `mode` now, as of its call `seq`.
    /// Answers the other admins there and whether what others see of the
    /// place changed (an arrival, a mode change or a tab pushed out by the
    /// cap). A check-in older than the tab's latest call, or no newer than
    /// its leave, changes nothing.
    pub fn check_in(
        &self,
        place: Place,
        visitor: Visitor<'_>,
        mode: Mode,
        seq: u64,
    ) -> (Vec<PresenceEntry>, bool) {
        let now = Instant::now();
        let mut places = self.lock();
        sweep(&mut places, now);
        let entries = room(&mut places, place);
        let before = admin_mode(entries, visitor.admin);
        let mut evicted = false;
        match entries.iter_mut().find(|entry| entry.tab == visitor.tab) {
            Some(entry) if seq < entry.seq || (entry.left && seq == entry.seq) => {}
            Some(entry) => {
                entry.mode = mode;
                entry.seen = now;
                entry.seq = seq;
                entry.left = false;
                if entry.name != visitor.name {
                    visitor.name.clone_into(&mut entry.name);
                }
            }
            None => {
                evicted = make_room(entries);
                entries.push(Entry {
                    tab: visitor.tab.to_owned(),
                    admin: visitor.admin.to_owned(),
                    name: visitor.name.to_owned(),
                    mode,
                    seen: now,
                    seq,
                    left: false,
                });
            }
        }
        let changed = evicted || before != admin_mode(entries, visitor.admin);
        (others(entries, visitor.admin, now), changed)
    }

    /// The admins at `place` other than `admin`, without checking in.
    pub fn here(&self, place: &Place, admin: &str) -> Vec<PresenceEntry> {
        let now = Instant::now();
        let mut places = self.lock();
        sweep(&mut places, now);
        places
            .get(place)
            .map(|entries| others(entries, admin, now))
            .unwrap_or_default()
    }

    /// The tab left `place` with its call `seq`; a leave older than the
    /// tab's latest check-in is ignored. The tab stays as a tombstone, also
    /// when the leave overtook its first check-in. Whether what others see
    /// of the place changed.
    pub fn leave(&self, place: Place, visitor: Visitor<'_>, seq: u64) -> bool {
        let now = Instant::now();
        let mut places = self.lock();
        sweep(&mut places, now);
        let entries = room(&mut places, place);
        let before = admin_mode(entries, visitor.admin);
        let mut evicted = false;
        match entries.iter_mut().find(|entry| entry.tab == visitor.tab) {
            Some(entry) if seq < entry.seq => {}
            Some(entry) => {
                entry.left = true;
                entry.seq = seq;
                entry.seen = now;
            }
            None => {
                evicted = make_room(entries);
                entries.push(Entry {
                    tab: visitor.tab.to_owned(),
                    admin: visitor.admin.to_owned(),
                    name: visitor.name.to_owned(),
                    mode: Mode::Viewing,
                    seen: now,
                    seq,
                    left: true,
                });
            }
        }
        evicted || before != admin_mode(entries, visitor.admin)
    }
}

/// The place's tabs, making room for it first (over [`MAX_PLACES`], the
/// place whose latest call is the oldest goes).
fn room(places: &mut HashMap<Place, Vec<Entry>>, place: Place) -> &mut Vec<Entry> {
    if !places.contains_key(&place)
        && places.len() >= MAX_PLACES
        && let Some(oldest) = places
            .iter()
            .min_by_key(|(_, entries)| entries.iter().map(|entry| entry.seen).max())
            .map(|(place, _)| place.clone())
    {
        places.remove(&oldest);
    }
    places.entry(place).or_default()
}

/// Over [`MAX_HERE`] the tab seen longest ago goes; whether a live one went.
fn make_room(entries: &mut Vec<Entry>) -> bool {
    if entries.len() < MAX_HERE {
        return false;
    }
    let Some(oldest) = entries
        .iter()
        .enumerate()
        .min_by_key(|(_, entry)| entry.seen)
        .map(|(index, _)| index)
    else {
        return false;
    };
    !entries.swap_remove(oldest).left
}

fn sweep(places: &mut HashMap<Place, Vec<Entry>>, now: Instant) {
    places.retain(|_, entries| {
        entries.retain(|entry| now.duration_since(entry.seen) < LIFETIME);
        !entries.is_empty()
    });
}

/// How `admin` shows at a place: editing if any of their present tabs is.
fn admin_mode(entries: &[Entry], admin: &str) -> Option<Mode> {
    entries
        .iter()
        .filter(|entry| entry.admin == admin && !entry.left)
        .map(|entry| entry.mode)
        .max_by_key(|mode| *mode == Mode::Editing)
}

/// Everyone but `caller`, one row per admin (editing if any tab is, seen at
/// their latest check-in): editors first, then the most recently seen.
fn others(entries: &[Entry], caller: &str, now: Instant) -> Vec<PresenceEntry> {
    let present: Vec<&Entry> = entries.iter().filter(|entry| !entry.left).collect();
    let mut rows: Vec<PresenceEntry> = Vec::new();
    for (index, entry) in present.iter().enumerate() {
        if entry.admin == caller
            || present[..index]
                .iter()
                .any(|earlier| earlier.admin == entry.admin)
        {
            continue;
        }
        let tabs = present[index..]
            .iter()
            .filter(|other| other.admin == entry.admin);
        let (mut mode, mut seen) = (entry.mode, entry.seen);
        for tab in tabs {
            if tab.mode == Mode::Editing {
                mode = Mode::Editing;
            }
            if tab.seen > seen {
                seen = tab.seen;
            }
        }
        rows.push(PresenceEntry {
            admin: Named {
                id: entry.admin.clone(),
                name: entry.name.clone(),
            },
            mode,
            seen_s: now.duration_since(seen).as_secs(),
        });
    }
    rows.sort_by(|a, b| {
        (b.mode == Mode::Editing)
            .cmp(&(a.mode == Mode::Editing))
            .then(a.seen_s.cmp(&b.seen_s))
            .then_with(|| a.admin.name.cmp(&b.admin.name))
    });
    rows
}

#[cfg(test)]
mod tests {
    use tokio::time::advance;

    use super::*;

    fn place(id: &str) -> Place {
        Place::new(PlaceKind::Request, id).unwrap()
    }

    fn visitor<'a>(tab: &'a str, admin: &'a str) -> Visitor<'a> {
        Visitor {
            tab,
            admin,
            name: admin,
        }
    }

    fn names(rows: &[PresenceEntry]) -> Vec<(&str, Mode, u64)> {
        rows.iter()
            .map(|row| (row.admin.name.as_str(), row.mode, row.seen_s))
            .collect()
    }

    #[test]
    fn places_are_a_known_kind_and_a_row_shaped_id() {
        assert_eq!(
            Place::parse("run:630b3544-aa"),
            Place::new(PlaceKind::Run, "630b3544-aa")
        );
        assert!(Place::parse("request:p-kalos_fixed").is_some());
        for bad in [
            "draft:d-weekend",
            "request:",
            "request",
            ":abc",
            "run:a b",
            "run:a/b",
            "run:a:b",
            "Run:abc",
        ] {
            assert_eq!(Place::parse(bad), None, "{bad}");
        }
        assert!(Place::new(PlaceKind::Run, &"a".repeat(MAX_ID)).is_some());
        assert!(Place::new(PlaceKind::Run, &"a".repeat(MAX_ID + 1)).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn an_entry_ages_and_is_gone_a_minute_after_its_last_check_in() {
        let board = Presence::default();
        board.check_in(place("p1"), visitor("t-ren", "ren"), Mode::Editing, 1);
        advance(Duration::from_secs(40)).await;
        assert_eq!(
            names(&board.here(&place("p1"), "asahi")),
            [("ren", Mode::Editing, 40)]
        );
        advance(Duration::from_secs(19)).await;
        assert_eq!(
            names(&board.here(&place("p1"), "asahi")),
            [("ren", Mode::Editing, 59)]
        );
        advance(Duration::from_secs(1)).await;
        assert!(board.here(&place("p1"), "asahi").is_empty());
        assert_eq!(board.lock().len(), 0, "the empty place is swept too");

        // A check-in renews the lifetime.
        board.check_in(place("p1"), visitor("t-ren", "ren"), Mode::Viewing, 2);
        advance(Duration::from_secs(50)).await;
        board.check_in(place("p1"), visitor("t-ren", "ren"), Mode::Viewing, 3);
        advance(Duration::from_secs(50)).await;
        assert_eq!(
            names(&board.here(&place("p1"), "asahi")),
            [("ren", Mode::Viewing, 50)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_caller_is_never_listed_and_an_admins_tabs_are_one_row() {
        let board = Presence::default();
        let (here, _) = board.check_in(place("p1"), visitor("t-asahi", "asahi"), Mode::Editing, 1);
        assert!(here.is_empty(), "alone");
        // Asahi's other tab sees nobody either: their own tabs are never listed.
        let (here, _) =
            board.check_in(place("p1"), visitor("t-asahi-2", "asahi"), Mode::Viewing, 1);
        assert!(here.is_empty());

        board.check_in(place("p1"), visitor("t-kaito", "kaito"), Mode::Viewing, 1);
        advance(Duration::from_secs(10)).await;
        board.check_in(place("p1"), visitor("t-ren-phone", "ren"), Mode::Viewing, 1);
        advance(Duration::from_secs(5)).await;
        board.check_in(
            place("p1"),
            visitor("t-ren-laptop", "ren"),
            Mode::Editing,
            1,
        );
        advance(Duration::from_secs(3)).await;
        let (here, _) = board.check_in(place("p1"), visitor("t-asahi", "asahi"), Mode::Editing, 2);
        assert_eq!(
            names(&here),
            [("ren", Mode::Editing, 3), ("kaito", Mode::Viewing, 18)],
            "editors first; Ren once, editing, seen at the latest tab"
        );
        assert_eq!(
            here[0].admin,
            Named {
                id: "ren".into(),
                name: "ren".into()
            }
        );
        // Another place is another list.
        assert!(board.here(&place("p2"), "asahi").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn changes_are_arrivals_mode_changes_and_leaves() {
        let board = Presence::default();
        let p = place("p1");
        let ren = |tab| visitor(tab, "ren");
        assert!(board.check_in(p.clone(), ren("t1"), Mode::Viewing, 1).1);
        assert!(
            !board.check_in(p.clone(), ren("t1"), Mode::Viewing, 2).1,
            "a heartbeat is no change"
        );
        assert!(board.check_in(p.clone(), ren("t1"), Mode::Editing, 3).1);
        assert!(
            !board.check_in(p.clone(), ren("t2"), Mode::Viewing, 1).1,
            "a second viewing tab of an editor shows nothing new"
        );
        assert!(!board.leave(p.clone(), ren("t2"), 2));
        assert!(board.leave(p.clone(), ren("t1"), 4), "Ren's last tab left");
        assert!(!board.leave(p.clone(), ren("t1"), 4), "already gone");
        assert!(!board.leave(place("p9"), ren("t1"), 1), "never there");
        assert!(board.here(&p, "asahi").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_in_that_arrives_after_its_leave_does_not_bring_the_tab_back() {
        let board = Presence::default();
        let p = place("p1");
        board.check_in(p.clone(), visitor("t1", "ren"), Mode::Editing, 2);
        // The page closes: its check-in seq 3 is aborted in flight, then the leave (seq 3) lands first.
        assert!(board.leave(p.clone(), visitor("t1", "ren"), 3));
        let (_, changed) = board.check_in(p.clone(), visitor("t1", "ren"), Mode::Editing, 3);
        assert!(!changed);
        assert!(
            board.here(&p, "asahi").is_empty(),
            "the late check-in is ignored"
        );
        // An older straggler is ignored too.
        board.check_in(p.clone(), visitor("t1", "ren"), Mode::Viewing, 1);
        assert!(board.here(&p, "asahi").is_empty());
        // Opened again: a newer seq is back.
        let (_, changed) = board.check_in(p.clone(), visitor("t1", "ren"), Mode::Viewing, 4);
        assert!(changed);
        assert_eq!(names(&board.here(&p, "asahi")), [("ren", Mode::Viewing, 0)]);
        // A leave older than the latest check-in is ignored.
        assert!(!board.leave(p.clone(), visitor("t1", "ren"), 3));
        assert_eq!(board.here(&p, "asahi").len(), 1);

        // A leave that overtook the tab's first check-in leaves a tombstone.
        board.leave(place("p2"), visitor("t2", "kaito"), 7);
        board.check_in(place("p2"), visitor("t2", "kaito"), Mode::Viewing, 6);
        assert!(board.here(&place("p2"), "asahi").is_empty());
        // Tombstones go with the lifetime (swept by the next call).
        advance(LIFETIME).await;
        board.here(&place("p1"), "asahi");
        assert!(board.lock().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_place_holds_eight_tabs_and_the_board_sixty_four_places() {
        let board = Presence::default();
        let p = place("p1");
        let tabs: Vec<String> = (0..=MAX_HERE).map(|n| format!("t{n}")).collect();
        for tab in &tabs {
            board.check_in(p.clone(), visitor(tab, tab), Mode::Viewing, 1);
            advance(Duration::from_secs(1)).await;
        }
        let here = board.here(&p, "asahi");
        assert_eq!(here.len(), MAX_HERE);
        assert!(
            here.iter().all(|row| row.admin.id != "t0"),
            "the oldest tab went"
        );
        // Tombstones count against the same cap.
        board.leave(p.clone(), visitor("gone", "gone"), 1);
        assert_eq!(board.lock()[&p].len(), MAX_HERE);

        for n in 0..=MAX_PLACES {
            board.leave(place(&format!("q{n}")), visitor("t", "ren"), 1);
            advance(Duration::from_millis(10)).await;
        }
        let places = board.lock();
        assert_eq!(places.len(), MAX_PLACES);
        assert!(!places.contains_key(&p), "the place seen longest ago went");
        assert!(places.contains_key(&place(&format!("q{MAX_PLACES}"))));
    }

    #[test]
    fn a_new_board_is_empty() {
        let board = Presence::default();
        assert!(board.here(&place("p1"), "asahi").is_empty());
        assert_eq!(format!("{board:?}"), "Presence { places: 0 }");
    }
}
