//! The History graph boards' data (`B_HistoryGraph*` in
//! docs/research/2026-10-12-graph-presence-mockups), built on the seeded
//! week by `POST /__mock/graph/board`: History grows to head #39 and the
//! drafts become the boards' six — three open (Weekend reshuffle on #36,
//! Bellona clean-up on #38, Kalos to Saturdays on #34), Tuesday Kalos
//! (#31, merged as #33), Scratch (#32, discarded Mon 21:00) and an expired
//! draft from #12. Every record is a real change to the mock's week.

use serde_json::json;

use super::Store;
use super::clock::{TZ_OFFSET_SECS, iso_z, local_now, week_start};
use super::drafts::{MockDraft, SeededConflict, StagedOp, op};
use super::history::Actor;

/// Minutes after the boss week's start (Thu 00:00 guild time).
const fn hm(day: i64, hour: i64, minute: i64) -> i64 {
    (day * 24 + hour) * 60 + minute
}

/// A guild-local minute of this boss week as an instant the mock writes.
fn local(minute: i64) -> String {
    let (today, _) = local_now();
    iso_z(week_start(today) * 1440 + minute)
}

/// One History record's change.
enum Edit {
    /// Flip a participant between in and maybe.
    Answer(&'static str, &'static str),
    Status(&'static str, &'static str),
    Move(&'static str, u8, &'static str),
}

/// The filler between the seeded #9 and #31: members answering on Discord.
const FILLER: [(&str, &str); 5] = [
    ("r-fa", "1011"),
    ("r-jupiter", "1008"),
    ("r-bm", "1008"),
    ("r-limbo", "1008"),
    ("r-bellona", "1009"),
];

impl Store {
    fn board_edit(&mut self, edit: &Edit) {
        let Some(run) = self.runs.iter_mut().find(|run| {
            run.id
                == match edit {
                    Edit::Answer(id, _) | Edit::Status(id, _) | Edit::Move(id, _, _) => *id,
                }
        }) else {
            return;
        };
        match edit {
            Edit::Answer(_, member) => {
                if let Some(p) = run.participants.iter_mut().find(|p| p.id == *member) {
                    p.answer = if p.answer == "yes" { "maybe" } else { "yes" };
                }
            }
            Edit::Status(_, status) => run.status = status,
            Edit::Move(_, day, time) => {
                run.day = *day;
                run.time = Some((*time).into());
            }
        }
    }

    fn board_record(
        &mut self,
        minute: i64,
        actor: Actor,
        surface: &'static str,
        edit: Edit,
    ) -> bool {
        let before = self.rows();
        self.board_edit(&edit);
        self.version += 1;
        let at = Self::start(false) * 86_400 - TZ_OFFSET_SECS + minute * 60;
        self.push_record(&before, actor, surface, at, Vec::new(), None)
            .is_some()
    }

    /// `POST /__mock/graph/board`: the boards' History and drafts.
    pub fn graph_board(&mut self) {
        // #10–#30: Sun 18:30 onwards, nine minutes apart.
        let mut minute = hm(3, 18, 30);
        while self.history.len() <= 30 {
            let (run, member) = FILLER[self.history.len() % FILLER.len()];
            // Every filler flips a seeded answer; a missing one would loop forever.
            if !self.board_record(
                minute,
                Actor::new("member", member),
                "discord",
                Edit::Answer(run, member),
            ) {
                break;
            }
            minute += 9;
        }
        let admin = Actor::discord_admin;
        let board: [(i64, Actor, &'static str, Edit); 9] = [
            (
                hm(3, 21, 47),
                Actor::new("member", "1005"),
                "discord",
                Edit::Answer("r-kalos", "1005"),
            ),
            (
                hm(3, 22, 30),
                Actor::new("member", "1002"),
                "discord",
                Edit::Answer("r-kalos", "1002"),
            ),
            (
                hm(3, 23, 20),
                admin("1001"),
                "draft_merge",
                Edit::Move("r-kalos", 5, "22:30"),
            ),
            (
                hm(4, 19, 5),
                admin("1001"),
                "admin_portal",
                Edit::Answer("r-bellona", "1009"),
            ),
            (
                hm(4, 20, 12),
                admin("1001"),
                "extraction_approval",
                Edit::Answer("r-jupiter", "1012"),
            ),
            (
                hm(4, 21, 40),
                admin("1001"),
                "admin_portal",
                Edit::Answer("r-jupiter", "1001"),
            ),
            (
                hm(5, 9, 30),
                Actor::new("member", "1003"),
                "discord",
                Edit::Answer("r-limbo", "1003"),
            ),
            (
                hm(5, 10, 42),
                admin("1002"),
                "admin_portal",
                Edit::Move("r-carling", 6, "21:00"),
            ),
            (
                hm(5, 11, 5),
                admin("1009"),
                "admin_portal",
                Edit::Status("r-bm", "cancelled"),
            ),
        ];
        for (at, actor, surface, edit) in board {
            self.board_record(at, actor, surface, edit);
        }
        self.drafts = board_drafts();
    }
}

fn staged(kind: &'static str, summary: &str, run: &str, body: serde_json::Value) -> StagedOp {
    op(kind, summary, run, body, 0)
}

fn weekly(summary: &str) -> StagedOp {
    StagedOp {
        kind: "apply_fixed_edit",
        summary: summary.into(),
        run_id: None,
        fixed_id: Some("f-kalos".into()),
        body: json!({"kind": "fixed_time", "fixed": "f-kalos"}),
        added_at: local(hm(4, 22, 15)),
    }
}

#[allow(clippy::too_many_arguments)]
fn draft(
    id: &str,
    title: &str,
    status: &'static str,
    base: (Option<u64>, Option<u64>),
    ops: Vec<StagedOp>,
    conflicts: Vec<SeededConflict>,
    created: i64,
    updated: i64,
    merged_seq: Option<u64>,
) -> MockDraft {
    MockDraft {
        id: id.into(),
        title: title.into(),
        status,
        version: 1 + ops.len() as u64,
        base_lag: base.0,
        pinned_base: base.1,
        ops,
        conflicts,
        created_at: local(created),
        updated_at: local(updated),
        merged_seq,
        close_reason: None,
    }
}

fn conflict(
    key: &'static str,
    kind: &'static str,
    run_id: &'static str,
    removal: Option<&'static str>,
    ops: Vec<usize>,
    record: (u64, &'static str, &'static str),
) -> SeededConflict {
    SeededConflict {
        key,
        kind,
        run_id,
        field: removal.is_none().then_some("slot"),
        values: removal.is_none().then_some((
            "2026-09-29T14:00:00+00:00",
            "2026-09-30T13:30:00+00:00",
            "2026-09-30T13:00:00+00:00",
        )),
        removal,
        message: "changed here and in History",
        ops,
        settle: vec!["keep", "take"],
        record,
    }
}

/// The boards' drafts; the open ones follow the head (`base_lag`), the
/// closed ones keep their base.
fn board_drafts() -> Vec<MockDraft> {
    let move_op = |summary: &str, run: &str, day: u8, time: &str| {
        staged(
            "amend_run",
            summary,
            run,
            json!({"kind": "move", "run": run, "day": day, "time": time}),
        )
    };
    let status_op = |summary: &str, run: &str, status: &str| {
        staged(
            "set_status",
            summary,
            run,
            json!({"kind": "status", "run": run, "status": status}),
        )
    };
    let party_op = |summary: &str, run: &str, add: &str| {
        staged(
            "swap_participants",
            summary,
            run,
            json!({"kind": "party", "run": run, "add": [add]}),
        )
    };
    vec![
        draft(
            "d-weekend",
            "Weekend reshuffle",
            "open",
            (Some(3), None),
            vec![
                move_op(
                    "Move HCarling + HStar to Wed 30 Sep 21:30",
                    "r-carling",
                    6,
                    "21:30",
                ),
                party_op("Party of HCarling + HStar: +Hinata", "r-carling", "1006"),
                status_op("Set XBM to confirmed", "r-bm", "confirmed"),
                party_op("Party of XBM: +Yuzu", "r-bm", "1004"),
                move_op("Move HFA to Wed 30 Sep 23:00", "r-fa", 6, "23:00"),
                status_op("Set NBellona to cancelled", "r-bellona", "cancelled"),
            ],
            vec![
                conflict(
                    "both/run:r-carling/slot",
                    "both_changed",
                    "r-carling",
                    None,
                    vec![0],
                    (1, "discord:1002", "Ren"),
                ),
                conflict(
                    "removed/run:r-bm",
                    "upstream_removed",
                    "r-bm",
                    Some("cancelled"),
                    vec![2, 3],
                    (0, "discord:1009", "Kaito"),
                ),
            ],
            hm(4, 21, 50),
            hm(5, 11, 48),
            None,
        ),
        draft(
            "d-bellona",
            "Bellona clean-up",
            "open",
            (Some(1), None),
            vec![party_op("Party of NBellona: +Minato", "r-bellona", "1012")],
            vec![conflict(
                "both/run:r-carling/slot",
                "both_changed",
                "r-carling",
                None,
                vec![0],
                (1, "discord:1002", "Ren"),
            )],
            hm(5, 10, 45),
            hm(5, 10, 50),
            None,
        ),
        draft(
            "d-kalos",
            "Kalos to Saturdays",
            "open",
            (Some(5), None),
            vec![
                weekly("Change weekly XKalos to Saturday 21:30"),
                weekly("Party of weekly XKalos: +Yuzu"),
            ],
            Vec::new(),
            hm(4, 19, 10),
            hm(4, 22, 15),
            None,
        ),
        draft(
            "d-tuesday",
            "Tuesday Kalos",
            "merged",
            (None, Some(31)),
            vec![move_op(
                "Move XKalos to Tue 29 Sep 22:30",
                "r-kalos",
                5,
                "22:30",
            )],
            Vec::new(),
            hm(3, 21, 50),
            hm(3, 23, 20),
            Some(33),
        ),
        draft(
            "d-scratch",
            "Scratch",
            "discarded",
            (None, Some(32)),
            Vec::new(),
            Vec::new(),
            hm(3, 22, 35),
            hm(4, 21, 0),
            None,
        ),
        draft(
            "d-untitled",
            "Untitled draft",
            "expired",
            (None, Some(12)),
            vec![move_op("Move HFA to Sun 27 Sep 20:00", "r-fa", 3, "20:00")],
            Vec::new(),
            hm(3, 18, 50),
            // Between #24 and #25.
            hm(3, 20, 40),
            None,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use serde_json::Value;

    fn ids(graph: &Value) -> Vec<&str> {
        let mut ids: Vec<&str> = graph["drafts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn the_board_scenario_draws_the_boards_lanes() {
        let mut s = store();
        s.graph_board();
        let all = s.draft_graph(Some("from=1&to=39")).ok().unwrap();
        assert_eq!(all["head"], 39);
        assert_eq!(
            ids(&all),
            [
                "d-bellona",
                "d-kalos",
                "d-scratch",
                "d-tuesday",
                "d-untitled",
                "d-weekend"
            ]
        );
        let row = |id: &str| {
            all["drafts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["id"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(
            (
                row("d-weekend")["base"].as_u64(),
                row("d-bellona")["base"].as_u64(),
                row("d-kalos")["base"].as_u64()
            ),
            (Some(36), Some(38), Some(34))
        );
        assert_eq!(row("d-tuesday")["merged_seq"], 33);
        assert_eq!(row("d-kalos")["scope"], "weekly");
        assert_eq!(row("d-untitled")["closed_by"]["name"], "Kanade");
        assert!(row("d-weekend")["closed_at"].is_null());
        assert_eq!(
            row("d-scratch")["closed_at"],
            row("d-scratch")["updated_at"]
        );
        // The merge record is a draft merge; times run forward.
        assert_eq!(s.record(33).unwrap().surface, "draft_merge");
        assert!(s.history.windows(2).skip(1).all(|w| w[0].at < w[1].at));
        // #25–#28: the expired draft ended below, the open ones start above.
        assert!(ids(&s.draft_graph(Some("from=25&to=28")).ok().unwrap()).is_empty());
        assert_eq!(
            ids(&s.draft_graph(Some("from=25&to=39")).ok().unwrap()),
            [
                "d-bellona",
                "d-kalos",
                "d-scratch",
                "d-tuesday",
                "d-weekend"
            ]
        );
        // #18–#24: the expired draft's lane passes.
        assert!(ids(&s.draft_graph(Some("from=18&to=24")).ok().unwrap()).contains(&"d-untitled"));
        assert!(ids(&s.draft_graph(Some("from=34&to=39")).ok().unwrap()).contains(&"d-scratch"));
        assert!(!ids(&s.draft_graph(Some("from=37&to=39")).ok().unwrap()).contains(&"d-scratch"));
    }

    #[test]
    fn the_graph_read_refuses_bad_ranges() {
        let s = store();
        for query in [
            None,
            Some("from=1"),
            Some("from=2&to=1"),
            Some("from=a&to=1"),
            Some("from=1&to=1&x=1"),
            Some("from=1&from=1&to=2"),
        ] {
            assert!(s.draft_graph(query).is_err(), "{query:?}");
        }
        // The default seed: the closed drafts keep their bases.
        let graph = s.draft_graph(Some("from=1&to=9")).ok().unwrap();
        assert_eq!(graph["head"], 9);
        assert_eq!(
            ids(&graph),
            ["d-carling", "d-scratch", "d-tuesday", "d-weekend"]
        );
    }
}
