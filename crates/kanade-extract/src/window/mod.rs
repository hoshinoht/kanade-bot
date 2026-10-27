//! How much history a rescan reads and how it is cut into bursts (v4
//! `bot/extract/window.py`). Rows are generic; `key` gives each row's instant.

mod rescan;

use std::cmp::Reverse;

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use chrono_tz::Tz;

pub use rescan::{
    AUTOMATED_WINDOW, DEFAULT_WINDOW, WINDOWS, WindowError, clamp_window, previous_week_start,
    should_widen, window_since,
};

/// Silence that ends a burst when replaying history. Planning threads pause
/// for most of an hour; a short gap splits one conversation into many prompts.
pub const BURST_GAP: TimeDelta = TimeDelta::hours(3);

/// Past this a burst is split at its longest pause; the prompt token budget is
/// the real guarantee, this keeps most bursts well clear of it.
pub const MAX_BURST_MESSAGES: usize = 12;

/// Split chronological rows wherever the channel was quiet for more than `gap`.
pub fn group_bursts<T: Clone>(
    rows: &[T],
    gap: TimeDelta,
    key: impl Fn(&T) -> DateTime<Utc>,
) -> Vec<Vec<T>> {
    let mut groups: Vec<Vec<T>> = Vec::new();
    let mut current: Vec<T> = Vec::new();
    let mut previous: Option<DateTime<Utc>> = None;
    for row in rows {
        let when = key(row);
        if previous.is_some_and(|previous| when - previous > gap) && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
        }
        current.push(row.clone());
        previous = Some(when);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// Cut a window of history into the conversations it was.
///
/// The unit is the guild-local calendar day: a day that fits in `cap` goes as
/// one burst, pauses and all. A bigger day is split on `gap` silences, then at
/// its longest pause until every piece fits.
pub fn group_for_rescan<T: Clone>(
    rows: &[T],
    zone: Tz,
    gap: TimeDelta,
    cap: usize,
    key: impl Fn(&T) -> DateTime<Utc>,
) -> Vec<Vec<T>> {
    let mut out = Vec::new();
    for day_rows in by_local_day(rows, zone, &key) {
        if day_rows.len() <= cap {
            out.push(day_rows);
            continue;
        }
        for chunk in group_bursts(&day_rows, gap, &key) {
            out.extend(split_until(&chunk, |piece| piece.len() <= cap, &key));
        }
    }
    out
}

/// Halve at the longest pause until every piece satisfies `fits`; a single row
/// that still does not fit comes back on its own.
pub fn split_until<T: Clone>(
    rows: &[T],
    mut fits: impl FnMut(&[T]) -> bool,
    key: impl Fn(&T) -> DateTime<Utc>,
) -> Vec<Vec<T>> {
    let mut out = Vec::new();
    split_into(rows, &mut fits, &key, &mut out);
    out
}

fn split_into<T: Clone>(
    rows: &[T],
    fits: &mut impl FnMut(&[T]) -> bool,
    key: &impl Fn(&T) -> DateTime<Utc>,
    out: &mut Vec<Vec<T>>,
) {
    if rows.len() <= 1 || fits(rows) {
        out.push(rows.to_vec());
        return;
    }
    let (head, tail) = rows.split_at(longest_pause(rows, key));
    split_into(head, fits, key, out);
    split_into(tail, fits, key, out);
}

/// The index after the longest silence; ties go to the most central split,
/// then the earliest (Python `max` keeps the first maximum).
fn longest_pause<T>(rows: &[T], key: &impl Fn(&T) -> DateTime<Utc>) -> usize {
    let middle = rows.len() / 2;
    let score = |index: usize| {
        (
            key(&rows[index]) - key(&rows[index - 1]),
            Reverse(index.abs_diff(middle)),
        )
    };
    let mut best = 1;
    for index in 2..rows.len() {
        if score(index) > score(best) {
            best = index;
        }
    }
    best
}

/// Rows grouped by their date in `zone`, days in order, rows in input order.
fn by_local_day<T: Clone>(rows: &[T], zone: Tz, key: &impl Fn(&T) -> DateTime<Utc>) -> Vec<Vec<T>> {
    let mut days: Vec<(NaiveDate, Vec<T>)> = Vec::new();
    for row in rows {
        let day = key(row).with_timezone(&zone).date_naive();
        match days.iter_mut().find(|(known, _)| *known == day) {
            Some((_, group)) => group.push(row.clone()),
            None => days.push((day, vec![row.clone()])),
        }
    }
    days.sort_by_key(|(day, _)| *day);
    days.into_iter().map(|(_, group)| group).collect()
}
