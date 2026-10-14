//! An administrator's changes to a member request while approving it (user
//! decision 2026-10-11): a new day and time, or changes left out. The
//! request's stored operations never change; the approval merges the
//! edited ones and records the edit in its `merged` event note, so the
//! member's own words stay what they asked.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveTime, Timelike, Weekday};

use super::RequestRefusal;
use crate::domain::drafts::DraftOp;

spelled! {
    /// Which part of an operation a change is: the whole operation, or one
    /// field group of a weekly-timing change.
    Part {
        Whole => "whole",
        DayTime => "day_time",
        Party => "party",
        Channel => "channel",
    }
}

impl Part {
    const SPLIT: [Self; 3] = [Self::DayTime, Self::Party, Self::Channel];
}

/// One change a request asks for: `<ord>` for a whole operation,
/// `<ord>.<part>` for a field group of a weekly-timing change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeId {
    pub ord: usize,
    pub part: Part,
}

impl ChangeId {
    pub fn key(self) -> String {
        match self.part {
            Part::Whole => self.ord.to_string(),
            part => format!("{}.{}", self.ord, part.as_str()),
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        let (ord, part) = match key.split_once('.') {
            None => (key, Part::Whole),
            Some((ord, name)) => (
                ord,
                Part::SPLIT.into_iter().find(|part| part.as_str() == name)?,
            ),
        };
        if ord.is_empty() || !ord.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            ord: ord.parse().ok()?,
            part,
        })
    }
}

/// The changes `ops` ask for, in order.
pub fn changes(ops: &[DraftOp]) -> Vec<ChangeId> {
    let mut out = Vec::new();
    for (ord, op) in ops.iter().enumerate() {
        match op {
            DraftOp::ApplyFixedEdit { edit, .. } => {
                let parts = [
                    (edit.weekday.is_some() || edit.time.is_some(), Part::DayTime),
                    (edit.participants.is_some(), Part::Party),
                    (edit.channel_id.is_some(), Part::Channel),
                ];
                out.extend(
                    parts
                        .into_iter()
                        .filter(|(asked, _)| *asked)
                        .map(|(_, part)| ChangeId { ord, part }),
                );
            }
            _ => out.push(ChangeId {
                ord,
                part: Part::Whole,
            }),
        }
    }
    out
}

/// Whether the change carries a weekly day and time an administrator may
/// edit (a weekly-timing change's day and time, or a new weekly run).
pub fn retimable(ops: &[DraftOp], id: ChangeId) -> bool {
    matches!(
        (ops.get(id.ord), id.part),
        (Some(DraftOp::ApplyFixedEdit { .. }), Part::DayTime)
            | (Some(DraftOp::AddFixedRun(_)), Part::Whole)
    )
}

/// The weekly day and time a retimable change asks for.
pub fn asked_time(ops: &[DraftOp], id: ChangeId) -> Option<(Option<Weekday>, Option<NaiveTime>)> {
    match (ops.get(id.ord)?, id.part) {
        (DraftOp::ApplyFixedEdit { edit, .. }, Part::DayTime) => Some((edit.weekday, edit.time)),
        (DraftOp::AddFixedRun(new), Part::Whole) => Some((Some(new.weekday), Some(new.time))),
        _ => None,
    }
}

/// A change in the words its member reads ("Not approved: …").
pub fn describe(ops: &[DraftOp], id: ChangeId) -> String {
    match (id.part, ops.get(id.ord)) {
        (Part::DayTime, _) => "the day and time change",
        (Part::Party, _) => "the party change",
        (Part::Channel, _) => "the channel change",
        (Part::Whole, Some(DraftOp::AddFixedRun(_))) => "the new weekly run",
        (
            Part::Whole,
            Some(DraftOp::SwapParticipants { .. } | DraftOp::FixedParticipants { .. }),
        ) => "the party change",
        (Part::Whole, _) => "the change",
    }
    .to_owned()
}

/// An approval's edit: new weekly day and time per change, and the changes
/// left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestEdit {
    pub retime: BTreeMap<ChangeId, (Weekday, NaiveTime)>,
    pub drop: BTreeSet<ChangeId>,
}

const EDITED: &str = "edited=";
const DROPPED: &str = "dropped=";

fn weekday(index: usize) -> Option<Weekday> {
    const DAYS: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];
    DAYS.get(index).copied()
}

impl RequestEdit {
    pub fn is_empty(&self) -> bool {
        self.retime.is_empty() && self.drop.is_empty()
    }

    /// The operations to merge: `ops` with the new days and times and
    /// without the changes left out (a weekly-timing change loses only
    /// those fields; an operation with nothing left goes).
    ///
    /// # Errors
    /// [`RequestRefusal::UnknownChange`] for a change `ops` do not ask for,
    /// [`RequestRefusal::NotRetimable`] for a new time on a change without
    /// one (or one also left out), [`RequestRefusal::NothingApproved`] when
    /// every change is left out.
    pub fn apply(&self, ops: &[DraftOp]) -> Result<Vec<DraftOp>, RequestRefusal> {
        let asked: BTreeSet<ChangeId> = changes(ops).into_iter().collect();
        if self
            .retime
            .keys()
            .chain(&self.drop)
            .any(|id| !asked.contains(id))
        {
            return Err(RequestRefusal::UnknownChange);
        }
        if self
            .retime
            .keys()
            .any(|id| !retimable(ops, *id) || self.drop.contains(id))
        {
            return Err(RequestRefusal::NotRetimable);
        }
        if asked.iter().all(|id| self.drop.contains(id)) {
            return Err(RequestRefusal::NothingApproved);
        }
        let mut out = Vec::with_capacity(ops.len());
        for (ord, op) in ops.iter().enumerate() {
            let dropped = |part| self.drop.contains(&ChangeId { ord, part });
            let retimed = |part| self.retime.get(&ChangeId { ord, part }).copied();
            let mut op = op.clone();
            match &mut op {
                DraftOp::ApplyFixedEdit { edit, .. } => {
                    if dropped(Part::DayTime) {
                        edit.weekday = None;
                        edit.time = None;
                    }
                    if dropped(Part::Party) {
                        edit.participants = None;
                    }
                    if dropped(Part::Channel) {
                        edit.channel_id = None;
                    }
                    if let Some((day, time)) = retimed(Part::DayTime) {
                        edit.weekday = Some(day);
                        edit.time = Some(time);
                    }
                    if *edit == Default::default() {
                        continue;
                    }
                }
                _ if dropped(Part::Whole) => continue,
                DraftOp::AddFixedRun(new) => {
                    if let Some((day, time)) = retimed(Part::Whole) {
                        new.weekday = day;
                        new.time = time;
                    }
                }
                _ => {}
            }
            out.push(op);
        }
        Ok(out)
    }

    /// The changes left out, in the member's words, in request order.
    pub fn not_approved(&self, ops: &[DraftOp]) -> Vec<String> {
        self.drop.iter().map(|id| describe(ops, *id)).collect()
    }

    /// How the approval's `merged` event records the edit:
    /// `edited=<change>@<weekday 0-6>/<HH:MM>,…` (possibly empty) and, when
    /// something was left out, `dropped=<change>,…`. Space-free tokens, as
    /// the other merged-note tokens.
    pub fn note(&self) -> String {
        let retimed: Vec<String> = self
            .retime
            .iter()
            .map(|(id, (day, time))| {
                format!(
                    "{}@{}/{:02}:{:02}",
                    id.key(),
                    day.num_days_from_monday(),
                    time.hour(),
                    time.minute()
                )
            })
            .collect();
        let mut note = format!("{EDITED}{}", retimed.join(","));
        if !self.drop.is_empty() {
            let dropped: Vec<String> = self.drop.iter().map(|id| id.key()).collect();
            note.push_str(&format!(" {DROPPED}{}", dropped.join(",")));
        }
        note
    }

    /// The edit a `merged` event note records, if the approval was edited.
    pub fn from_note(note: &str) -> Option<Self> {
        let mut edit = None;
        let mut drop = BTreeSet::new();
        for token in note.split_whitespace() {
            if let Some(list) = token.strip_prefix(EDITED) {
                let mut retime = BTreeMap::new();
                for item in list.split(',').filter(|item| !item.is_empty()) {
                    let (key, at) = item.split_once('@')?;
                    let (day, time) = at.split_once('/')?;
                    let day = weekday(day.parse().ok()?)?;
                    let time = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
                    retime.insert(ChangeId::parse(key)?, (day, time));
                }
                edit = Some(retime);
            } else if let Some(list) = token.strip_prefix(DROPPED) {
                for key in list.split(',').filter(|key| !key.is_empty()) {
                    drop.insert(ChangeId::parse(key)?);
                }
            }
        }
        edit.map(|retime| Self { retime, drop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::drafts::Target;
    use crate::domain::schedule::{FixedEdit, FixedEditChoices};

    fn at(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    fn change_fixed() -> Vec<DraftOp> {
        vec![DraftOp::ApplyFixedEdit {
            fixed: Target::Existing("f-1".into()),
            edit: FixedEdit {
                weekday: Some(Weekday::Sat),
                time: Some(at(22, 30)),
                participants: Some(vec!["1".into(), "2".into()]),
                ..FixedEdit::default()
            },
            choices: FixedEditChoices::UpdateAll,
        }]
    }

    #[test]
    fn a_timing_change_splits_into_its_field_groups() {
        let ops = change_fixed();
        let keys: Vec<String> = changes(&ops).into_iter().map(ChangeId::key).collect();
        assert_eq!(keys, ["0.day_time", "0.party"]);
        for key in keys {
            assert_eq!(ChangeId::parse(&key).map(ChangeId::key), Some(key));
        }
        assert_eq!(ChangeId::parse("0.bosses"), None);
        assert_eq!(ChangeId::parse("x"), None);
    }

    #[test]
    fn retiming_and_leaving_out_edit_only_those_fields() {
        let ops = change_fixed();
        let day_time = ChangeId::parse("0.day_time").unwrap();
        let party = ChangeId::parse("0.party").unwrap();
        let edit = RequestEdit {
            retime: BTreeMap::from([(day_time, (Weekday::Sat, at(22, 0)))]),
            drop: BTreeSet::from([party]),
        };
        let applied = edit.apply(&ops).unwrap();
        let [DraftOp::ApplyFixedEdit { edit: applied, .. }] = applied.as_slice() else {
            panic!("one edit");
        };
        assert_eq!(
            (applied.weekday, applied.time, applied.participants.clone()),
            (Some(Weekday::Sat), Some(at(22, 0)), None)
        );
        assert_eq!(edit.not_approved(&ops), ["the party change"]);
        assert_eq!(RequestEdit::from_note(&edit.note()), Some(edit.clone()));
        assert_eq!(
            RequestEdit::from_note(&format!("choices=update_all {}", edit.note())),
            Some(edit)
        );
        assert_eq!(RequestEdit::from_note("choices=update_all"), None);
    }

    #[test]
    fn unknown_unretimable_and_empty_edits_are_refused() {
        let ops = change_fixed();
        let party = ChangeId::parse("0.party").unwrap();
        let day_time = ChangeId::parse("0.day_time").unwrap();
        let unknown = RequestEdit {
            drop: BTreeSet::from([ChangeId::parse("0.channel").unwrap()]),
            ..RequestEdit::default()
        };
        assert_eq!(unknown.apply(&ops), Err(RequestRefusal::UnknownChange));
        let party_time = RequestEdit {
            retime: BTreeMap::from([(party, (Weekday::Mon, at(1, 0)))]),
            ..RequestEdit::default()
        };
        assert_eq!(party_time.apply(&ops), Err(RequestRefusal::NotRetimable));
        let everything = RequestEdit {
            drop: BTreeSet::from([party, day_time]),
            ..RequestEdit::default()
        };
        assert_eq!(everything.apply(&ops), Err(RequestRefusal::NothingApproved));
    }
}
