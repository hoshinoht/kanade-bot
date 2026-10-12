//! Admin drafts (`drafts.json`): staged changes that are not live until a
//! merge. Seeded with an up-to-date draft, one whose base moved under it
//! (a both-changed slot and an upstream cancellation, as the
//! `B_DraftsConflict` board draws), and closed ones. Merging applies the
//! staged moves and status changes through the same mutations as the run
//! sheet, as one History record; party, reset and weekly-timing changes are
//! listed but not applied by the mock.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::clock::{iso_z, local_now, week_start};
use super::dto::{MoveRequest, StatusRequest};
use super::{MoveError, Store};

const UNTITLED: &str = "Untitled draft";

#[derive(Clone, Serialize)]
pub struct Decider {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
}

fn asahi() -> Decider {
    Decider {
        kind: "admin",
        id: "discord:1001".into(),
        name: "Asahi".into(),
    }
}

/// One staged change and the body it was staged with (applied on merge).
#[derive(Clone)]
pub struct StagedOp {
    pub kind: &'static str,
    pub summary: String,
    pub run_id: Option<String>,
    pub fixed_id: Option<String>,
    pub body: Value,
    pub added_at: String,
}

/// A seeded conflict, shown while the draft's base is behind the head.
#[derive(Clone)]
pub struct SeededConflict {
    pub key: &'static str,
    pub kind: &'static str,
    pub run_id: &'static str,
    pub field: Option<&'static str>,
    pub values: Option<(&'static str, &'static str, &'static str)>,
    pub removal: Option<&'static str>,
    pub message: &'static str,
    pub ops: Vec<usize>,
    pub settle: Vec<&'static str>,
    /// Seq offset below the head and who made it.
    pub record: (u64, &'static str, &'static str),
}

#[derive(Clone)]
pub struct MockDraft {
    pub id: String,
    pub title: String,
    pub status: &'static str,
    pub version: u64,
    /// `None`: on the current head (seeded drafts follow the mock's head).
    pub base_lag: Option<u64>,
    /// The base as a fixed seq: a closed draft's (pinned when it closes, so
    /// later changes never move it) and the graph scenario's.
    pub pinned_base: Option<u64>,
    pub ops: Vec<StagedOp>,
    pub conflicts: Vec<SeededConflict>,
    pub created_at: String,
    pub updated_at: String,
    pub merged_seq: Option<u64>,
    pub close_reason: Option<String>,
}

/// Body of `POST …/ops`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddOp {
    pub version: Option<u64>,
    pub op: Value,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Rebase {
    pub version: Option<u64>,
    #[serde(default)]
    pub settle: std::collections::BTreeMap<String, String>,
}

pub(super) fn at(hour: i64) -> String {
    let (today, _) = local_now();
    iso_z(week_start(today) * 1440 + hour * 60)
}

pub(super) fn op(kind: &'static str, summary: &str, run: &str, body: Value, hour: i64) -> StagedOp {
    StagedOp {
        kind,
        summary: summary.into(),
        run_id: Some(run.into()),
        fixed_id: None,
        body,
        added_at: at(hour),
    }
}

pub fn seed() -> Vec<MockDraft> {
    vec![
        MockDraft {
            id: "d-weekend".into(),
            title: "Weekend reshuffle".into(),
            status: "open",
            version: 3,
            base_lag: None,
            pinned_base: None,
            ops: vec![
                op(
                    "amend_run",
                    "Move XKalos to Wed 30 Sep 21:30",
                    "r-kalos",
                    json!({"kind": "move", "run": "r-kalos", "day": 6, "time": "21:30"}),
                    110,
                ),
                op(
                    "swap_participants",
                    "Party of XKalos: +Hinata",
                    "r-kalos",
                    json!({"kind": "party", "run": "r-kalos", "add": ["1006"]}),
                    111,
                ),
            ],
            conflicts: Vec::new(),
            created_at: at(109),
            updated_at: at(111),
            merged_seq: None,
            close_reason: None,
        },
        MockDraft {
            id: "d-carling".into(),
            title: "Carling retime".into(),
            status: "open",
            version: 4,
            base_lag: Some(3),
            pinned_base: None,
            ops: vec![
                op(
                    "amend_run",
                    "Move HCarling to Sun 27 Sep 21:00",
                    "r-carling",
                    json!({"kind": "move", "run": "r-carling", "day": 3, "time": "21:00"}),
                    100,
                ),
                op(
                    "swap_participants",
                    "Party of NLimbo: +Yuzu",
                    "r-limbo",
                    json!({"kind": "party", "run": "r-limbo", "add": ["1004"]}),
                    101,
                ),
                op(
                    "set_status",
                    "Set NLimbo to confirmed",
                    "r-limbo",
                    json!({"kind": "status", "run": "r-limbo", "status": "confirmed"}),
                    102,
                ),
            ],
            conflicts: vec![
                SeededConflict {
                    key: "both/run:r-carling/slot",
                    kind: "both_changed",
                    run_id: "r-carling",
                    field: Some("slot"),
                    values: Some((
                        "2026-09-26T13:00:00+00:00",
                        "2026-09-27T13:00:00+00:00",
                        "2026-09-26T14:00:00+00:00",
                    )),
                    removal: None,
                    message: "run r-carling slot: changed here and in History",
                    ops: vec![0],
                    settle: vec!["keep", "take"],
                    record: (1, "discord:1002", "Ren"),
                },
                SeededConflict {
                    key: "removed/run:r-limbo",
                    kind: "upstream_removed",
                    run_id: "r-limbo",
                    field: None,
                    values: None,
                    removal: Some("cancelled"),
                    message: "run r-limbo was cancelled since",
                    ops: vec![1, 2],
                    settle: vec!["keep", "take"],
                    record: (0, "discord:1002", "Ren"),
                },
            ],
            created_at: at(99),
            updated_at: at(102),
            merged_seq: None,
            close_reason: None,
        },
        MockDraft {
            id: "d-tuesday".into(),
            title: "Tuesday Kalos".into(),
            status: "merged",
            version: 2,
            base_lag: None,
            // Based on #2, merged as #3 (the change that moved XKalos to 22:00).
            pinned_base: Some(2),
            ops: vec![op(
                "amend_run",
                "Move XKalos to Tue 29 Sep 22:00",
                "r-kalos",
                json!({"kind": "move", "run": "r-kalos", "day": 5, "time": "22:00"}),
                60,
            )],
            conflicts: Vec::new(),
            created_at: at(38),
            updated_at: at(41),
            merged_seq: Some(3),
            close_reason: None,
        },
        MockDraft {
            id: "d-scratch".into(),
            title: "Scratch".into(),
            status: "discarded",
            version: 1,
            base_lag: None,
            pinned_base: Some(1),
            ops: Vec::new(),
            conflicts: Vec::new(),
            created_at: at(40),
            updated_at: at(43),
            merged_seq: None,
            close_reason: None,
        },
    ]
}

fn coded(status: u16, code: &'static str, message: impl Into<String>) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

fn not_found() -> MoveError {
    coded(404, "not_found", "You have no draft with that id.")
}

fn version_required() -> MoveError {
    coded(
        422,
        "version_required",
        "Name the draft version you edited (`version`).",
    )
}

fn stale() -> MoveError {
    coded(
        409,
        "stale",
        "The draft changed or was closed since you loaded it; reload it.",
    )
}

impl Store {
    fn drafts_head(&self) -> u64 {
        self.history.last().map_or(0, |record| record.seq)
    }

    fn base_of(&self, draft: &MockDraft) -> u64 {
        draft.pinned_base.unwrap_or_else(|| {
            self.drafts_head()
                .saturating_sub(draft.base_lag.unwrap_or(0))
        })
    }

    fn draft_row(&self, draft: &MockDraft) -> Value {
        let head = self.drafts_head();
        let base = self.base_of(draft);
        let titled = draft.title != UNTITLED;
        let title = match (titled, draft.ops.first()) {
            (true, _) | (false, None) => draft.title.clone(),
            (false, Some(first)) if draft.ops.len() == 1 => first.summary.clone(),
            (false, Some(first)) => format!("{} and {} more", first.summary, draft.ops.len() - 1),
        };
        let closed = draft.status != "open";
        // As the server derives it: weekly timings only when every op is one.
        let weekly = !draft.ops.is_empty() && draft.ops.iter().all(|op| op.run_id.is_none());
        let closer = || match draft.status {
            "expired" => Decider {
                kind: "system",
                id: "delivery".into(),
                name: "Kanade".into(),
            },
            _ => asahi(),
        };
        json!({
            "id": draft.id,
            "short_id": draft.id.trim_start_matches("d-"),
            "title": title,
            "titled": titled,
            "status": draft.status,
            "version": draft.version,
            "base": base,
            "up_to_date": base == head,
            "scope": if weekly { "weekly" } else { "week" },
            "week": (!weekly).then_some("2026-09-24"),
            "staged": draft.ops.len(),
            "author": asahi(),
            "created_at": draft.created_at,
            "updated_at": draft.updated_at,
            "merged_seq": draft.merged_seq,
            "closed_by": closed.then(closer),
            "close_reason": draft.close_reason,
        })
    }

    fn find_draft(&self, id: &str) -> Result<usize, MoveError> {
        self.drafts
            .iter()
            .position(|draft| draft.id == id)
            .ok_or_else(not_found)
    }

    fn live(&self, id: &str, version: Option<u64>) -> Result<usize, MoveError> {
        let index = self.find_draft(id)?;
        let version = version.ok_or_else(version_required)?;
        let draft = &self.drafts[index];
        if draft.status != "open" || draft.version != version {
            return Err(stale());
        }
        Ok(index)
    }

    fn written(&self, index: usize, message: &str) -> Value {
        let draft = &self.drafts[index];
        json!({
            "id": draft.id,
            "version": draft.version,
            "status": draft.status,
            "seq": draft.merged_seq,
            "message": message,
        })
    }

    fn touch(&mut self, index: usize) {
        let draft = &mut self.drafts[index];
        draft.version += 1;
        draft.updated_at = super::clock::iso_now();
    }

    pub fn draft_list(&self, state: Option<&str>) -> Result<Value, MoveError> {
        let open = match state {
            None | Some("open") => true,
            Some("closed") => false,
            Some(_) => return Err(coded(422, "invalid_query", "state is open or closed.")),
        };
        let rows: Vec<Value> = self
            .drafts
            .iter()
            .filter(|draft| (draft.status == "open") == open)
            .map(|draft| self.draft_row(draft))
            .collect();
        Ok(json!({"head": self.drafts_head(), "drafts": rows}))
    }

    pub fn draft_detail(&self, id: &str) -> Result<Value, MoveError> {
        let draft = &self.drafts[self.find_draft(id)?];
        let mut value = self.draft_row(draft);
        value["ops"] = draft
            .ops
            .iter()
            .enumerate()
            .map(|(ord, op)| {
                json!({
                    "ord": ord,
                    "kind": op.kind,
                    "summary": op.summary,
                    "run_id": op.run_id,
                    "fixed_id": op.fixed_id,
                    "created_by": null,
                    "author": asahi(),
                    "added_at": op.added_at,
                })
            })
            .collect();
        Ok(value)
    }

    pub fn draft_preview(&self, id: &str) -> Result<Value, MoveError> {
        let draft = &self.drafts[self.find_draft(id)?];
        let head = self.drafts_head();
        let base = self.base_of(draft);
        let conflicts: Vec<Value> = draft
            .conflicts
            .iter()
            .map(|c| {
                json!({
                    "key": c.key,
                    "kind": c.kind,
                    "run_id": c.run_id,
                    "fixed_id": null,
                    "field": c.field,
                    "base": c.values.map(|v| v.0),
                    "yours": c.values.map(|v| v.1),
                    "now": c.values.map(|v| v.2),
                    "removal": c.removal,
                    "message": c.message,
                    "ops": c.ops,
                    "settle": c.settle,
                    "record": {
                        "seq": head.saturating_sub(c.record.0),
                        "actor": {"kind": "admin", "id": c.record.1, "name": c.record.2},
                        "at": at(105),
                    },
                })
            })
            .collect();
        let rows: Vec<Value> = draft
            .ops
            .iter()
            .filter_map(|op| {
                let run = self
                    .runs
                    .iter()
                    .find(|run| Some(&run.id) == op.run_id.as_ref())?;
                Some(json!({
                    "key": {"table": "runs", "id": run.id},
                    "before": {"id": run.id, "status": run.status},
                    "after": {"id": run.id, "status": run.status, "staged": op.summary},
                }))
            })
            .collect();
        let clean = conflicts.is_empty();
        Ok(json!({
            "id": draft.id,
            "version": draft.version,
            "base": base,
            "head": head,
            "up_to_date": base == head,
            "mergeable": base == head && clean && !rows.is_empty() && draft.status == "open",
            "rows": rows,
            "conflicts": conflicts,
            "consequence": clean.then_some("Party unchanged · 2 reminders will move"),
        }))
    }

    pub fn draft_create(&mut self, title: Option<&str>) -> Value {
        let title = title
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or(UNTITLED);
        self.next_id += 1;
        let now = super::clock::iso_now();
        self.drafts.push(MockDraft {
            id: format!("d-new-{}", self.next_id),
            title: title.into(),
            status: "open",
            version: 1,
            base_lag: None,
            pinned_base: None,
            ops: Vec::new(),
            conflicts: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            merged_seq: None,
            close_reason: None,
        });
        self.written(
            self.drafts.len() - 1,
            "Draft saved; nothing is live until it is merged.",
        )
    }

    pub fn draft_retitle(
        &mut self,
        id: &str,
        version: Option<u64>,
        title: &str,
    ) -> Result<Value, MoveError> {
        let index = self.live(id, version)?;
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 200 {
            return Err(coded(
                422,
                "invalid_title",
                "A draft title is 1 to 200 characters.",
            ));
        }
        self.drafts[index].title = title.into();
        self.touch(index);
        Ok(self.written(index, "Draft saved; nothing is live until it is merged."))
    }

    pub fn draft_discard(&mut self, id: &str, version: Option<u64>) -> Result<Value, MoveError> {
        let index = self.live(id, version)?;
        self.drafts[index].pinned_base = Some(self.base_of(&self.drafts[index]));
        self.drafts[index].status = "discarded";
        self.drafts[index].updated_at = super::clock::iso_now();
        Ok(self.written(index, "Draft discarded; nothing was applied."))
    }

    pub fn draft_add(&mut self, id: &str, req: AddOp) -> Result<Value, MoveError> {
        let index = self.live(id, req.version)?;
        let run = req.op["run"].as_str().map(str::to_owned);
        let fixed = req.op["fixed"].as_str().map(str::to_owned);
        let (kind, summary) = match req.op["kind"].as_str() {
            Some("move") => ("amend_run", "Move"),
            Some("status") => ("set_status", "Set status of"),
            Some("party") => ("swap_participants", "Party of"),
            Some("reset") => ("reset_to_fixed", "Reset"),
            Some("fixed_party") => ("fixed_participants", "Party of weekly"),
            Some("fixed_time") => ("apply_fixed_edit", "Change weekly"),
            _ => return Err(coded(422, "invalid_body", "The request body is not valid.")),
        };
        let target = run.clone().or_else(|| fixed.clone()).unwrap_or_default();
        if run.as_ref().is_some_and(|run| {
            !run.starts_with("created:") && self.runs.iter().all(|r| &r.id != run)
        }) {
            return Err(coded(422, "invalid", "That run no longer exists."));
        }
        self.drafts[index].ops.push(StagedOp {
            kind,
            summary: format!("{summary} {target}"),
            run_id: run,
            fixed_id: fixed,
            body: req.op,
            added_at: super::clock::iso_now(),
        });
        self.touch(index);
        Ok(self.written(index, "Draft saved; nothing is live until it is merged."))
    }

    pub fn draft_remove(
        &mut self,
        id: &str,
        ord: &str,
        version: Option<u64>,
    ) -> Result<Value, MoveError> {
        let index = self.live(id, version)?;
        let ord: usize = ord.parse().map_err(|_| not_found())?;
        if ord >= self.drafts[index].ops.len() {
            return Err(coded(
                422,
                "op_needed",
                format!("the draft has no operation {ord}"),
            ));
        }
        self.drafts[index].ops.remove(ord);
        for conflict in &mut self.drafts[index].conflicts {
            conflict.ops.retain(|o| *o != ord);
            for o in &mut conflict.ops {
                if *o > ord {
                    *o -= 1;
                }
            }
        }
        self.touch(index);
        Ok(self.written(index, "Draft saved; nothing is live until it is merged."))
    }

    pub fn draft_rebase(&mut self, id: &str, req: Rebase) -> Result<Value, MoveError> {
        let index = self.live(id, req.version)?;
        let draft = &self.drafts[index];
        let mut drop: Vec<usize> = Vec::new();
        let mut revive: Vec<(usize, &'static str)> = Vec::new();
        for conflict in &draft.conflicts {
            match req.settle.get(conflict.key).map(String::as_str) {
                Some("take") => drop.extend(&conflict.ops),
                Some("keep") if conflict.removal.is_some() => {
                    revive.push((conflict.ops.first().copied().unwrap_or(0), conflict.run_id));
                }
                Some("keep") => {}
                Some(_) => {
                    return Err(coded(
                        422,
                        "invalid",
                        "Settle each conflict with keep or take.",
                    ));
                }
                None => {
                    return Err(coded(
                        409,
                        "conflicts",
                        "The draft has unsettled conflicts with the schedule; open its preview.",
                    ));
                }
            }
        }
        let mut ops = Vec::new();
        for (ord, staged) in draft.ops.iter().enumerate() {
            for (_, run) in revive.iter().filter(|(at, _)| *at == ord) {
                ops.push(op(
                    "set_status",
                    &format!("Revive {run}"),
                    run,
                    json!({"kind": "status", "run": run, "status": "planned"}),
                    0,
                ));
            }
            if !drop.contains(&ord) {
                ops.push(staged.clone());
            }
        }
        let draft = &mut self.drafts[index];
        draft.ops = ops;
        draft.conflicts.clear();
        draft.base_lag = None;
        draft.pinned_base = None;
        self.touch(index);
        Ok(self.written(index, "Draft rebased; your changes are kept."))
    }

    pub fn draft_merge(&mut self, id: &str, version: Option<u64>) -> Result<Value, MoveError> {
        let index = self.live(id, version)?;
        if self.base_of(&self.drafts[index]) != self.drafts_head() {
            return Err(coded(
                409,
                "base_moved",
                "the schedule moved; rebase the draft first",
            ));
        }
        if self.drafts[index].ops.is_empty() {
            return Err(coded(422, "empty", "Stage a change before merging."));
        }
        let ops = self.drafts[index].ops.clone();
        let actor = self.session_actor();
        self.tracked(actor, "draft_merge", |s| {
            for staged in &ops {
                let run = staged.body["run"].as_str().unwrap_or_default();
                match staged.body["kind"].as_str() {
                    Some("move") => {
                        let request = MoveRequest {
                            day: staged.body["day"].as_u64().unwrap_or(0) as u8,
                            time: staged.body["time"].as_str().map(str::to_owned),
                            version: s.version,
                        };
                        s.move_run(run, request)?;
                    }
                    Some("status") => {
                        let request = StatusRequest {
                            status: staged.body["status"].as_str().unwrap_or_default().into(),
                            version: s.version,
                        };
                        s.set_status(run, request)?;
                    }
                    _ => {}
                }
            }
            Ok(())
        })?;
        let seq = self.drafts_head();
        // The base it merged from: the head just below its merge record.
        let base = seq.saturating_sub(1);
        let draft = &mut self.drafts[index];
        draft.pinned_base = Some(base);
        draft.status = "merged";
        draft.merged_seq = Some(seq);
        draft.updated_at = super::clock::iso_now();
        Ok(json!({
            "id": draft.id,
            "version": draft.version,
            "status": "merged",
            "seq": seq,
            "message": format!("Draft merged as change #{seq}."),
        }))
    }

    /// `GET /api/admin/drafts/graph?from=&to=` (`DraftGraph`): the drafts
    /// whose span touches records `from..=to`, as the server decides it; a
    /// closed draft's close time is its last change (`updated_at`).
    pub fn draft_graph(&self, query: Option<&str>) -> Result<Value, MoveError> {
        let invalid = || {
            coded(
                422,
                "invalid_query",
                "from and to are History seqs, from <= to.",
            )
        };
        let pairs: Vec<(&str, &str)> = query
            .unwrap_or_default()
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
            .collect();
        let seq = |key: &str| -> Result<u64, MoveError> {
            let mut values = pairs.iter().filter(|(name, _)| *name == key);
            match (values.next(), values.next()) {
                (Some((_, text)), None)
                    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    text.parse::<u64>()
                        .ok()
                        .filter(|seq| *seq <= i64::MAX as u64)
                        .ok_or_else(invalid)
                }
                _ => Err(invalid()),
            }
        };
        if pairs
            .iter()
            .any(|(name, _)| *name != "from" && *name != "to")
        {
            return Err(invalid());
        }
        let (from, to) = (seq("from")?, seq("to")?);
        if from > to {
            return Err(invalid());
        }
        let head = self.drafts_head();
        let secs = |text: &str| super::clock::parse_instant(&text.replace("+00:00", "Z"));
        let from_at = (from > 0)
            .then(|| self.history.get(from as usize).and_then(|r| secs(&r.at)))
            .flatten();
        let mut touching: Vec<&MockDraft> = self
            .drafts
            .iter()
            .filter(|draft| {
                let reaches = match draft.status {
                    "open" => head >= from,
                    "merged" => draft.merged_seq.is_some_and(|seq| seq >= from),
                    _ => from_at
                        .is_none_or(|first| secs(&draft.updated_at).is_none_or(|at| at >= first)),
                };
                self.base_of(draft) <= to && reaches
            })
            .collect();
        // Most recently changed first, as the server lists them.
        touching.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let rows: Vec<Value> = touching
            .into_iter()
            .map(|draft| {
                let mut row = self.draft_row(draft);
                row["closed_at"] = (draft.status != "open")
                    .then(|| draft.updated_at.clone())
                    .into();
                row
            })
            .collect();
        Ok(json!({"head": head, "drafts": rows}))
    }
}
