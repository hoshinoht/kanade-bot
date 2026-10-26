//! Guide reads: the list, one guide, an unsaved preview, the publish plan
//! and a run's progress. None writes anything.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::{StatusCode, Uri},
    response::IntoResponse,
};
use serde::Deserialize;

use super::{
    Guides, Refusal, Renderer, Reply, blocks_refused,
    status::{self, Facts},
};
use crate::api::{
    admin::write::Body,
    auth::{AdminSession, wire},
    dto::guides::{
        self as dto, GuideBlock, GuideDetail, GuideList, GuidePlan, GuidePlanBlocked, GuidePreview,
    },
    error::ApiError,
    listeners::Site,
};
use kanade_domain::guides::{
    Block, Blocked, Guide, Plan, PublishedSlot, Rendered, plan as slot_plan, plan_unpublish,
};

/// `GET /api/admin/guides`: every guide by title, with its chip.
pub(super) async fn list(guides: Guides, State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let all = guides.book().guides().await.map_err(Refusal::unavailable)?;
    let rows = status::rows(&guides, &site, all).await?;
    Ok(Json(GuideList { guides: rows }).into_response())
}

/// `GET /api/admin/guides/{id}`: the draft, its chip and its slots.
pub(super) async fn detail(
    guides: Guides,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let guide = guides.guide(&id).await?;
    let facts = guides.facts(guide).await?;
    let rendered = Renderer::new(&site, &guides.state)
        .render(facts.guide.blocks.clone())
        .await?;
    let Facts { guide, slots, .. } = &facts;
    Ok(Json(GuideDetail {
        guide: status::row(&site, &guides.state, &facts, &rendered),
        blocks: guide.blocks.iter().map(dto::block).collect(),
        created_at: crate::api::dto::iso_instant(guide.created_at),
        created_by: dto::actor(&guide.created_by),
        slots: slots.iter().map(dto::slot_row).collect(),
    })
    .into_response())
}

/// `GuideRender`: unsaved blocks to preview.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuideRender {
    blocks: Vec<GuideBlock>,
}

/// `POST /api/admin/guides/{id}/render` (CSRF; no effect, no key): the
/// editor's unsaved blocks as Discord would show them; `422
/// invalid_blocks` with every problem.
pub(super) async fn preview(
    guides: Guides,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(id): UrlPath<String>,
    Body(body): Body<GuideRender>,
) -> Reply {
    guides.guide(&id).await?;
    let blocks: Vec<Block> = body.blocks.into_iter().map(Block::from).collect();
    match Renderer::new(&site, &guides.state).render(blocks).await? {
        Ok(rendered) => Ok(Json(GuidePreview {
            slots: dto::preview_slots(&rendered.slots),
            warnings: dto::warnings(&rendered.warnings),
        })
        .into_response()),
        Err(errors) => Ok(blocks_refused("invalid_blocks", &errors)),
    }
}

/// What a plan was made from.
pub(super) struct Planned {
    pub guide: Guide,
    pub rendered: Rendered,
    pub plan: Plan,
}

/// A publish plan or an unpublish plan.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PlanKind {
    Publish { delete_old: bool },
    Unpublish,
}

impl Guides {
    /// The saved draft rendered and planned against the published slots
    /// and, while Discord is connected, a live read of the channel (within
    /// the port's budget; `checked: false` without it). An unpublish
    /// deletes every published message and renders nothing.
    pub(super) async fn planned(
        &self,
        site: &Site,
        guide: Guide,
        kind: PlanKind,
    ) -> Result<Result<Planned, axum::response::Response>, Refusal> {
        let slots = self
            .book()
            .slots(guide.id.clone())
            .await
            .map_err(Refusal::unavailable)?;
        let delete_old = match kind {
            PlanKind::Unpublish => {
                let channel = unpublish_channel(&guide, &slots);
                return Ok(Ok(Planned {
                    plan: plan_unpublish(&slots, &channel),
                    rendered: Rendered {
                        slots: Vec::new(),
                        warnings: Vec::new(),
                    },
                    guide,
                }));
            }
            PlanKind::Publish { delete_old } => delete_old,
        };
        let channel = guide.channel_id.clone().ok_or_else(|| {
            Refusal::conflict("no_channel", "Choose a channel for this guide first.")
        })?;
        let rendered = match Renderer::new(site, &self.state)
            .render(guide.blocks.clone())
            .await?
        {
            Ok(rendered) => rendered,
            Err(errors) => return Ok(Err(blocks_refused("draft_invalid", &errors))),
        };
        let inspection = if self.state.channels.connected() {
            self.port.inspect(channel.clone(), slots.clone()).await
        } else {
            None
        };
        let plan = slot_plan(
            &slots,
            &rendered.slots,
            &channel,
            delete_old,
            inspection.as_ref(),
        );
        Ok(Ok(Planned {
            guide,
            rendered,
            plan,
        }))
    }
}

/// The channel an unpublish runs in: the guide's, else its messages'.
fn unpublish_channel(guide: &Guide, slots: &[PublishedSlot]) -> String {
    guide
        .channel_id
        .clone()
        .or_else(|| slots.first().map(|slot| slot.channel_id.clone()))
        .unwrap_or_default()
}

fn blocked(blocked: &Blocked) -> GuidePlanBlocked {
    match blocked {
        Blocked::UnknownSlot {
            channel_id,
            position,
        } => GuidePlanBlocked {
            reason: "unknown_slot",
            channel_id: Some(channel_id.clone()),
            position: Some(*position),
            grants: Vec::new(),
        },
        Blocked::ChannelMissing => GuidePlanBlocked {
            reason: "channel_missing",
            channel_id: None,
            position: None,
            grants: Vec::new(),
        },
        Blocked::Grants(grants) => GuidePlanBlocked {
            reason: "missing_grants",
            channel_id: None,
            position: None,
            grants: grants.iter().map(|grant| grant.as_str()).collect(),
        },
    }
}

pub(super) fn plan_view(planned: &Planned, kind: PlanKind) -> GuidePlan {
    let Planned {
        guide,
        rendered,
        plan,
    } = planned;
    GuidePlan {
        kind: match kind {
            PlanKind::Publish { .. } => "publish",
            PlanKind::Unpublish => "unpublish",
        },
        version: guide.version,
        channel_id: plan.channel_id.clone(),
        delete_old: matches!(kind, PlanKind::Publish { delete_old: true }),
        checked: plan.checked,
        no_op: plan.no_op(),
        plan_hash: plan.hash.clone(),
        counts: dto::counts(&plan.counts),
        needs: (&plan.needs).into(),
        blocked: plan.blocked.as_ref().map(blocked),
        ops: dto::plan_ops(plan, &rendered.slots, &guide.blocks),
        slots: dto::preview_slots(&rendered.slots),
        warnings: dto::warnings(&rendered.warnings),
    }
}

/// `?kind=publish|unpublish` (default `publish`) and `?delete_old=1|0`
/// (default `1`: a channel move deletes the old messages, decision 8).
fn plan_query(uri: &Uri) -> Result<PlanKind, Refusal> {
    let pairs = wire::query_pairs(uri.query());
    let invalid = || Refusal::from(ApiError::INVALID_QUERY);
    let mut kind = "publish".to_owned();
    let mut delete_old = true;
    for (key, value) in pairs {
        match (key.as_str(), value.as_str()) {
            ("kind", "publish" | "unpublish") => kind = value,
            ("delete_old", "1") => delete_old = true,
            ("delete_old", "0") => delete_old = false,
            _ => return Err(invalid()),
        }
    }
    Ok(if kind == "unpublish" {
        PlanKind::Unpublish
    } else {
        PlanKind::Publish { delete_old }
    })
}

/// `GET /api/admin/guides/{id}/plan`: the saved version's plan, its hash
/// to publish with, what needs confirming and why it cannot run, if so.
pub(super) async fn plan(
    guides: Guides,
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(id): UrlPath<String>,
    uri: Uri,
) -> Reply {
    let kind = plan_query(&uri)?;
    let guide = guides.guide(&id).await?;
    Ok(match guides.planned(&site, guide, kind).await? {
        Ok(planned) => Json(plan_view(&planned, kind)).into_response(),
        Err(refused) => refused,
    })
}

/// `GET /api/admin/guides/{id}/publishes/{run}`: a run and its op log.
pub(super) async fn run(
    guides: Guides,
    _: AdminSession,
    UrlPath((id, run)): UrlPath<(String, String)>,
) -> Reply {
    let gone = || Refusal::not_found("not_found", "That publish run is gone.");
    let (publish, ops) = guides
        .book()
        .publish(run)
        .await
        .map_err(Refusal::unavailable)?
        .filter(|(publish, _)| publish.guide_id == id)
        .ok_or_else(gone)?;
    let total = guides.planned_total(&publish).await?;
    Ok((StatusCode::OK, Json(dto::run(&publish, &ops, total))).into_response())
}
