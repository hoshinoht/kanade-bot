//! Weekly timings (v4 /fixed) and the boss catalog pages.

use super::catalog::{BossRow, parse_bosses};
use super::clock::valid_time;
use super::dto::*;
use super::seed::{self, Fixed, Rec};
use super::{MoveError, Store};

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const OPEN: [&str; 4] = ["planned", "confirmed", "at_risk", "otot"];

/// Day, time, bosses and roster from the timing; answers carry over for people who stay.
pub fn apply_timing(run: &mut Rec, timing: &Fixed) {
    run.day = seed::day_of(timing.weekday);
    if run.status != "otot" {
        run.time = Some(timing.time.clone());
    }
    run.bosses = timing.bosses.clone();
    run.channel = timing.channel;
    let before = std::mem::take(&mut run.participants);
    run.participants = timing
        .participants
        .iter()
        .filter_map(|id| {
            let kept = before.iter().find(|p| p.id == *id).cloned();
            kept.or_else(|| {
                seed::member_name(id).map(|(id, name)| Participant {
                    id,
                    name,
                    answer: "waiting",
                })
            })
        })
        .collect();
}

/// As the server: a rostered member (bossing role), not necessarily in the party.
fn roster_owner(id: &str) -> Result<&'static str, MoveError> {
    seed::members()
        .into_iter()
        .find(|m| m.id == id.trim() && m.bossing)
        .map(|m| m.id)
        .ok_or(MoveError::invalid("Pick an owner from the roster."))
}

impl Store {
    fn fixed_row(&self, f: &Fixed) -> FixedRow {
        let (channel_id, channel_name, watched) =
            seed::channel(f.channel).unwrap_or((f.channel, f.channel, false));
        FixedRow {
            id: f.id.clone(),
            short_id: f.short_id.clone(),
            weekday: f.weekday,
            weekday_name: WEEKDAYS[usize::from(f.weekday)],
            time: f.time.clone(),
            bosses: self.bosses(&f.bosses),
            participants: f
                .participants
                .iter()
                .filter_map(|id| seed::member_name(id))
                .map(|(id, name)| Named {
                    id: id.into(),
                    name: name.into(),
                })
                .collect(),
            channel_id,
            channel_name,
            channel_watched: watched,
            owner: seed::member_name(f.owner_id).map_or(f.owner_id, |m| m.1),
            owner_id: f.owner_id,
            note: f.note.clone(),
            runs: self
                .runs
                .iter()
                .filter(|r| {
                    r.fixed_id.as_deref() == Some(f.id.as_str()) && OPEN.contains(&r.status)
                })
                .map(|r| FixedRunLink {
                    run_id: r.id.clone(),
                    short_id: r.short_id.clone(),
                    week: if r.next_week { "next" } else { "this" },
                    day: r.day,
                    time: r.time.clone(),
                    status: r.status,
                    amended: self.amended(r),
                })
                .collect(),
        }
    }

    pub fn fixed_rows(&self) -> Vec<FixedRow> {
        let mut rows: Vec<&Fixed> = self.fixed.iter().filter(|f| !f.retired).collect();
        rows.sort_by(|a, b| (a.weekday, &a.time).cmp(&(b.weekday, &b.time)));
        rows.into_iter().map(|f| self.fixed_row(f)).collect()
    }

    fn validated(req: &FixedRequest) -> Result<Fixed, MoveError> {
        if req.weekday > 6 {
            return Err(MoveError::invalid("Pick a weekday."));
        }
        if !valid_time(&req.time) {
            return Err(MoveError::invalid("Times are HH:MM, 00:00 to 23:59."));
        }
        let bosses = parse_bosses(&req.bosses).map_err(MoveError::Invalid)?;
        let (channel, _, _) =
            seed::channel(&req.channel_id).ok_or(MoveError::invalid("Pick a home channel."))?;
        let mut participants = Vec::new();
        for id in &req.participants {
            let (id, _) = seed::member_name(id).ok_or(MoveError::invalid("No such member."))?;
            if !participants.contains(&id) {
                participants.push(id);
            }
        }
        if participants.is_empty() {
            return Err(MoveError::invalid("A timing needs at least one member."));
        }
        // The server's default for a sign-in that names no Discord user.
        let owner_id = participants[0];
        Ok(Fixed {
            id: String::new(),
            short_id: String::new(),
            weekday: req.weekday,
            time: req.time.clone(),
            bosses,
            participants,
            channel,
            note: req.note.clone().filter(|n| !n.trim().is_empty()),
            owner_id,
            retired: false,
        })
    }

    /// A new timing materialises a run this week (if its day is still ahead) and next week.
    pub fn create_fixed(&mut self, req: FixedRequest) -> Result<FixedRow, MoveError> {
        let mut timing = Self::validated(&req)?;
        if let Some(owner) = &req.owner_id {
            timing.owner_id = roster_owner(owner)?;
        }
        let (id, short_id) = self.fresh_id("f");
        timing.id = id;
        timing.short_id = short_id;
        let today = (super::local_now().0 - Self::start(false)) as u8;
        for next in [false, true] {
            if !next && seed::day_of(timing.weekday) < today {
                continue;
            }
            let (id, short_id) = self.fresh_id("r");
            let mut run = Rec {
                id,
                short_id,
                next_week: next,
                day: 0,
                time: None,
                status: "planned",
                bosses: Vec::new(),
                participants: Vec::new(),
                channel: timing.channel,
                fixed_id: Some(timing.id.clone()),
            };
            apply_timing(&mut run, &timing);
            self.runs.push(run);
        }
        self.fixed.push(timing);
        self.version += 1;
        let created = self.fixed.last().ok_or(MoveError::NotFound)?;
        Ok(self.fixed_row(created))
    }

    /// Unamended runs follow the new timing. Amended runs need a decision each:
    /// `update` takes the new timing, `keep` leaves this week's change alone.
    /// Per-field stale check as on the server: a resent field that differs from the
    /// stored row and changed after `version` is `409 stale`.
    pub fn update_fixed(&mut self, id: &str, req: FixedRequest) -> Result<FixedRow, MoveError> {
        let version = req.version.ok_or_else(|| {
            MoveError::Coded(
                422,
                "version_required",
                "Send the week version the timing was loaded at.".into(),
            )
        })?;
        let mut timing = Self::validated(&req)?;
        let index = self
            .fixed
            .iter()
            .position(|f| f.id == id && !f.retired)
            .ok_or(MoveError::NotFound)?;
        let old = &self.fixed[index];
        // Omitted keeps the owner; checked against the roster only when it changes.
        timing.owner_id = match req.owner_id.as_deref().map(str::trim) {
            Some(owner) if owner != old.owner_id => roster_owner(owner)?,
            _ => old.owner_id,
        };
        // As the server: each field the form changes must not have moved since `version`.
        let changed = [
            ("weekday", old.weekday != timing.weekday),
            ("time", old.time != timing.time),
            ("bosses", old.bosses != timing.bosses),
            ("participants", old.participants != timing.participants),
            ("channel_id", old.channel != timing.channel),
            ("note", old.note != timing.note),
            ("owner_id", old.owner_id != timing.owner_id),
        ];
        // Nothing differs: the server answers 200 without writing, before any other check.
        if changed.iter().all(|(_, differs)| !differs) {
            return Ok(self.fixed_row(old));
        }
        // Stale before choices, as the server checks preconditions first.
        let key = serde_json::json!({ "table": "fixed_runs", "id": id });
        if changed
            .iter()
            .any(|(field, differs)| *differs && self.changed_after(&key, field, version))
        {
            return Err(MoveError::Stale);
        }
        let amended: Vec<String> = self
            .runs
            .iter()
            .filter(|r| {
                r.fixed_id.as_deref() == Some(id) && OPEN.contains(&r.status) && self.amended(r)
            })
            .map(|r| r.id.clone())
            .collect();
        if let Some(missing) = amended.iter().find(|r| {
            !matches!(
                req.decisions.get(*r).map(String::as_str),
                Some("update" | "keep")
            )
        }) {
            return Err(MoveError::Coded(
                422,
                "choices_required",
                format!("Choose update or keep for amended run {missing}."),
            ));
        }
        let old = &self.fixed[index];
        timing.id = old.id.clone();
        timing.short_id = old.short_id.clone();
        for run in self
            .runs
            .iter_mut()
            .filter(|r| r.fixed_id.as_deref() == Some(id) && OPEN.contains(&r.status))
        {
            let keep = amended.contains(&run.id)
                && req.decisions.get(&run.id).map(String::as_str) == Some("keep");
            if !keep {
                apply_timing(run, &timing);
            }
        }
        self.fixed[index] = timing;
        self.version += 1;
        Ok(self.fixed_row(&self.fixed[index]))
    }

    /// Retiring a timing cancels its runs that have not happened (v4 "Remove").
    pub fn retire_fixed(&mut self, id: &str) -> Result<usize, MoveError> {
        let timing = self
            .fixed
            .iter_mut()
            .find(|f| f.id == id && !f.retired)
            .ok_or(MoveError::NotFound)?;
        timing.retired = true;
        let mut cancelled = 0;
        for run in self
            .runs
            .iter_mut()
            .filter(|r| r.fixed_id.as_deref() == Some(id) && OPEN.contains(&r.status))
        {
            run.status = "cancelled";
            cancelled += 1;
        }
        self.version += 1;
        Ok(cancelled)
    }

    pub fn validate_bosses(&self, text: &str) -> Result<ValidateResult, MoveError> {
        let bosses = parse_bosses(text).map_err(MoveError::Invalid)?;
        Ok(ValidateResult {
            bosses: self.bosses(&bosses),
        })
    }

    pub fn boss_rows(&self) -> Vec<BossRow> {
        let in_use: Vec<String> = self
            .fixed
            .iter()
            .filter(|f| !f.retired)
            .flat_map(|f| f.bosses.iter().map(|b| b.token.clone()))
            .collect();
        self.catalog.rows(&in_use)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use crate::mock::{MoveError, Store, dto::FixedRequest, history::Actor};
    use std::collections::HashMap;

    fn request(time: &str, decisions: &[(&str, &str)], version: Option<u64>) -> FixedRequest {
        FixedRequest {
            weekday: 4,
            time: time.into(),
            bosses: "xkalos".into(),
            participants: vec!["1001".into(), "1002".into(), "1005".into(), "1006".into()],
            channel_id: "kalos-four".into(),
            note: None,
            owner_id: None,
            decisions: decisions
                .iter()
                .map(|(a, b)| ((*a).into(), (*b).into()))
                .collect::<HashMap<_, _>>(),
            version,
        }
    }

    #[test]
    fn editing_a_timing_asks_about_amended_runs() {
        let mut s = store();
        assert!(
            s.update_fixed("f-kalos", request("21:00", &[], Some(s.version)))
                .is_err(),
            "r-kalos is amended"
        );
        let row = s
            .update_fixed(
                "f-kalos",
                request("21:00", &[("r-kalos", "keep")], Some(s.version)),
            )
            .ok()
            .unwrap();
        let week = s.week(false);
        assert_eq!(
            week.runs
                .iter()
                .find(|r| r.id == "r-kalos")
                .unwrap()
                .time
                .as_deref(),
            Some("22:00")
        );
        assert_eq!(
            s.week(true)
                .runs
                .iter()
                .find(|r| r.id == "n-kalos")
                .unwrap()
                .time
                .as_deref(),
            Some("21:00")
        );
        assert_eq!(row.time, "21:00");
        s.update_fixed(
            "f-kalos",
            request("20:30", &[("r-kalos", "update")], Some(s.version)),
        )
        .ok()
        .unwrap();
        assert_eq!(
            s.week(false)
                .runs
                .iter()
                .find(|r| r.id == "r-kalos")
                .unwrap()
                .time
                .as_deref(),
            Some("20:30")
        );
    }

    #[test]
    fn edits_need_the_version_they_were_loaded_at_and_conflict_per_field() {
        let mut s = store();
        let keep = [("r-kalos", "keep")];
        match s.update_fixed("f-kalos", request("21:00", &keep, None)) {
            Err(MoveError::Coded(422, "version_required", _)) => {}
            Err(other) => panic!("wanted version_required, got {other}"),
            Ok(_) => panic!("an edit without a version was applied"),
        }
        let loaded = s.version;
        let edit = |s: &mut Store, req: FixedRequest| {
            s.tracked(Actor::admin(), "admin_portal", |s| {
                s.update_fixed("f-kalos", req)
            })
        };
        edit(&mut s, request("21:00", &keep, Some(loaded)))
            .ok()
            .unwrap();
        // A form loaded before that edit resends the old time: refused, not reverted.
        assert!(matches!(
            edit(&mut s, request("20:00", &keep, Some(loaded))),
            Err(MoveError::Stale)
        ));
        // A field nobody touched since merges freely at the same old version.
        let mut noted = request("21:00", &keep, Some(loaded));
        noted.note = Some("bring potions".into());
        let row = edit(&mut s, noted).ok().unwrap();
        assert_eq!(row.note.as_deref(), Some("bring potions"));
    }

    #[test]
    fn a_no_op_save_answers_the_row_and_stale_comes_before_choices() {
        let mut s = store();
        let loaded = s.version;
        let edit = |s: &mut Store, req: FixedRequest| {
            s.tracked(Actor::admin(), "admin_portal", |s| {
                s.update_fixed("f-kalos", req)
            })
        };
        // The stored values resent: 200 with the row, no choices asked, nothing written.
        let row = edit(&mut s, request("21:30", &[], Some(loaded)))
            .ok()
            .unwrap();
        assert_eq!((row.time.as_str(), s.version), ("21:30", loaded));
        match edit(&mut s, request("21:00", &[], Some(loaded))) {
            Err(MoveError::Coded(422, "choices_required", _)) => {}
            Err(other) => panic!("wanted choices_required, got {other}"),
            Ok(_) => panic!("applied without a choice for the amended run"),
        }
        edit(
            &mut s,
            request("21:00", &[("r-kalos", "keep")], Some(loaded)),
        )
        .ok()
        .unwrap();
        // A stale form without choices is refused as stale, not asked for choices.
        assert!(matches!(
            edit(&mut s, request("20:00", &[], Some(loaded))),
            Err(MoveError::Stale)
        ));
    }

    #[test]
    fn owners_are_rostered_settable_and_conflict_per_field() {
        let mut s = store();
        let mut create = request("19:00", &[], None);
        create.owner_id = Some("1012".into());
        let row = s.create_fixed(create).ok().unwrap();
        assert_eq!((row.owner_id, row.owner), ("1012", "Minato"));
        let loaded = s.version;
        let edit = |s: &mut Store, req: FixedRequest| {
            s.tracked(Actor::admin(), "admin_portal", |s| {
                s.update_fixed("f-kalos", req)
            })
        };
        let keep = [("r-kalos", "keep")];
        let mut bad = request("21:30", &keep, Some(loaded));
        bad.owner_id = Some("1014".into());
        assert!(matches!(edit(&mut s, bad), Err(MoveError::Invalid(_))));
        let mut owned = request("21:30", &keep, Some(loaded));
        owned.owner_id = Some("1004".into());
        let row = edit(&mut s, owned).ok().unwrap();
        assert_eq!(row.owner_id, "1004");
        // Omitted keeps it; another owner from the old form is stale.
        let row = edit(&mut s, request("21:30", &keep, Some(loaded)))
            .ok()
            .unwrap();
        assert_eq!(row.owner_id, "1004");
        let mut stale = request("21:30", &keep, Some(loaded));
        stale.owner_id = Some("1001".into());
        assert!(matches!(edit(&mut s, stale), Err(MoveError::Stale)));
    }

    #[test]
    fn retiring_cancels_open_runs_and_creating_materialises() {
        let mut s = store();
        assert_eq!(s.retire_fixed("f-carling").ok(), Some(2));
        assert!(s.fixed_rows().iter().all(|f| f.id != "f-carling"));
        let created = s.create_fixed(request("19:00", &[], None)).ok().unwrap();
        assert!(!created.runs.is_empty());
        assert!(s.validate_bosses("hstar, cfoo").is_err());
        assert!(
            s.validate_bosses("cstar").is_err(),
            "Radiant Malefic Star has no Chaos"
        );
        assert_eq!(
            s.validate_bosses("hstar xkalos").ok().unwrap().bosses.len(),
            2
        );
    }
}
