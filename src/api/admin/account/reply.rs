//! The Account's reply style: the profile chat answers this member in and the
//! member's own saved choice, resolved exactly as a chat turn resolves it
//! (`resolve_profile`). Saving the choice is the Members edit
//! (`PATCH /api/admin/members/{id}` with `persona`), which accepts only
//! public profiles.

use crate::{
    api::{
        dto::account::{ReplyStyle, ReplyStyleRef},
        state::ApiState,
    },
    chat::persona::{
        ProfileId, ProfileQuery, ProfileSet, ProfileSource, RoleAssignment, RoleId, resolve_profile,
    },
    domain::members::MemberProfile,
};

fn named(
    profiles: &ProfileSet,
    selectable: &std::collections::BTreeSet<ProfileId>,
    id: &ProfileId,
) -> ReplyStyleRef {
    ReplyStyleRef {
        key: id.to_string(),
        name: profiles
            .get(id)
            .map_or_else(|| id.to_string(), |profile| profile.value.label.clone()),
        public: selectable.contains(id),
    }
}

/// `None` without a config desk or a loaded persona (chat is off).
pub(super) async fn style(state: &ApiState, profile: &MemberProfile) -> Option<ReplyStyle> {
    let desk = state.config.as_ref()?;
    let settings = desk.settings().await;
    let choices = desk.profile_choices_for(&settings);
    let snapshot = choices.snapshot.as_deref()?;
    let profiles = &snapshot.active()?.profiles;
    let member_roles: Vec<RoleId> = profile.roles.iter().cloned().map(RoleId::new).collect();
    let assignments: Vec<RoleAssignment> = settings
        .persona
        .role_profiles
        .iter()
        .filter_map(|assignment| {
            Some(RoleAssignment {
                role: RoleId::new(assignment.role_id.clone()),
                profile: ProfileId::parse(&assignment.profile).ok()?,
            })
        })
        .collect();
    let saved = profile
        .reply_style
        .as_deref()
        .and_then(|style| ProfileId::parse(style).ok());
    let query = ProfileQuery {
        member_roles: &member_roles,
        role_assignments: &assignments,
        saved_selection: saved.as_ref(),
        selectable: &choices.selectable,
    };
    let (effective, source) = resolve_profile(profiles, &query);
    // The resolver's own rule: the first assignment the member holds whose
    // profile is readable.
    let winner = settings.persona.role_profiles.iter().find(|assignment| {
        profile.roles.contains(&assignment.role_id)
            && ProfileId::parse(&assignment.profile).is_ok_and(|id| profiles.get(&id).is_some())
    });
    let role_name = winner
        .filter(|_| matches!(source, ProfileSource::RoleAssignment))
        .filter(|_| state.channels.connected())
        .and_then(|assignment| {
            state
                .channels
                .roles()
                .into_iter()
                .find(|role| role.id == assignment.role_id)
                .map(|role| role.name)
        });
    Some(ReplyStyle {
        in_effect: effective.map(|loaded| named(profiles, &choices.selectable, &loaded.value.id)),
        source: match source {
            ProfileSource::RoleAssignment => "role",
            ProfileSource::MemberSelection => "saved",
            ProfileSource::BundleDefault { .. } => "default",
        },
        role_name,
        saved: saved
            .as_ref()
            .map(|id| named(profiles, &choices.selectable, id)),
    })
}
