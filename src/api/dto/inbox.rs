//! `inbox.json`: extractor/chat proposals and member requests as one list
//! (`docs/v5/admin-api.md` "Inbox (A6)").

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use serde::Serialize;

use super::{
    Boss, Named, dow, hhmm,
    week::{Context, WeekFrame},
    when,
};
use crate::domain::{
    drafts::{
        DraftOp, FieldValue, LoadedDraft, MergeAnalysis, MergeConflict, ProposalInfo, Removal,
        Target,
    },
    history::Actor,
    ids::short_id,
    proposals::{ProposalSubject, StoredCard},
    requests::{RequestType, Subject, public_summary},
    schedule::{Change, ChangeSet, SchedulePolicy, ScheduleSnapshot, utc_instant},
    scheduler::{ProposalPreview, RequestPreview},
};

#[derive(Clone, Debug, Serialize)]
pub struct Evidence {
    pub id: String,
    pub author: String,
    /// `None` when the message is gone.
    pub author_id: Option<String>,
    pub at: String,
    pub content: Option<String>,
    pub url: Option<String>,
    pub missing: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FieldChange {
    pub field: String,
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FieldConflict {
    pub field: String,
    pub expected: String,
    pub found: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Preview {
    pub no_effect: bool,
    pub changes: Vec<FieldChange>,
    pub conflicts: Vec<FieldConflict>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Choice {
    pub run_id: String,
    pub label: String,
    pub when: String,
    pub amended: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelfService {
    pub member: Named,
    pub via: &'static str,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProposalDto {
    pub id: String,
    pub short_id: String,
    pub kind: &'static str,
    pub kind_label: &'static str,
    pub source: &'static str,
    pub tab: &'static str,
    pub version: u64,
    pub flags: Vec<&'static str>,
    pub preview: Preview,
    pub expires_at: Option<String>,
    pub choices: Option<Vec<Choice>>,
    pub public_summary: Option<String>,
    pub bosses: Vec<Boss>,
    pub run_id: Option<String>,
    pub from_when: Option<String>,
    pub when: String,
    pub participants: Vec<Named>,
    pub confidence: Option<f64>,
    pub is_question: bool,
    pub channel: Option<String>,
    pub read_at: String,
    pub summary: String,
    pub evidence: Vec<Evidence>,
    pub card_url: Option<String>,
    pub self_service: Option<SelfService>,
    /// Sort key (oldest first).
    #[serde(skip)]
    pub created_at: DateTime<Utc>,
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "move" => "Move",
        "add" => "New run",
        "cancel" => "Cancel",
        "split" => "Split",
        "otot" => "Own time",
        "sub" | "swap" => "Swap",
        "rsvp" => "Answer",
        "fix" => "Weekly timing",
        "new_fixed" => "New weekly run",
        "change_fixed" => "Weekly timing change",
        "join" => "Join",
        "leave" => "Leave",
        _ => "Change",
    }
}

/// `Tue 22:00`, a weekly slot.
pub fn slot(weekday: Weekday, time: NaiveTime) -> String {
    format!("{} {}", dow(weekday), hhmm(time))
}

const NONE: &str = "—";

fn names(ctx: &Context<'_>, ids: &[String]) -> String {
    if ids.is_empty() {
        return NONE.into();
    }
    ids.iter()
        .map(|id| ctx.name(id))
        .collect::<Vec<_>>()
        .join(", ")
}

fn tokens(bosses: &[String]) -> String {
    if bosses.is_empty() {
        NONE.into()
    } else {
        bosses.join(" + ")
    }
}

fn opt(value: Option<&str>) -> String {
    value.map_or_else(|| NONE.into(), str::to_owned)
}

/// Field-level changes the merge result writes against `current`. Fields are
/// the domain's names; with more than one row touched each is prefixed by
/// the row's short id.
pub fn changes(ctx: &Context<'_>, current: &ScheduleSnapshot, set: &ChangeSet) -> Vec<FieldChange> {
    let mut rows: Vec<(String, &'static str, String, String)> = Vec::new();
    let mut push = |row: &str, field: &'static str, from: String, to: String| {
        if from != to {
            rows.push((row.to_owned(), field, from, to));
        }
    };
    let mut answers: Vec<(String, String, String, String)> = Vec::new();
    let channel = |id: Option<&String>| opt(id.map(|id| ctx.channel_name(id)).as_deref());
    for change in &set.changes {
        match change {
            Change::PutRun(run) => match current.runs.iter().find(|old| old.id == run.id) {
                None => push(
                    &run.id,
                    "new_run",
                    NONE.into(),
                    format!("{} {}", when(run.datetime, ctx.zone), tokens(&run.bosses)),
                ),
                Some(old) => {
                    push(
                        &run.id,
                        "slot",
                        when(old.datetime, ctx.zone),
                        when(run.datetime, ctx.zone),
                    );
                    push(&run.id, "bosses", tokens(&old.bosses), tokens(&run.bosses));
                    push(
                        &run.id,
                        "participants",
                        names(ctx, &old.participants),
                        names(ctx, &run.participants),
                    );
                    push(
                        &run.id,
                        "channel",
                        channel(old.channel_id.as_ref()),
                        channel(run.channel_id.as_ref()),
                    );
                    push(
                        &run.id,
                        "status",
                        old.status.as_str().into(),
                        run.status.as_str().into(),
                    );
                }
            },
            Change::PutFixedRun(row) => {
                match current.fixed_runs.iter().find(|old| old.id == row.id) {
                    None => push(
                        &row.id,
                        "new_fixed",
                        NONE.into(),
                        format!("{} {}", slot(row.weekday, row.time), tokens(&row.bosses)),
                    ),
                    Some(old) => {
                        push(
                            &row.id,
                            "day_time",
                            slot(old.weekday, old.time),
                            slot(row.weekday, row.time),
                        );
                        push(&row.id, "bosses", tokens(&old.bosses), tokens(&row.bosses));
                        push(
                            &row.id,
                            "participants",
                            names(ctx, &old.participants),
                            names(ctx, &row.participants),
                        );
                        push(
                            &row.id,
                            "channel",
                            channel(old.channel_id.as_ref()),
                            channel(row.channel_id.as_ref()),
                        );
                        push(
                            &row.id,
                            "note",
                            opt(old.note.as_deref()),
                            opt(row.note.as_deref()),
                        );
                        push(
                            &row.id,
                            "owner",
                            ctx.name(&old.owner_id),
                            ctx.name(&row.owner_id),
                        );
                    }
                }
            }
            Change::DeleteFixedRun(id) => {
                let from = current
                    .fixed_runs
                    .iter()
                    .find(|old| &old.id == id)
                    .map_or_else(|| NONE.into(), |old| slot(old.weekday, old.time));
                push(id, "retired", from, "retired".into());
            }
            Change::PutRsvp(rsvp) => {
                let from = current
                    .rsvps
                    .iter()
                    .find(|old| old.run_id == rsvp.run_id && old.user_id == rsvp.user_id)
                    .map_or(NONE, |old| old.state.as_str());
                answers.push((
                    rsvp.run_id.clone(),
                    rsvp.user_id.clone(),
                    from.into(),
                    rsvp.state.as_str().into(),
                ));
            }
            Change::DeleteRsvp { run_id, user_id } => {
                let from = current
                    .rsvps
                    .iter()
                    .find(|old| &old.run_id == run_id && &old.user_id == user_id)
                    .map_or(NONE, |old| old.state.as_str());
                answers.push((run_id.clone(), user_id.clone(), from.into(), NONE.into()));
            }
            Change::PutReminder(_) | Change::DeleteReminder(_) => {}
        }
    }
    let mut entities: BTreeSet<&str> = rows.iter().map(|(row, ..)| row.as_str()).collect();
    entities.extend(answers.iter().map(|(run, ..)| run.as_str()));
    let prefix = |row: &str, field: String| {
        if entities.len() > 1 {
            format!("#{} {field}", short_id(row))
        } else {
            field
        }
    };
    let mut out: Vec<FieldChange> = rows
        .iter()
        .map(|(row, field, from, to)| FieldChange {
            field: prefix(row, (*field).to_owned()),
            from: from.clone(),
            to: to.clone(),
        })
        .collect();
    out.extend(answers.iter().filter(|(_, _, from, to)| from != to).map(
        |(run, user, from, to)| FieldChange {
            field: prefix(run, format!("answer:{user}")),
            from: from.clone(),
            to: to.clone(),
        },
    ));
    out
}

fn value(value: Option<&FieldValue>) -> String {
    match value {
        None | Some(FieldValue::Text(None)) => NONE.into(),
        Some(FieldValue::Text(Some(text))) => text.clone(),
        Some(FieldValue::Set(items)) if items.is_empty() => NONE.into(),
        Some(FieldValue::Set(items)) => items.join(", "),
    }
}

fn removal(removal: Removal) -> &'static str {
    match removal {
        Removal::Deleted => "deleted",
        Removal::Retired => "retired",
        Removal::Cancelled => "cancelled",
        Removal::Done => "done",
        Removal::LeftRun => "left the run",
    }
}

/// Three-way conflicts: `expected` is what the change was based on,
/// `found` what the schedule holds now.
pub fn conflicts(analysis: &MergeAnalysis) -> Vec<FieldConflict> {
    analysis
        .conflicts
        .iter()
        .map(|conflict| match conflict {
            MergeConflict::BothChanged {
                entity,
                field,
                base,
                upstream,
                ..
            } => FieldConflict {
                field: format!("{entity} {}", field.as_str()),
                expected: value(base.as_ref()),
                found: value(upstream.as_ref()),
            },
            MergeConflict::UpstreamRemoved { entity, removal: r } => FieldConflict {
                field: entity.to_string(),
                expected: "present".into(),
                found: removal(*r).into(),
            },
            MergeConflict::OpRejected { error, .. } => refused(error.to_string()),
            MergeConflict::ChoicesStale { base, current, .. } => FieldConflict {
                field: "choices".into(),
                expected: base.join(", "),
                found: current.join(", "),
            },
            MergeConflict::StaleWeeks { .. } => FieldConflict {
                field: "weeks".into(),
                expected: "the weeks it was staged for".into(),
                found: "a later boss week".into(),
            },
            MergeConflict::Divergent { entity, field } => FieldConflict {
                field: format!("{entity} {}", field.as_str()),
                expected: "the merged value".into(),
                found: "something else".into(),
            },
        })
        .collect()
}

/// A change that no longer applies, as a conflict line.
pub fn refused(reason: String) -> FieldConflict {
    FieldConflict {
        field: "change".into(),
        expected: "applies".into(),
        found: reason,
    }
}

fn preview_of(
    ctx: &Context<'_>,
    current: &ScheduleSnapshot,
    analysis: &MergeAnalysis,
    no_effect: bool,
) -> Preview {
    Preview {
        no_effect,
        changes: analysis
            .result_changes
            .as_ref()
            .map(|set| changes(ctx, current, set))
            .unwrap_or_default(),
        conflicts: conflicts(analysis),
    }
}

/// What every item shares.
pub struct Common<'a> {
    pub ctx: &'a Context<'a>,
    pub policy: &'a SchedulePolicy,
    pub current: &'a ScheduleSnapshot,
    pub frames: &'a [WeekFrame; 2],
}

impl Common<'_> {
    fn deadline(&self, week: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // Expiry closes it at the reset that ends `week`.
        let weeks = self.policy.materialised_weeks(week).ok()?;
        utc_instant(&weeks[1]).ok()
    }

    fn run_when(&self, run_id: &str) -> Option<String> {
        self.current
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .map(|run| when(run.datetime, self.ctx.zone))
    }
}

fn timed(ops: &[DraftOp]) -> Option<DateTime<Utc>> {
    ops.iter().find_map(|op| match op {
        DraftOp::AmendRun { to, .. } => Some(*to),
        DraftOp::CreateRun { datetime, .. } => Some(*datetime),
        _ => None,
    })
}

fn weekly(ops: &[DraftOp], current: &ScheduleSnapshot) -> Option<(String, Option<String>)> {
    ops.iter().find_map(|op| match op {
        DraftOp::AddFixedRun(new) => Some((slot(new.weekday, new.time), None)),
        DraftOp::ApplyFixedEdit {
            fixed: Target::Existing(id),
            edit,
            ..
        } => {
            let row = current.fixed_runs.iter().find(|row| &row.id == id)?;
            Some((
                slot(
                    edit.weekday.unwrap_or(row.weekday),
                    edit.time.unwrap_or(row.time),
                ),
                Some(slot(row.weekday, row.time)),
            ))
        }
        DraftOp::RetireFixedRun {
            fixed: Target::Existing(id),
            ..
        } => {
            let row = current.fixed_runs.iter().find(|row| &row.id == id)?;
            Some(("retired".into(), Some(slot(row.weekday, row.time))))
        }
        _ => None,
    })
}

fn flags(preview: &Preview, expired: bool, frozen: bool, unauthorised: bool) -> Vec<&'static str> {
    [
        (!preview.conflicts.is_empty(), "conflict"),
        (expired, "expired"),
        (frozen, "requester_frozen"),
        (unauthorised, "requester_unauthorised"),
        (preview.no_effect, "no_effect"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect()
}

pub fn message_url(ctx: &Context<'_>, channel: &str, message: &str) -> Option<String> {
    Some(format!(
        "https://discord.com/channels/{}/{channel}/{message}",
        ctx.guild_id?
    ))
}

/// A proposal item. `preview` is `Err` with the reason it could not be
/// previewed (listed, blocked).
#[allow(clippy::too_many_arguments)]
pub fn proposal(
    common: &Common<'_>,
    loaded: &LoadedDraft,
    info: &ProposalInfo,
    subject: &ProposalSubject,
    card: Option<&StoredCard>,
    preview: Result<&ProposalPreview, String>,
    evidence: Vec<Evidence>,
) -> ProposalDto {
    let ctx = common.ctx;
    let draft = &loaded.draft;
    let ops = loaded.draft_ops();
    let (mut shown, expired) = match &preview {
        Ok(preview) => {
            let mut shown = preview_of(ctx, common.current, &preview.analysis, preview.no_effect);
            if let Some(refusal) = &preview.refusal {
                shown.conflicts.push(refused(refusal.to_string()));
            }
            (shown, preview.expired)
        }
        Err(reason) => (
            Preview {
                conflicts: vec![refused(reason.clone())],
                ..Preview::default()
            },
            info.expires_at <= ctx.now,
        ),
    };
    shown.conflicts.dedup();
    let details = card.map(|card| &card.details);
    let run = subject
        .run_id
        .as_deref()
        .and_then(|id| common.current.runs.iter().find(|run| run.id == id));
    let bosses = [
        details.map(|details| details.bosses.clone()),
        Some(subject.bosses.clone()),
        run.map(|run| run.bosses.clone()),
    ]
    .into_iter()
    .flatten()
    .find(|bosses| !bosses.is_empty())
    .unwrap_or_default();
    let participants = details
        .map(|details| details.participants.clone())
        .filter(|people| !people.is_empty())
        .unwrap_or_else(|| subject.named.clone());
    let weekly = weekly(&ops, common.current);
    let when = timed(&ops)
        .map(|at| super::when(at, ctx.zone))
        .or_else(|| weekly.as_ref().map(|(to, _)| to.clone()))
        .or_else(|| subject.run_id.as_deref().and_then(|id| common.run_when(id)))
        .or_else(|| details.and_then(|details| details.payload.weekly_when.clone()))
        .unwrap_or_else(|| NONE.into());
    let from_when = match subject.kind.as_str() {
        "move" => subject.run_id.as_deref().and_then(|id| common.run_when(id)),
        "fix" => weekly.and_then(|(_, from)| from),
        _ => None,
    };
    let channel = card
        .map(|card| card.channel_id.clone())
        .or_else(|| subject.channel_id.clone());
    let deadline = draft
        .scope
        .expires_week()
        .and_then(|week| common.deadline(week))
        .map_or(info.expires_at, |week| week.min(info.expires_at));
    ProposalDto {
        id: draft.id.clone(),
        short_id: short_id(&draft.id),
        kind: subject.kind.as_str(),
        kind_label: kind_label(subject.kind.as_str()),
        source: info.source.as_str(),
        tab: "extractor",
        version: draft.version,
        flags: flags(&shown, expired, false, false),
        preview: shown,
        expires_at: Some(super::when(deadline, ctx.zone)),
        choices: None,
        public_summary: None,
        bosses: ctx.bosses(&bosses),
        run_id: subject.run_id.clone(),
        from_when,
        when,
        participants: participants.iter().map(|id| ctx.named(id)).collect(),
        confidence: details.map(|details| details.confidence),
        is_question: details.is_some_and(|details| details.is_question),
        channel: channel.map(|id| ctx.channel_name(&id)),
        read_at: super::when(draft.created_at, ctx.zone),
        summary: details
            .and_then(|details| details.summary.clone())
            .unwrap_or_else(|| draft.title.clone()),
        evidence,
        card_url: card
            .and_then(|card| message_url(ctx, &card.channel_id, card.message_id.as_deref()?)),
        self_service: None,
        created_at: draft.created_at,
    }
}

/// A member request item; `choices` lists a `change_fixed`'s amended runs.
pub fn request(
    common: &Common<'_>,
    loaded: &LoadedDraft,
    preview: Result<&RequestPreview, String>,
    choices: Option<Vec<Choice>>,
) -> ProposalDto {
    let ctx = common.ctx;
    let current = common.current;
    let draft = &loaded.draft;
    let ops = loaded.draft_ops();
    let kind = draft
        .request_type
        .as_deref()
        .and_then(RequestType::parse)
        .map_or("change", RequestType::as_str);
    let subject = draft.subject.as_deref().and_then(Subject::parse);
    let requester = match &draft.author {
        Actor::Member { id } => id.clone(),
        other => other.id().to_owned(),
    };
    let (shown, frozen, unauthorised) = match &preview {
        Ok(preview) => (
            preview_of(ctx, current, &preview.analysis, preview.no_effect),
            preview.requester_frozen,
            !preview.requester_authorised,
        ),
        Err(reason) => (
            Preview {
                conflicts: vec![refused(reason.clone())],
                ..Preview::default()
            },
            false,
            false,
        ),
    };
    let expires_week = draft.scope.expires_week();
    let expired = expires_week.is_some_and(|week| week < common.frames[0].start);
    let mut bosses = Vec::new();
    let mut when = None;
    let mut from_when = None;
    let mut run_id = None;
    let mut channel = None;
    let mut participants = Vec::new();
    match &subject {
        Some(Subject::Run(id)) => {
            run_id = Some(id.clone());
            if let Some(run) = current.runs.iter().find(|run| &run.id == id) {
                bosses.clone_from(&run.bosses);
                when = Some(super::when(run.datetime, ctx.zone));
                channel.clone_from(&run.channel_id);
            }
        }
        Some(Subject::Fixed(id)) => {
            if let Some(row) = current.fixed_runs.iter().find(|row| &row.id == id) {
                bosses.clone_from(&row.bosses);
                when = Some(slot(row.weekday, row.time));
                channel.clone_from(&row.channel_id);
                participants.clone_from(&row.participants);
            }
        }
        None => {}
    }
    for op in &ops {
        match op {
            DraftOp::AddFixedRun(new) => {
                bosses.clone_from(&new.bosses);
                when = Some(slot(new.weekday, new.time));
                channel.clone_from(&new.channel_id);
                participants.clone_from(&new.participants);
            }
            DraftOp::ApplyFixedEdit { edit, .. } => {
                if let Some((to, from)) = weekly(std::slice::from_ref(op), current) {
                    from_when = from;
                    when = Some(to);
                }
                if let Some(party) = &edit.participants {
                    participants.clone_from(party);
                }
            }
            DraftOp::SwapParticipants { remove, add, .. }
            | DraftOp::FixedParticipants { remove, add, .. } => {
                participants = remove.iter().chain(add).cloned().collect();
            }
            _ => {}
        }
    }
    ProposalDto {
        id: draft.id.clone(),
        short_id: short_id(&draft.id),
        kind,
        kind_label: kind_label(kind),
        source: "self_service",
        tab: "self_service",
        version: draft.version,
        flags: flags(&shown, expired, frozen, unauthorised),
        preview: shown,
        expires_at: expires_week
            .and_then(|week| common.deadline(week))
            .map(|at| super::when(at, ctx.zone)),
        choices,
        public_summary: RequestType::parse(kind).map(|kind| public_summary(kind, subject.as_ref())),
        bosses: ctx.bosses(&bosses),
        run_id,
        from_when,
        when: when.unwrap_or_else(|| NONE.into()),
        participants: participants.iter().map(|id| ctx.named(id)).collect(),
        confidence: None,
        is_question: false,
        channel: channel.map(|id| ctx.channel_name(&id)),
        read_at: super::when(draft.created_at, ctx.zone),
        summary: draft.title.clone(),
        evidence: Vec::new(),
        card_url: None,
        self_service: Some(SelfService {
            member: ctx.named(&requester),
            via: "request",
            note: Some(draft.title.clone()),
        }),
        created_at: draft.created_at,
    }
}
