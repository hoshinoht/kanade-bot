//! JSON shapes, mirroring web/packages/api-types (hand-written until generated).

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
pub struct Boss {
    pub token: String,
    pub key: String,
    pub name: String,
    pub difficulty: &'static str,
    pub level: Option<u16>,
    pub portrait: Option<String>,
    pub portrait_sm: Option<String>,
    pub art: Option<String>,
    pub hue: u16,
}

#[derive(Clone, Serialize)]
pub struct Participant {
    /// Stable member id; display names may repeat (two different "Ren"s).
    pub id: &'static str,
    pub name: &'static str,
    pub answer: &'static str,
}

#[derive(Clone, Copy, Serialize)]
pub struct Tally {
    pub on: usize,
    pub total: usize,
}

#[derive(Clone, Serialize)]
pub struct Card {
    pub label: &'static str,
    pub state: &'static str,
    pub at: String,
    pub url: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Named {
    pub id: String,
    pub name: String,
}

/// This week's roster against the weekly timing (v4 "this week: −A +B").
#[derive(Clone, Serialize)]
pub struct RosterChange {
    pub out: Vec<Named>,
    #[serde(rename = "in")]
    pub added: Vec<Named>,
}

#[derive(Clone, Serialize)]
pub struct Run {
    pub id: String,
    pub short_id: String,
    pub day: u8,
    pub time: Option<String>,
    pub minutes: u32,
    pub status: &'static str,
    pub bosses: Vec<Boss>,
    pub tally: Tally,
    pub participants: Vec<Participant>,
    pub party: &'static str,
    /// The channel id a re-read targets; `channel` is its display name.
    pub channel_id: &'static str,
    pub channel: &'static str,
    pub cards: Vec<Card>,
    /// The weekly timing this run was materialised from, if any.
    pub fixed_id: Option<String>,
    /// Day, time or roster differs from the weekly timing.
    pub amended: bool,
    pub roster_change: Option<RosterChange>,
}

/// Public projection: schedule facts and an aggregate tally, no people,
/// party handle or concurrency version (see web/packages/api-types).
#[derive(Serialize)]
pub struct PublicRun {
    pub id: String,
    pub day: u8,
    pub time: Option<String>,
    pub status: &'static str,
    pub bosses: Vec<Boss>,
    pub tally: Tally,
}

#[derive(Clone, Serialize)]
pub struct WeekDay {
    pub index: u8,
    pub date: String,
    pub dow: &'static str,
    pub is_reset: bool,
    pub is_today: bool,
}

#[derive(Serialize)]
pub struct Week {
    pub starts: String,
    pub timezone: &'static str,
    pub reset: &'static str,
    pub days: Vec<WeekDay>,
    pub runs: Vec<Run>,
    pub generated_at: String,
    pub version: u64,
}

#[derive(Serialize)]
pub struct PublicWeek {
    pub starts: String,
    pub timezone: &'static str,
    pub reset: &'static str,
    pub days: Vec<WeekDay>,
    pub runs: Vec<PublicRun>,
    pub generated_at: String,
}

#[derive(Serialize)]
pub struct DayStat {
    pub day: u8,
    pub answered: usize,
    pub waiting: usize,
}

#[derive(Serialize)]
pub struct Stats {
    pub per_day: Vec<DayStat>,
}

#[derive(Serialize)]
pub struct NextRun {
    pub run_id: String,
    pub bosses: String,
    pub when: String,
    pub countdown: String,
    pub on: usize,
    pub total: usize,
}

#[derive(Serialize)]
pub struct Model {
    pub busy: bool,
    pub holder: Option<&'static str>,
}

#[derive(Serialize)]
pub struct Summary {
    pub next: Option<NextRun>,
    pub unanswered: usize,
    pub inbox: usize,
    pub members: usize,
    pub reminders: usize,
    pub model: Model,
    /// Config → Notifications quiet mode, as the last config PATCH left it.
    pub quiet_mode: bool,
}

/// The signed-out sign-in strip: time, boss names and the aggregate tally only.
#[derive(Serialize)]
pub struct TonightRun {
    pub time: String,
    pub bosses: Vec<String>,
    pub tally: Tally,
}

#[derive(Serialize)]
pub struct Tonight {
    pub run: Option<TonightRun>,
}

#[derive(Deserialize)]
pub struct MoveRequest {
    pub day: u8,
    pub time: Option<String>,
    pub version: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwapRequest {
    pub with: String,
    pub version: u64,
}

#[derive(Deserialize)]
pub struct StatusRequest {
    pub status: String,
    pub version: u64,
}

#[derive(Deserialize)]
pub struct RsvpRequest {
    pub member_id: String,
    pub answer: String,
    pub version: u64,
}

#[derive(Deserialize)]
pub struct ParticipantsRequest {
    pub add: Option<String>,
    pub remove: Option<String>,
    pub version: u64,
}

#[derive(Serialize)]
pub struct Previous {
    pub day: u8,
    pub time: Option<String>,
}

#[derive(Serialize)]
pub struct MoveResult {
    pub run: Run,
    pub previous: Previous,
    pub version: u64,
}

#[derive(Serialize)]
pub struct SwapResult {
    pub runs: [Run; 2],
    pub version: u64,
}

#[derive(Serialize)]
pub struct RunResult {
    pub run: Run,
    pub version: u64,
}

#[derive(Serialize)]
pub struct FixedRunLink {
    pub run_id: String,
    pub short_id: String,
    pub week: &'static str,
    pub day: u8,
    pub time: Option<String>,
    pub status: &'static str,
    pub amended: bool,
}

#[derive(Serialize)]
pub struct FixedRow {
    pub id: String,
    pub short_id: String,
    pub weekday: u8,
    pub weekday_name: &'static str,
    pub time: String,
    pub bosses: Vec<Boss>,
    pub participants: Vec<Named>,
    pub channel_id: &'static str,
    pub channel_name: &'static str,
    pub channel_watched: bool,
    pub owner: &'static str,
    pub owner_id: &'static str,
    pub note: Option<String>,
    /// Materialised, still-live runs this week and next.
    pub runs: Vec<FixedRunLink>,
}

#[derive(Deserialize)]
pub struct FixedRequest {
    pub weekday: u8,
    pub time: String,
    pub bosses: String,
    pub participants: Vec<String>,
    pub channel_id: String,
    pub note: Option<String>,
    /// Omitted: the first participant on create (a token sign-in names no
    /// Discord user), unchanged on edit.
    #[serde(default)]
    pub owner_id: Option<String>,
    /// Per amended run: `update` (follow the new timing) or `keep` (this week only).
    #[serde(default)]
    pub decisions: std::collections::HashMap<String, String>,
    /// Week version the form was loaded at: required by PATCH, ignored by POST.
    #[serde(default)]
    pub version: Option<u64>,
}

#[derive(Deserialize)]
pub struct ValidateRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct ValidateResult {
    pub bosses: Vec<Boss>,
}

#[derive(Serialize)]
pub struct MemberRow {
    pub id: &'static str,
    pub name: &'static str,
    pub nickname: Option<&'static str>,
    pub aliases: Vec<String>,
    pub runs_this_week: usize,
    pub ping_level: &'static str,
    pub persona: Option<&'static str>,
    pub persona_available: bool,
    pub bossing: bool,
    pub access: &'static str,
}

#[derive(Serialize)]
pub struct Persona {
    pub key: &'static str,
    pub name: &'static str,
}

#[derive(Deserialize)]
pub struct MemberPatch {
    pub ping_level: Option<String>,
    /// Empty string clears back to the default reply style.
    pub persona: Option<String>,
}

#[derive(Deserialize)]
pub struct AliasRequest {
    pub alias: String,
}

#[derive(Serialize)]
pub struct ReminderRow {
    pub id: String,
    pub run_id: String,
    pub run_short_id: String,
    pub kind: &'static str,
    /// `queued` (due later), `due` (time passed, not posted yet), `sent`, `stale` (retired unposted).
    pub state: &'static str,
    /// Local `Tue 29 Sep 21:00`.
    pub at: String,
    pub bosses: Vec<Boss>,
    pub party: Vec<&'static str>,
    pub url: Option<String>,
}

#[derive(Serialize)]
pub struct Reminders {
    pub upcoming: Vec<ReminderRow>,
    pub sent: Vec<ReminderRow>,
}
