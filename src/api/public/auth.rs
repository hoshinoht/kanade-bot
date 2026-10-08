//! Member sign-in and sign-out: `GET …/discord/start?next=/path` → Discord
//! (`identify`, `prompt=none`, PKCE S256, one-use state bound to
//! `__Host-kanade_pub_login`) → `GET …/discord/callback` → `200` landing
//! that navigates to `next` with `__Host-kanade_pub`, or `303
//! /?login_error=<code>`. An ineligible member gets `not_eligible` with no
//! session row and no session cookie. `POST …/logout` ends this browser's
//! session and clears both cookies, open portal or not.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri, header::SET_COOKIE},
    response::{IntoResponse, Response},
};

use crate::api::{
    auth::{
        actor_id,
        audit::{AuditContext, AuditEvent},
        crypto, csrf, device,
        discord::{BeginError, CodeExchange, DiscordError, DiscordLogin},
        member::{Eligibility, MemberAuth, SESSION_SAME_SITE, StartError},
        rate::Route,
        wire::{self, MEMBER_LOGIN_COOKIE, MEMBER_SESSION_COOKIE, landing, see_other},
    },
    error::ApiError,
    listeners::Site,
};
use crate::infrastructure::store::web_sessions::{LoginMethod, SessionOrigin};

/// The pre-auth cookie must survive Discord's cross-site redirect back.
const LOGIN_SAME_SITE: &str = "Lax";
/// `discord::LOGIN_TTL`.
const LOGIN_COOKIE_SECONDS: i64 = 600;

/// Browser-flow failures land on the app with a fixed code, never details.
fn login_error(code: &'static str) -> Response {
    see_other(
        &format!("/?login_error={code}"),
        [wire::clear_cookie(MEMBER_LOGIN_COOKIE, LOGIN_SAME_SITE)],
    )
}

/// The realm and its login while the portal is open.
fn open_login(site: &Site) -> Option<(&MemberAuth, &DiscordLogin)> {
    let member = site.member.as_deref().filter(|member| member.is_open())?;
    Some((member, member.discord()?))
}

/// Take a rate-limit token from the member realm's own buckets; audits a refusal.
fn admitted(member: &MemberAuth, context: &AuditContext, route: Route) -> bool {
    let admitted = member.rate().take(route, context.client, member.now());
    if !admitted {
        member.audit(
            context,
            AuditEvent::RateLimited {
                route: route.as_str(),
            },
        );
    }
    admitted
}

pub(super) async fn start(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    uri: Uri,
) -> Response {
    let Some((member, discord)) = open_login(&site) else {
        return login_error("closed");
    };
    if !admitted(member, &context, Route::DiscordStart) {
        return login_error("rate_limited");
    }
    let now = member.now();
    if discord.cooling_down(now) {
        return login_error("unavailable");
    }
    let pairs = wire::query_pairs(uri.query());
    let next = wire::safe_next(wire::query_value(&pairs, "next").as_deref());
    match discord.begin(next, now, context.client) {
        Ok(started) => see_other(
            &started.authorize_url,
            [wire::set_cookie(
                MEMBER_LOGIN_COOKIE,
                &started.login_id,
                LOGIN_SAME_SITE,
                LOGIN_COOKIE_SECONDS,
            )],
        ),
        Err(BeginError::Busy) => {
            member.audit(
                &context,
                AuditEvent::RateLimited {
                    route: "pending_logins",
                },
            );
            login_error("rate_limited")
        }
        Err(BeginError::Unavailable) => login_error("unavailable"),
    }
}

pub(super) async fn callback(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let pairs = wire::query_pairs(uri.query());
    let login_id = wire::cookie(&headers, MEMBER_LOGIN_COOKIE);
    let state = wire::query_value(&pairs, "state");
    // One use, whatever happens next: closed, rate-limited or refused.
    let resume = |discord: &DiscordLogin, now| match (&login_id, &state) {
        (Some(login_id), Some(state)) => discord.resume(login_id, state, now),
        _ => None,
    };
    let Some((member, discord)) = open_login(&site) else {
        if let Some(member) = site.member.as_deref()
            && let Some(discord) = member.discord()
        {
            let _ = resume(discord, member.now());
        }
        return login_error("closed");
    };
    let resumed = resume(discord, member.now());
    if !admitted(member, &context, Route::DiscordCallback) {
        return login_error("rate_limited");
    }
    let refused = |reason: &'static str, user: Option<&str>, code: &'static str| {
        member.audit(
            &context,
            AuditEvent::LoginRefused {
                method: "discord",
                reason,
                user: user.map(str::to_owned),
            },
        );
        login_error(code)
    };
    let cooled = |wait| {
        discord.cool_down(member.now(), wait);
        refused("discord_rate_limited", None, "unavailable")
    };
    let Some(resumed) = resumed else {
        return refused("state", None, "state");
    };
    // Discord's own error text is never echoed.
    if wire::query_value(&pairs, "error").is_some() {
        return refused("denied", None, "denied");
    }
    let Some(code) =
        wire::query_value(&pairs, "code").filter(|code| (1..=512).contains(&code.len()))
    else {
        return refused("no_code", None, "discord");
    };
    if discord.cooling_down(member.now()) {
        return refused("discord_cooling_down", None, "unavailable");
    }
    let grant = match discord
        .api
        .exchange_code(CodeExchange {
            client: &discord.client,
            code: &code,
            code_verifier: &resumed.verifier,
        })
        .await
    {
        Ok(grant) => grant,
        Err(DiscordError::Rejected) => return refused("code_rejected", None, "discord"),
        Err(DiscordError::RateLimited(wait)) => return cooled(wait),
        Err(_) => return refused("discord_unavailable", None, "unavailable"),
    };
    // A broader grant is refused without using the token at all.
    let user = if grant.identify_only() {
        Some(discord.api.current_user(&grant.token).await)
    } else {
        None
    };
    let user_id = user
        .as_ref()
        .and_then(|user| user.as_ref().ok())
        .map(|user| user.id.clone());
    // One call per token: revoked before the identity is used.
    if let Err(error) = discord.api.revoke(&discord.client, grant.token).await {
        if let DiscordError::RateLimited(wait) = error {
            discord.cool_down(member.now(), wait);
        }
        member.audit(
            &context,
            AuditEvent::RevokeFailed {
                reason: match error {
                    DiscordError::RateLimited(_) => "rate_limited",
                    DiscordError::Rejected => "rejected",
                    _ => "unavailable",
                },
                user: user_id,
            },
        );
    }
    let user = match user {
        None => return refused("scope", None, "discord"),
        Some(Ok(user)) => user,
        Some(Err(DiscordError::Rejected)) => return refused("user_rejected", None, "discord"),
        Some(Err(DiscordError::RateLimited(wait))) => return cooled(wait),
        Some(Err(_)) => return refused("discord_unavailable", None, "unavailable"),
    };
    let eligibility = if user.bot {
        Eligibility::NotEligible
    } else {
        member.gate().check(&user.id).await
    };
    match eligibility {
        Eligibility::Eligible => {}
        Eligibility::NotEligible => {
            // No session row, no session cookie; a session this member still
            // holds elsewhere ends now rather than at its next re-check.
            let _ = member.end_all(&context, &user.id, "not_eligible").await;
            return refused("not_eligible", Some(&user.id), "not_eligible");
        }
        Eligibility::Unavailable => {
            return refused("member_data_unavailable", Some(&user.id), "unavailable");
        }
    }
    let replaces = wire::cookie(&headers, MEMBER_SESSION_COOKIE);
    let id = match member
        .start_session(
            &context,
            &user,
            device::label(&headers),
            replaces.as_deref(),
        )
        .await
    {
        Ok(id) => id,
        Err(StartError::NotEligible) => {
            return refused("not_eligible", Some(&user.id), "not_eligible");
        }
        Err(StartError::Store) => return login_error("unavailable"),
    };
    landing(
        &resumed.next,
        [
            wire::set_cookie(
                MEMBER_SESSION_COOKIE,
                &id,
                SESSION_SAME_SITE,
                member.policy().absolute.num_seconds(),
            ),
            wire::clear_cookie(MEMBER_LOGIN_COOKIE, LOGIN_SAME_SITE),
        ],
    )
}

/// `204`, deleting this browser's session (if any) and clearing both
/// cookies. Same-origin markers are always required, and the session's
/// member CSRF token whenever a session cookie comes with the request. A
/// rotated-out id in its D9 grace may only read, so it gets `401` and its
/// cookie is kept (the browser may already hold the new id).
pub(super) async fn logout(
    State(site): State<Arc<Site>>,
    context: AuditContext,
    headers: HeaderMap,
) -> Response {
    let Some(member) = site.member.as_deref() else {
        return ApiError::AUTH_UNAVAILABLE.into_response();
    };
    if !csrf::same_origin(&headers) {
        return ApiError::CSRF.into_response();
    }
    let id = wire::cookie(&headers, MEMBER_SESSION_COOKIE);
    if let Some(id) = &id {
        if !csrf::token_matches(csrf::MEMBER, &headers, id) {
            return ApiError::CSRF.into_response();
        }
        let hash = crypto::sha256_hex(id.as_bytes());
        let now = member.now();
        let row = match member.sessions().load_session(&hash).await {
            Ok(Some(row)) => Some(row),
            Ok(None) => match member.sessions().load_superseded(&hash, now).await {
                Ok(Some(_)) => return ApiError::UNAUTHENTICATED.into_response(),
                Ok(None) => None,
                Err(_) => return ApiError::UNAVAILABLE.into_response(),
            },
            Err(_) => return ApiError::UNAVAILABLE.into_response(),
        };
        if let Some(row) = row.filter(|row| row.origin == SessionOrigin::Public) {
            if member.sessions().delete_session(&hash).await.is_err() {
                return ApiError::UNAVAILABLE.into_response();
            }
            member.audit(
                &context,
                AuditEvent::SessionEnded {
                    actor: actor_id(LoginMethod::Discord, &row.subject),
                    reason: "logout",
                },
            );
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.append(
        SET_COOKIE,
        wire::clear_cookie(MEMBER_SESSION_COOKIE, SESSION_SAME_SITE),
    );
    headers.append(
        SET_COOKIE,
        wire::clear_cookie(MEMBER_LOGIN_COOKIE, LOGIN_SAME_SITE),
    );
    response
}
