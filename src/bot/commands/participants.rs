//! v4 `_resolve_participants` (with `include_invoker=False`): member
//! pickers first, then typed names, with v4's problem wording. The
//! scheduler validates the result again on write.

use crate::chat::tools::read::roster::{bossers, resolve_participant_text};
use crate::domain::members::{Member, MemberProfile};

use super::text::mention;

/// The ids for a party, or v4's `; `-joined problems.
///
/// # Errors
/// The problems text (shown as `❌ {problems}`).
pub fn resolve_participants(
    profiles: &[MemberProfile],
    typed: Option<&str>,
    picked: &[String],
) -> Result<Vec<String>, String> {
    let member = |uid: &str| profiles.iter().find(|p| p.member.user_id == uid);
    let is_bot = |uid: &str| member(uid).is_some_and(|p| p.member.is_bot);
    let has_role = |uid: &str| member(uid).is_some_and(|p| p.member.has_role);

    let mut ids: Vec<String> = Vec::new();
    let add = |ids: &mut Vec<String>, uid: &str| {
        if !ids.iter().any(|seen| seen == uid) {
            ids.push(uid.to_owned());
        }
    };
    let mut bots: Vec<String> = Vec::new();
    for uid in picked {
        if is_bot(uid) {
            bots.push(mention(uid));
        } else {
            add(&mut ids, uid);
        }
    }

    let members: Vec<Member> = profiles.iter().map(|p| p.member.clone()).collect();
    let roster = bossers(&members);
    let mut resolution = resolve_participant_text(typed.unwrap_or_default(), &roster);
    // v4 also matched `/nick` aliases; the shared resolver reads names only,
    // so an exact alias settles a token it could not place.
    resolution.unknown.retain(|token| {
        let alias = token.to_lowercase();
        let holders: Vec<&MemberProfile> = profiles
            .iter()
            .filter(|p| p.member.has_role && p.aliases.contains(&alias))
            .collect();
        match holders.as_slice() {
            [one] => {
                resolution.ids.push(one.member.user_id.clone());
                false
            }
            _ => true,
        }
    });
    for uid in &resolution.ids {
        add(&mut ids, uid);
    }

    let mut problems: Vec<String> = Vec::new();
    for (token, names) in &resolution.ambiguous {
        problems.push(format!(
            "`{token}` could be {} - use the member pickers",
            names.join(" or ")
        ));
    }
    if !resolution.unknown.is_empty() {
        let unknown: Vec<String> = resolution
            .unknown
            .iter()
            .map(|token| format!("`{token}`"))
            .collect();
        problems.push(format!(
            "couldn't match: {} - use the member pickers or `/nick`",
            unknown.join(", ")
        ));
    }
    let outsiders: Vec<String> = ids
        .iter()
        .filter(|uid| !has_role(uid))
        .map(|uid| mention(uid))
        .collect();
    for uid in &ids {
        if is_bot(uid) && !bots.contains(&mention(uid)) {
            bots.push(mention(uid));
        }
    }
    if !bots.is_empty() {
        ids.retain(|uid| !bots.contains(&mention(uid)));
        problems.push(format!("bots can't be participants: {}", bots.join(", ")));
    }
    if !outsiders.is_empty() {
        problems.push(format!("not in the bossing role: {}", outsiders.join(", ")));
    }
    if ids.is_empty() && problems.is_empty() {
        problems.push("a run needs at least one participant".to_owned());
    }
    if problems.is_empty() {
        Ok(ids)
    } else {
        Err(problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, name: &str, role: bool, bot: bool) -> MemberProfile {
        MemberProfile {
            member: Member {
                user_id: id.into(),
                display_name: Some(name.into()),
                has_role: role,
                is_bot: bot,
                ..Member::default()
            },
            ..MemberProfile::default()
        }
    }

    fn roster() -> Vec<MemberProfile> {
        let mut alvin = profile("1001", "Alvin", true, false);
        alvin.aliases = vec!["my".into()];
        vec![
            alvin,
            profile("1002", "kanon", true, false),
            profile("1003", "Kanade", true, false),
            profile("1004", "Outsider", false, false),
            profile("1005", "Botty", true, true),
        ]
    }

    #[test]
    fn pickers_then_names_then_aliases() {
        let ids = resolve_participants(&roster(), Some("MY, kanon"), &["1003".into()]);
        assert_eq!(ids, Ok(vec!["1003".into(), "1002".into(), "1001".into()]));
    }

    #[test]
    fn problems_use_v4_wording_and_order() {
        let error = resolve_participants(
            &roster(),
            Some("kan, nobody"),
            &["1005".into(), "1004".into()],
        )
        .unwrap_err();
        assert_eq!(
            error,
            "`kan` could be Kanade or kanon - use the member pickers; couldn't match: \
             `nobody` - use the member pickers or `/nick`; bots can't be participants: <@1005>; \
             not in the bossing role: <@1004>"
        );
        assert_eq!(
            resolve_participants(&roster(), None, &[]),
            Err("a run needs at least one participant".into())
        );
    }
}
