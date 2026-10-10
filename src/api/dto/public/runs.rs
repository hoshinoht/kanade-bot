//! The member's own-run write results and the run deep-link view.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::MemberRun;
use crate::{
    api::{
        admin::write::Previous,
        dto::{hhmm, week::WeekFrame},
    },
    domain::schedule::FixedRun,
};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRunResult {
    pub run: MemberRun,
    pub version: u64,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberMoveResult {
    pub run: MemberRun,
    pub previous: Previous,
    pub version: u64,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRemoved {
    pub by: String,
    pub at: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberTimingSlot {
    pub day: u8,
    pub time: String,
}

impl MemberTimingSlot {
    /// The weekly slot (0 = Monday), as `MemberTiming` and request bodies.
    pub fn of(fixed: &FixedRun) -> Self {
        Self {
            day: fixed.weekday.num_days_from_monday() as u8,
            time: hhmm(fixed.time),
        }
    }
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRunLink {
    pub run: MemberRun,
    #[cfg_attr(test, ts(type = "'current' | 'next' | 'past' | 'later'"))]
    pub week: &'static str,
    pub week_starts: String,
    pub week_ends_at: String,
    pub started: bool,
    pub removed: Option<MemberRemoved>,
    pub this_week: Option<MemberRun>,
    pub timing: Option<MemberTimingSlot>,
    pub generated_at: String,
}

/// A run's boss week relative to now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunWeek {
    Current,
    Next,
    Past,
    Later,
}

impl RunWeek {
    /// `frames` are this and next boss week.
    pub fn of(frames: &[WeekFrame; 2], week_start: DateTime<Utc>) -> Self {
        if week_start == frames[0].start {
            Self::Current
        } else if week_start == frames[1].start {
            Self::Next
        } else if week_start < frames[0].start {
            Self::Past
        } else {
            Self::Later
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Next => "next",
            Self::Past => "past",
            Self::Later => "later",
        }
    }

    /// This or next boss week: the weeks members act in.
    pub fn open(self) -> bool {
        matches!(self, Self::Current | Self::Next)
    }
}
