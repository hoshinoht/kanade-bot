//! Change notices a mutation asks to post in a run's home channel.
//!
//! Domain events only: nothing is sent here. `effect_kind`/`effect_context`
//! match v4's journal identity for the same notice; `notify::plan_notice`
//! resolves mentions and turns a notice into a delivery intent.

use chrono::{DateTime, NaiveTime, Timelike, Utc, Weekday};

use super::run::{FixedField, RunStatus};
use crate::domain::time::to_iso;

/// What changed, with the facts the notice names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeChange {
    RunStatus {
        run_id: String,
        from: RunStatus,
        to: RunStatus,
    },
    RunMoved {
        run_id: String,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    },
    /// This week's line-up changed; `participants` is the new line-up.
    RunSwapped {
        run_id: String,
        participants: Vec<String>,
        leaving: Vec<String>,
        joining: Vec<String>,
    },
    /// v5 only: an amended run was put back on its weekly timing.
    RunReset {
        run_id: String,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    },
    /// v5 only: an administrator reverted recorded changes; one summary
    /// notice per rollback, however many changes it undid.
    Rollback {
        /// Reverted record positions, newest first.
        reverted: Vec<u64>,
        /// This channel's runs whose creation was reverted, now cancelled.
        cancelled_runs: Vec<String>,
        /// This channel's affected runs.
        run_ids: Vec<String>,
        /// The checkpoint a restore went back to (`None` for a revert).
        checkpoint: Option<String>,
    },
    /// v5 only: an administrator merged a draft; one summary notice per
    /// affected channel, however many operations it staged.
    Merged {
        draft: String,
        version: u64,
        title: String,
        /// This channel's changed runs.
        run_ids: Vec<String>,
        /// This channel's changed weekly timings.
        fixed_ids: Vec<String>,
    },
    /// v5 only: a member request was approved, rejected or expired. Lists
    /// only the requester, so it mentions nobody else.
    RequestDecided {
        request: String,
        decision: RequestDecision,
        /// A rejection's reason.
        reason: Option<String>,
    },
    FixedChanged {
        fixed_id: String,
        /// Touched fields, sorted by v4 column name.
        fields: Vec<FixedField>,
        weekday: Weekday,
        time: NaiveTime,
        participants: Vec<String>,
    },
}

/// What became of a member request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequestDecision {
    Approved,
    Rejected,
    Expired,
}

impl RequestDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

/// A change notice a mutation asks for: what happened, where, and who it lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub change: NoticeChange,
    /// The home channel; `None` means there is nowhere to post it.
    pub channel_id: Option<String>,
    /// Everyone the notice names, in order; the mention policy picks from these.
    pub listed: Vec<String>,
    /// Made outside the channel (portal/API/CLI) rather than by a command there.
    pub via_portal: bool,
}

impl NoticeChange {
    /// The run the notice is about; `None` for a weekly-timing change.
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::RunStatus { run_id, .. }
            | Self::RunMoved { run_id, .. }
            | Self::RunSwapped { run_id, .. }
            | Self::RunReset { run_id, .. } => Some(run_id),
            Self::FixedChanged { .. }
            | Self::Rollback { .. }
            | Self::Merged { .. }
            | Self::RequestDecided { .. } => None,
        }
    }
}

impl Notice {
    /// v4's mention-policy kind: every notice here is informational.
    pub fn ping_kind(&self) -> &'static str {
        match self.change {
            NoticeChange::RunStatus { .. }
            | NoticeChange::Rollback { .. }
            | NoticeChange::Merged { .. }
            | NoticeChange::RequestDecided { .. } => "status",
            NoticeChange::RunMoved { .. } | NoticeChange::RunReset { .. } => "amend",
            NoticeChange::RunSwapped { .. } => "swap",
            NoticeChange::FixedChanged { .. } => "fixed",
        }
    }

    /// v4's `effect_kind`, e.g. `notice.run.status.cancelled`.
    pub fn effect_kind(&self) -> String {
        match &self.change {
            NoticeChange::RunStatus { to, .. } => format!("notice.run.status.{}", to.as_str()),
            NoticeChange::RunMoved { .. } => "notice.run.move.moved".into(),
            NoticeChange::RunSwapped { .. } => "notice.run.swap.updated".into(),
            NoticeChange::RunReset { .. } => "notice.run.reset.restored".into(),
            NoticeChange::FixedChanged { .. } => "notice.fixed.edit.changed".into(),
            NoticeChange::Rollback {
                checkpoint: None, ..
            } => "notice.rollback.reverted".into(),
            NoticeChange::Rollback {
                checkpoint: Some(_),
                ..
            } => ROLLBACK_RESTORED.into(),
            NoticeChange::Merged { .. } => DRAFT_MERGED.into(),
            NoticeChange::RequestDecided { decision, .. } => {
                format!("notice.request.{}", decision.as_str())
            }
        }
    }

    /// v4's `effect_context`: the facts that make one notice distinct.
    pub fn effect_context(&self) -> Vec<String> {
        match &self.change {
            NoticeChange::RunStatus { run_id, from, to } => vec![
                run_id.clone(),
                from.as_str().into(),
                to.as_str().into(),
                if self.via_portal { "portal" } else { "command" }.into(),
            ],
            NoticeChange::RunMoved { run_id, from, to }
            | NoticeChange::RunReset { run_id, from, to } => {
                vec![run_id.clone(), iso(*from), iso(*to)]
            }
            NoticeChange::RunSwapped {
                run_id,
                participants,
                ..
            } => std::iter::once(run_id.clone())
                .chain(participants.iter().cloned())
                .collect(),
            NoticeChange::Rollback {
                reverted,
                cancelled_runs,
                run_ids,
                checkpoint,
            } => {
                let seqs: Vec<String> = reverted.iter().map(u64::to_string).collect();
                vec![
                    format!("reverted:{}", reverted.len()),
                    seqs.join(","),
                    run_ids.join(","),
                    cancelled_runs.join(","),
                    checkpoint.clone().unwrap_or_default(),
                ]
            }
            NoticeChange::Merged {
                draft,
                version,
                run_ids,
                fixed_ids,
                ..
            } => vec![
                draft.clone(),
                format!("v{version}"),
                run_ids.join(","),
                fixed_ids.join(","),
            ],
            // One notice per request and decision (delivery dedupe key).
            NoticeChange::RequestDecided {
                request, decision, ..
            } => vec![format!("request:{request}:{}", decision.as_str())],
            NoticeChange::FixedChanged {
                fixed_id,
                fields,
                weekday,
                time,
                participants,
            } => {
                let names: Vec<&str> = fields.iter().map(|field| field.as_str()).collect();
                [
                    fixed_id.clone(),
                    names.join(","),
                    weekday.num_days_from_monday().to_string(),
                    format!("{:02}:{:02}", time.hour(), time.minute()),
                ]
                .into_iter()
                .chain(participants.iter().cloned())
                .collect()
            }
        }
    }
}

/// The effect kind of a rollback that restored a checkpoint; such a
/// record's last `ref` is the checkpoint's head, not an undone change.
pub const ROLLBACK_RESTORED: &str = "notice.rollback.restored";

/// The effect kind of a draft merge's summary notice.
pub const DRAFT_MERGED: &str = "notice.draft.merged";

/// A mutation's result plus the notices it asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome<T> {
    pub value: T,
    pub notices: Vec<Notice>,
}

impl<T> Outcome<T> {
    pub fn quiet(value: T) -> Self {
        Self {
            value,
            notices: Vec::new(),
        }
    }
}

fn iso(at: DateTime<Utc>) -> String {
    // A stored UTC instant is always within v4's representable years.
    to_iso(&at).unwrap_or_default()
}
