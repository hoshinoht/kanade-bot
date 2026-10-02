//! `members.json`: the roster rows and reply-style personas.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    api::state::{GuildAccess, PersonaOption},
    domain::{members::MemberProfile, schedule::ScheduleSnapshot},
};

#[derive(Serialize)]
pub struct MemberRow {
    pub id: String,
    pub name: String,
    pub nickname: Option<String>,
    pub aliases: Vec<String>,
    pub runs_this_week: usize,
    pub ping_level: &'static str,
    pub persona: Option<String>,
    pub persona_available: bool,
    pub bossing: bool,
    pub access: &'static str,
}

#[derive(Serialize)]
pub struct Persona {
    pub key: String,
    pub name: String,
}

pub fn personas(options: &[PersonaOption]) -> Vec<Persona> {
    options
        .iter()
        .map(|option| Persona {
            key: option.key.clone(),
            name: option.name.clone(),
        })
        .collect()
}

/// The bossing roster plus anyone with chatbot access (v4 /members); bots never.
pub fn rows(
    profiles: &[MemberProfile],
    access: &GuildAccess,
    personas: &[PersonaOption],
    snapshot: &ScheduleSnapshot,
    this_week: DateTime<Utc>,
) -> Vec<MemberRow> {
    let mut rows: Vec<MemberRow> = profiles
        .iter()
        .filter(|profile| listed(profile, access))
        .map(|profile| row(profile, access, personas, snapshot, this_week))
        .collect();
    rows.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
    rows
}

/// The listed rows with `bossing` set: the Members page heading's count.
pub fn bossers(profiles: &[MemberProfile], access: &GuildAccess) -> usize {
    profiles
        .iter()
        .filter(|profile| listed(profile, access) && bossing(profile))
        .count()
}

fn listed(profile: &MemberProfile, access: &GuildAccess) -> bool {
    !profile.member.is_bot && (bossing(profile) || access.access(profile) != "none")
}

fn bossing(profile: &MemberProfile) -> bool {
    profile.member.has_role
}

/// One member's row (also for edits of members outside the listed roster).
pub fn row(
    profile: &MemberProfile,
    access: &GuildAccess,
    personas: &[PersonaOption],
    snapshot: &ScheduleSnapshot,
    this_week: DateTime<Utc>,
) -> MemberRow {
    let member = &profile.member;
    MemberRow {
        id: member.user_id.clone(),
        name: member
            .name()
            .map_or_else(|| member.user_id.clone(), str::to_owned),
        nickname: member.nickname.clone(),
        aliases: profile.aliases.clone(),
        runs_this_week: snapshot
            .runs
            .iter()
            .filter(|run| run.week_start == this_week && run.participants.contains(&member.user_id))
            .count(),
        ping_level: member.ping_level.as_str(),
        persona: profile.reply_style.clone(),
        persona_available: profile
            .reply_style
            .as_ref()
            .is_none_or(|key| personas.iter().any(|option| option.key == *key)),
        bossing: bossing(profile),
        access: access.access(profile),
    }
}
