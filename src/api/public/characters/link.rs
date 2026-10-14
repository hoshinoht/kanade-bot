//! The member's NEXON consent flow (module `character_data`): the Add
//! lookup preview (never stored), a character's link, the notice (v1, the
//! words `/character link` shows), Link (consent, surface `portal`, then
//! the first fetch: the same path as Discord's Link), Cancel (changes
//! nothing; an admin's request stays), Unlink (no confirm: the data is
//! deleted at once and share cards are stripped), Unlink all, and My data.

use std::sync::Arc;

use axum::{
    Json,
    extract::{
        Path as UrlPath, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::{
    module, own, roster as roster_of,
    roster::{pending, requester},
};
use crate::{
    api::{
        admin::{
            character_data::{lookup_answer, not_connected},
            replay::{Keyed, replayed},
            write::{Refusal, state},
        },
        auth::{
            audit::{AuditContext, AuditEvent},
            member::MemberSession,
            rate::MEMBER_LOOKUP_ROUTE,
        },
        dto::{
            character_data::{CharacterLink, facts, link, link_state},
            history::SettingsActor,
            iso_instant,
            public::{
                LinkCancelled, LinkNotice, LinkOutcome, MyData, MyDataEntry, NoticeSection,
                SharedCard, Unlinked, UnlinkedAll,
            },
        },
        error::ApiError,
        listeners::Site,
        public::write::{admit, invalid_body, path_id},
        state::ApiState,
    },
    bot::chardata::copy::{NoticePart, notice_text},
    domain::{
        characters::{Character, parse_ign},
        chardata::{
            ATTRIBUTION, ConsentSurface, LINK_REGION, LinkStatus, NOTICE_VERSION, PENDING_TTL,
            UnlinkReason,
        },
        history::{Actor, Surface},
    },
    infrastructure::{
        character_data::{CharacterData, ConfirmOutcome, FetchState, LinkView},
        store::replays::ReplayScope,
    },
};

type Reply = Result<Response, Refusal>;

fn retired() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "retired",
        "A retired character can't be linked.",
    )
}

fn region() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "region",
        "Only MapleSEA characters can be linked.",
    )
}

fn not_linked() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_linked",
        "That character is not linked.",
    )
}

/// The caller's own character `id` and the module.
async fn mine<'a>(
    state: &'a ApiState,
    session: &MemberSession,
    id: &str,
) -> Result<(Character, &'a Arc<CharacterData>), Refusal> {
    let character = own(roster_of(state)?.as_ref(), &session.user_id, id).await?;
    Ok((character, module(state)?))
}

async fn view_of(module: &CharacterData, id: &str) -> Result<CharacterLink, Refusal> {
    let view = module.view(id).await.map_err(Refusal::unavailable)?;
    Ok(link(id, &view, module.privacy_url()))
}

/// `LookupPreview`: the IGN as typed (MapleSEA only).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LookupBody {
    ign: String,
}

/// `POST /api/public/me/characters/lookup` → `CharacterLookup`: NEXON's
/// answer for a typed IGN (`id` + `basic`), shown once and never stored.
/// Ten per ten minutes per member, besides the shared daily budget and the
/// per-caller spacing.
pub(super) async fn look_up(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    body: Result<Json<LookupBody>, JsonRejection>,
) -> Reply {
    let member = site.member.as_deref().ok_or(ApiError::AUTH_UNAVAILABLE)?;
    if !member
        .rate()
        .take_member_lookup(&session.user_id, member.now())
    {
        member.audit(
            &audit,
            AuditEvent::RateLimited {
                route: MEMBER_LOOKUP_ROUTE,
            },
        );
        return Err(ApiError::RATE_LIMITED.into());
    }
    let Ok(Json(body)) = body else {
        return Err(invalid_body());
    };
    let ign = parse_ign(&body.ign).map_err(|error| Refusal::invalid(error.to_string()))?;
    let module = module(state(&site)?)?;
    lookup_answer(
        module
            .look_up(&Actor::member(session.user_id.clone()), &ign)
            .await
            .map_err(Refusal::unavailable)?,
    )
}

/// `GET /api/public/me/characters/{id}/link` → `CharacterLink`.
pub(super) async fn status(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Reply {
    let id = path_id(path)?;
    let state = state(&site)?;
    let (_, module) = mine(state, &session, &id).await?;
    Ok(Json(view_of(module, &id).await?).into_response())
}

fn section(part: NoticePart) -> NoticeSection {
    NoticeSection {
        heading: part.heading,
        text: part.text,
    }
}

/// `GET /api/public/me/characters/{id}/link/notice` → `LinkNotice`: the
/// notice for one of the caller's active MapleSEA characters that is not
/// linked, with the admin's request when one waits.
pub(super) async fn notice(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Reply {
    let id = path_id(path)?;
    let state = state(&site)?;
    let (character, module) = mine(state, &session, &id).await?;
    if !module.connected() {
        return Err(not_connected());
    }
    if character.world != LINK_REGION {
        return Err(region());
    }
    if !character.is_active() {
        return Err(retired());
    }
    let view = module.status(&id).await.map_err(Refusal::unavailable)?;
    let request = match &view.link {
        Some(found) if found.status == LinkStatus::Active => {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "already_linked",
                "That character is already linked.",
            ));
        }
        Some(found)
            if found.status == LinkStatus::Pending && found.requested_by.kind() == "admin" =>
        {
            let profiles = state.store.members().await.map_err(Refusal::unavailable)?;
            Some(pending(&character, found, &profiles))
        }
        _ => None,
    };
    let text = notice_text(
        &character.ign,
        request
            .as_ref()
            .map(|request| request.requested_by_name.as_deref()),
    );
    Ok(Json(LinkNotice {
        character_id: id,
        ign: character.ign,
        version: NOTICE_VERSION,
        title: text.title,
        lead: text.lead,
        what: section(text.what),
        why: section(text.why),
        how_long: section(text.how_long),
        who: section(text.who),
        as_is: text.as_is,
        link_label: text.link_label,
        cancel_label: text.cancel_label,
        request,
        privacy_url: module.privacy_url().map(str::to_owned),
        attribution: ATTRIBUTION,
    })
    .into_response())
}

/// `LinkConfirm`: the notice version the member was shown.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfirmBody {
    notice: u32,
}

fn outcome(state: &'static str, link: Option<CharacterLink>) -> Response {
    Json(LinkOutcome { state, link }).into_response()
}

/// `POST /api/public/me/characters/{id}/link` `{notice}` → `LinkOutcome`:
/// the member's consent (recorded with surface `portal`) and the first
/// fetch, as Discord's Link. A retry after it linked answers `linked`.
pub(super) async fn confirm(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
    body: Result<Json<ConfirmBody>, JsonRejection>,
) -> Reply {
    let (member, _) = admit(&site, &audit, &session, &headers)?;
    session.require_fresh(member.now())?;
    let id = path_id(path)?;
    let Ok(Json(ConfirmBody { notice })) = body else {
        return Err(invalid_body());
    };
    let state = state(&site)?;
    let (character, module) = mine(state, &session, &id).await?;
    if character.world != LINK_REGION {
        return Ok(outcome("other_region", None));
    }
    if !character.is_active() {
        return Err(retired());
    }
    let me = session.user_id.as_str();
    match module
        .confirm_link(me, &id, notice, ConsentSurface::Portal)
        .await
        .map_err(Refusal::unavailable)?
    {
        ConfirmOutcome::Linked(_, FetchState::Stored) => {
            let state = if module
                .data(&id)
                .await
                .map_err(Refusal::unavailable)?
                .is_some()
            {
                "linked"
            } else {
                "linked_later"
            };
            Ok(outcome(state, Some(view_of(module, &id).await?)))
        }
        ConfirmOutcome::Linked(_, FetchState::Failed { .. }) => {
            Ok(outcome("linked_later", Some(view_of(module, &id).await?)))
        }
        // NEXON has no such character: nothing is kept, not even the link.
        ConfirmOutcome::Linked(_, FetchState::Stale) => {
            module
                .unlink(
                    &id,
                    &Actor::member(me),
                    Surface::PublicPortal,
                    UnlinkReason::NotFound,
                    None,
                )
                .await
                .map_err(Refusal::unavailable)?;
            Ok(outcome("not_found", None))
        }
        ConfirmOutcome::Linked(_, FetchState::Unlinked) => Err(ApiError::UNAVAILABLE.into()),
        ConfirmOutcome::AlreadyLinked => Ok(outcome("linked", Some(view_of(module, &id).await?))),
        ConfirmOutcome::Busy => Ok(outcome("busy", None)),
        ConfirmOutcome::Unavailable(_) => Ok(outcome("unavailable", None)),
        ConfirmOutcome::NotConnected => Err(not_connected()),
        ConfirmOutcome::NoticeOutdated => Err(Refusal::new(
            StatusCode::CONFLICT,
            "notice_changed",
            "The notice changed: read it again before linking.",
        )),
        ConfirmOutcome::NotEligible => Err(ApiError::NOT_FOUND.into()),
        ConfirmOutcome::Conflict => Err(Refusal::new(
            StatusCode::CONFLICT,
            "stale",
            "The link changed meanwhile: try again.",
        )),
    }
}

/// `POST /api/public/me/characters/{id}/link/cancel` → `LinkCancelled`:
/// nothing changes; an admin's request stays waiting (no request of the
/// member's own is made).
pub(super) async fn cancel(
    State(site): State<Arc<Site>>,
    session: MemberSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Reply {
    let id = path_id(path)?;
    let state = state(&site)?;
    let (_, module) = mine(state, &session, &id).await?;
    let view = module.status(&id).await.map_err(Refusal::unavailable)?;
    let request = view.link.filter(|found| {
        found.status == LinkStatus::Pending && found.requested_by.kind() == "admin"
    });
    let requested_by_name = match &request {
        Some(found) => requester(
            found,
            &state.store.members().await.map_err(Refusal::unavailable)?,
        ),
        None => None,
    };
    Ok(Json(LinkCancelled {
        request_kept: request.is_some(),
        requested_by_name,
    })
    .into_response())
}

/// `DELETE /api/public/me/characters/{id}/link` → `Unlinked`: no confirm
/// and no fresh sign-in; the link and its data go in one write and its
/// share cards are queued for stripping. `404 not_linked` without a link.
pub(super) async fn unlink(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
) -> Reply {
    let (_, key) = admit(&site, &audit, &session, &headers)?;
    let id = path_id(path)?;
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let keyed = Keyed::member(
        ReplayScope::Config,
        me,
        key,
        &format!("character.unlink\u{1f}{id}"),
    );
    let now = state.now();
    if let Some(found) = keyed.recall(state.store.as_ref(), now).await? {
        return replayed(&found);
    }
    let (character, module) = mine(state, &session, &id).await?;
    let cards = module
        .character_posts(&id)
        .await
        .map_err(Refusal::unavailable)?
        .len();
    let body = json!(Unlinked {
        unlinked: character.ign,
        cards: u32::try_from(cards).unwrap_or(u32::MAX),
    });
    let replay = keyed.answered(StatusCode::OK, &body, now);
    match module
        .unlink(
            &id,
            &Actor::member(me),
            Surface::PublicPortal,
            UnlinkReason::Requested,
            Some(replay),
        )
        .await
        .map_err(Refusal::unavailable)?
    {
        Some(_) => Ok(Json(body).into_response()),
        None => Err(not_linked()),
    }
}

/// `DELETE /api/public/me/characters/links` → `UnlinkedAll`: every link of
/// the caller's. Each unlink is its own atomic write (links and History live
/// in different files, so all of them can't be one); every link is tried,
/// and if any failed the answer is `503` with nothing recorded under the
/// key, so a retry removes the rest and the ones already gone are skipped.
pub(super) async fn unlink_all(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
) -> Reply {
    let (_, key) = admit(&site, &audit, &session, &headers)?;
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let keyed = Keyed::member(
        ReplayScope::Config,
        me,
        key,
        &format!("character.unlink_all\u{1f}{me}"),
    );
    let now = state.now();
    if let Some(found) = keyed.recall(state.store.as_ref(), now).await? {
        return replayed(&found);
    }
    let module = module(state)?;
    let actor = Actor::member(me);
    let mut unlinked = Vec::new();
    let mut failed = None;
    for entry in module
        .member_characters(me)
        .await
        .map_err(Refusal::unavailable)?
    {
        if entry.link.is_none() {
            continue;
        }
        match module
            .unlink(
                &entry.character.id,
                &actor,
                Surface::PublicPortal,
                UnlinkReason::Requested,
                None,
            )
            .await
        {
            Ok(Some(_)) => unlinked.push(entry.character.ign),
            Ok(None) => {}
            Err(error) => {
                failed = Some(Refusal::unavailable(error));
            }
        }
    }
    if let Some(refusal) = failed {
        return Err(refusal);
    }
    let body = json!(UnlinkedAll { unlinked });
    if state
        .store
        .put_replay(keyed.answered(StatusCode::OK, &body, now))
        .await
        .is_err()
    {
        // A retry finds nothing left and answers an empty list.
        crate::runtime::logging::log!(warn, "idempotency_replay_unrecorded");
    }
    Ok(Json(body).into_response())
}

/// `GET /api/public/me/data` → `MyData`: everything kept for the caller,
/// with dates (`/character data`): waiting requests first, then the active
/// main, other active characters and retired ones, each group by IGN.
pub(super) async fn data(State(site): State<Arc<Site>>, session: MemberSession) -> Reply {
    let state = state(&site)?;
    let module = module(state)?;
    let profiles = state.store.members().await.map_err(Refusal::unavailable)?;
    let channels = state.channels.channels();
    let now = module.now();
    // Each entry with its place: the roster's own order breaks creation-time
    // ties by random id, so the order is spelled out here.
    let mut entries = Vec::new();
    for entry in module
        .member_characters(&session.user_id)
        .await
        .map_err(Refusal::unavailable)?
    {
        let character = &entry.character;
        let linkable = entry.linkable();
        let Some(kept) = entry.link else {
            if linkable {
                entries.push((
                    order(character, false),
                    MyDataEntry {
                        character_id: character.id.clone(),
                        ign: character.ign.clone(),
                        state: "not_linked",
                        retired: false,
                        data: None,
                        requested_by: None,
                        requested_by_name: None,
                        requested_at: None,
                        request_expires_at: None,
                        linked_at: None,
                        notice: None,
                        consented_at: None,
                        consent_surface: None,
                        cards: Vec::new(),
                    },
                ));
            }
            continue;
        };
        let found = &kept.link;
        let pending = found.status == LinkStatus::Pending;
        let state = match link_state(&LinkView {
            connected: module.connected(),
            link: Some(found.clone()),
            snapshot: kept.snapshot.clone(),
            refresh: None,
            now,
        }) {
            "pending" => "pending",
            "stale" => "stale",
            "expired" => "expired",
            _ => "linked",
        };
        entries.push((
            order(character, pending),
            MyDataEntry {
                character_id: character.id.clone(),
                ign: character.ign.clone(),
                state,
                retired: !character.is_active(),
                data: kept
                    .snapshot
                    .as_ref()
                    .filter(|snapshot| snapshot.has_data())
                    .map(facts),
                requested_by: pending.then(|| SettingsActor {
                    kind: found.requested_by.kind(),
                    id: found.requested_by.id().to_owned(),
                }),
                requested_by_name: if pending {
                    requester(found, &profiles)
                } else {
                    None
                },
                requested_at: pending.then(|| iso_instant(found.requested_at)),
                request_expires_at: pending.then(|| iso_instant(found.requested_at + PENDING_TTL)),
                linked_at: found.linked_at.map(iso_instant),
                notice: found.consent_version,
                consented_at: found.consented_at.map(iso_instant),
                consent_surface: found.consent_surface.map(ConsentSurface::as_str),
                cards: kept
                    .posts
                    .iter()
                    .map(|post| SharedCard {
                        // The channel's name only: its id never leaves the server.
                        channel: channels
                            .iter()
                            .find(|channel| channel.id == post.channel_id)
                            .map(|channel| channel.name.clone()),
                        posted_at: iso_instant(post.posted_at),
                        cleared_by: iso_instant(post.strip_due_at),
                    })
                    .collect(),
            },
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Json(MyData {
        entries: entries.into_iter().map(|(_, entry)| entry).collect(),
        notice_version: NOTICE_VERSION,
        privacy_url: module.privacy_url().map(str::to_owned),
        attribution: ATTRIBUTION,
    })
    .into_response())
}

/// My data's place for a character: waiting for the member first, then the
/// active main, other active characters, retired ones; by IGN within each
/// (case-insensitive, then exact, then id, so the order is total).
fn order(character: &Character, pending: bool) -> (bool, bool, bool, String, String, String) {
    (
        !pending,
        !character.is_active(),
        !character.is_active_main(),
        character.ign_key(),
        character.ign.clone(),
        character.id.clone(),
    )
}
