//! The Chat, Extractions and Rewrites logs and rescan jobs (A7). Lists return every
//! match, newest first (the PWA pages client-side; cursor paging stays
//! proposed), with the unfiltered `total` and the filter facets.

mod chat;
mod extractions;
mod filter;
pub(super) mod rescan;
mod rewrites;

use std::{collections::BTreeMap, sync::Arc};

use axum::{Router, routing::get};

use super::{
    context::{roster, unavailable},
    write::{Refusal, state},
};
use crate::{
    api::{
        dto::logs::Names,
        listeners::Site,
        state::{ApiState, ChannelEntry},
    },
    domain::members::Roster,
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/chat", get(chat::list))
        .route("/api/admin/chat/{id}", get(chat::detail))
        .route("/api/admin/extractions", get(extractions::list))
        .route("/api/admin/extractions/{id}", get(extractions::detail))
        .route("/api/admin/rewrites", get(rewrites::list))
        .route("/api/admin/rewrites/{id}", get(rewrites::detail))
        .route("/api/admin/rescan/targets", get(rescan::targets))
        .route("/api/admin/rescan", axum::routing::post(rescan::submit))
        .route(
            "/api/admin/rescan/{id}",
            get(rescan::poll).delete(rescan::cancel),
        )
}

type Reply = Result<axum::response::Response, Refusal>;

/// Members and channels by id, for names.
struct Directory {
    roster: Roster,
    channels: BTreeMap<String, ChannelEntry>,
}

impl Directory {
    async fn load(state: &ApiState) -> Result<Self, Refusal> {
        let profiles = state
            .store
            .members()
            .await
            .map_err(|error| Refusal::from(unavailable(error)))?;
        Ok(Self {
            roster: roster(&profiles),
            channels: state
                .channels
                .channels()
                .into_iter()
                .map(|channel| (channel.id.clone(), channel))
                .collect(),
        })
    }

    fn names(&self) -> Names<'_> {
        Names {
            roster: &self.roster,
            channels: &self.channels,
        }
    }
}

fn store_down<E>(_: E) -> Refusal {
    crate::api::error::ApiError::UNAVAILABLE.into()
}
