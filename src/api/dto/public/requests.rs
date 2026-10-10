//! The member's own requests (`/api/public/requests`): what they asked, how it
//! stands, the limits and the form's choices. A request is a draft of kind
//! `request`; nothing here names another member's request.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{MemberRun, RunWeek, actor_label, member_run_in};
use crate::{
    api::dto::{
        Boss, Named, hhmm, iso_instant,
        week::{Context, WeekFrame},
    },
    domain::{
        completion::RunEnds,
        drafts::{DraftOp, DraftStatus, LoadedDraft, StoredDraft},
        requests::{RequestType, Subject, public_summary},
        schedule::{SchedulePolicy, ScheduleSnapshot, utc_instant},
    },
};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberProposed {
    pub day: Option<u8>,
    pub time: Option<String>,
    pub channel: Option<String>,
    pub party: Option<Vec<Named>>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRequest {
    pub id: String,
    #[cfg_attr(
        test,
        ts(type = "'join' | 'leave' | 'swap' | 'new_fixed' | 'change_fixed'")
    )]
    pub kind: &'static str,
    #[cfg_attr(
        test,
        ts(type = "'waiting' | 'approved' | 'rejected' | 'withdrawn' | 'expired'")
    )]
    pub state: &'static str,
    pub summary: String,
    pub note: Option<String>,
    pub run: Option<MemberRun>,
    pub fixed_id: Option<String>,
    pub bosses: Vec<Boss>,
    pub channel: Option<String>,
    pub with: Option<Named>,
    pub proposed: Option<MemberProposed>,
    pub sent_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRequestOptions {
    pub channels: Vec<Named>,
    pub members: Vec<Named>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRequests {
    pub requests: Vec<MemberRequest>,
    pub open: u32,
    pub today: u32,
    pub max_open: u32,
    pub max_today: u32,
    pub options: MemberRequestOptions,
    pub generated_at: String,
}

// `429 request_limit`: the refusal body plus which limit refused.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRequestLimit {
    #[cfg_attr(test, ts(type = "'request_limit'"))]
    pub error: &'static str,
    pub message: String,
    #[cfg_attr(test, ts(type = "'open' | 'today'"))]
    pub limit: &'static str,
}

/// What projecting the member's requests reads besides the rows.
pub struct RequestView<'a> {
    pub ctx: &'a Context<'a>,
    /// This and next boss week.
    pub frames: &'a [WeekFrame; 2],
    pub policy: &'a SchedulePolicy,
    pub ends: &'a RunEnds,
    pub user_id: &'a str,
}

impl RequestView<'_> {
    /// Its boss week has passed: the expiry tick closes it, or already did.
    fn expired(&self, draft: &StoredDraft) -> bool {
        draft
            .scope
            .expires_week()
            .is_some_and(|week| week < self.frames[0].start)
    }

    /// Still awaiting a decision (the member may withdraw it).
    pub fn waiting(&self, draft: &StoredDraft) -> bool {
        draft.status == DraftStatus::Submitted && !self.expired(draft)
    }

    fn state(&self, draft: &StoredDraft) -> &'static str {
        match draft.status {
            DraftStatus::Open | DraftStatus::Submitted if self.expired(draft) => "expired",
            DraftStatus::Open | DraftStatus::Submitted => "waiting",
            DraftStatus::Merged => "approved",
            DraftStatus::Rejected => "rejected",
            DraftStatus::Withdrawn | DraftStatus::Discarded => "withdrawn",
            DraftStatus::Expired => "expired",
        }
    }

    /// The reset that closes the request's boss week.
    fn deadline(&self, week: DateTime<Utc>) -> Option<String> {
        let weeks = self.policy.materialised_weeks(week).ok()?;
        utc_instant(&weeks[1]).ok().map(iso_instant)
    }

    fn channel(&self, id: Option<&String>) -> Option<String> {
        id.map(|id| self.ctx.channel_name(id))
    }

    fn party(&self, ids: &[String]) -> Vec<Named> {
        ids.iter().map(|id| self.ctx.named(id)).collect()
    }

    /// `snapshot` holds the subject run, if it still exists, and every
    /// weekly timing.
    pub fn request(&self, loaded: &LoadedDraft, snapshot: &ScheduleSnapshot) -> MemberRequest {
        let draft = &loaded.draft;
        let ctx = self.ctx;
        let kind = draft.request_type.as_deref().and_then(RequestType::parse);
        let subject = draft.subject.as_deref().and_then(Subject::parse);
        let summary = kind.map_or_else(
            || draft.title.clone(),
            |kind| public_summary(kind, subject.as_ref()),
        );
        let mut run = None;
        let mut fixed_id = None;
        let mut bosses: &[String] = &[];
        let mut channel = None;
        match &subject {
            Some(Subject::Run(id)) => {
                if let Some(found) = snapshot.runs.iter().find(|run| &run.id == id) {
                    let week = RunWeek::of(self.frames, found.week_start);
                    run = Some(member_run_in(
                        ctx,
                        snapshot,
                        found,
                        week,
                        self.ends,
                        self.user_id,
                    ));
                    bosses = &found.bosses;
                    channel = self.channel(found.channel_id.as_ref());
                }
            }
            Some(Subject::Fixed(id)) => {
                fixed_id = Some(id.clone());
                if let Some(fixed) = snapshot.fixed_runs.iter().find(|row| &row.id == id) {
                    bosses = &fixed.bosses;
                    channel = self.channel(fixed.channel_id.as_ref());
                }
            }
            None => {}
        }
        let mut with = None;
        let mut proposed = None;
        for staged in &loaded.ops {
            match &staged.op {
                DraftOp::AddFixedRun(new) => {
                    bosses = &new.bosses;
                    channel = self.channel(new.channel_id.as_ref());
                    proposed = Some(MemberProposed {
                        day: Some(new.weekday.num_days_from_monday() as u8),
                        time: Some(hhmm(new.time)),
                        channel: channel.clone(),
                        party: Some(self.party(&new.participants)),
                    });
                }
                DraftOp::ApplyFixedEdit { edit, .. } => {
                    proposed = Some(MemberProposed {
                        day: edit.weekday.map(|day| day.num_days_from_monday() as u8),
                        time: edit.time.map(hhmm),
                        channel: self.channel(edit.channel_id.as_ref()),
                        party: edit.participants.as_deref().map(|ids| self.party(ids)),
                    });
                }
                DraftOp::SwapParticipants { add, .. } | DraftOp::FixedParticipants { add, .. }
                    if kind == Some(RequestType::Swap) =>
                {
                    with = add.first().map(|id| ctx.named(id));
                }
                _ => {}
            }
        }
        let closed = !draft.status.is_live();
        MemberRequest {
            id: draft.id.clone(),
            kind: kind.map_or("join", RequestType::as_str),
            state: self.state(draft),
            // An untitled request stores its summary as the title.
            note: (draft.title != summary).then(|| draft.title.clone()),
            summary,
            run,
            fixed_id,
            bosses: ctx.bosses(bosses),
            channel,
            with,
            proposed,
            sent_at: iso_instant(draft.created_at),
            decided_at: closed.then(|| iso_instant(draft.updated_at)),
            decided_by: draft
                .closed_by
                .as_ref()
                .filter(|_| closed)
                .map(|actor| actor_label(ctx, actor)),
            reason: draft.close_reason.clone(),
            expires_at: draft
                .scope
                .expires_week()
                .and_then(|week| self.deadline(week)),
        }
    }
}
