//! The running role models. The config API swaps them after a save; every
//! session open reads the governor route this keeps current (alias and
//! resolved effort), so the next question, extraction or rewrite uses them
//! and sessions already open keep theirs.

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use super::super::{
    Effort, ListedModel,
    governor::{ConfigError, Governor, GroupConfig, Role},
};
use super::{
    ModelRoles, RoleEffort, SetupError,
    catalog::{self, CatalogState, variant_of},
    effort::{self, EffortStatus},
};

/// What a role's sessions open with now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningRole {
    pub alias: String,
    pub effort: Option<Effort>,
}

/// One role whose running alias or effort changed (`None`: unrouted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleSwap {
    pub role: Role,
    pub before: Option<RunningRole>,
    pub after: Option<RunningRole>,
}

#[derive(Debug)]
pub(super) struct LiveRoles {
    /// Also serialises every push to the governor (swaps and listings).
    roles: Mutex<ModelRoles>,
    /// The default single group a new alias joins; `None` with declared groups.
    open: Option<GroupConfig>,
}

impl LiveRoles {
    pub fn new(roles: ModelRoles, open: Option<GroupConfig>) -> Self {
        Self {
            roles: Mutex::new(roles),
            open,
        }
    }

    fn lock(&self) -> MutexGuard<'_, ModelRoles> {
        self.roles.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self) -> ModelRoles {
        self.lock().clone()
    }

    /// Pushes the resolved efforts (startup, before any listing).
    pub fn refresh(&self, governor: &Governor, catalog: &CatalogState) {
        let roles = self.lock();
        push_efforts(&roles, governor, catalog);
    }

    /// Listing observer: stores the listing, re-derives zones and efforts.
    /// Under the roles lock, so a concurrent switch never re-derives from an
    /// older listing after this one.
    pub fn observe(&self, listed: &[ListedModel], governor: &Governor, catalog: &CatalogState) {
        let roles = self.lock();
        catalog.apply(listed, governor);
        push_efforts(&roles, governor, catalog);
    }

    /// Reroutes changed aliases (new ones start external until a listing
    /// confirms their zone; the cached listing counts) and pushes the
    /// resolved efforts. All-or-nothing: checked before anything moves.
    pub fn apply(
        &self,
        next: ModelRoles,
        governor: &Governor,
        catalog: &CatalogState,
    ) -> Result<Vec<RoleSwap>, SetupError> {
        if next.extraction.effort == RoleEffort::Inherit {
            return Err(SetupError::ExtractionInherits);
        }
        for role in ModelRoles::ALL {
            if next
                .get(role)
                .alias
                .as_deref()
                .is_some_and(|alias| alias.trim().is_empty())
            {
                return Err(SetupError::Governor(ConfigError::EmptyRoleAlias { role }));
            }
        }
        let mut roles = self.lock();
        let before = running(governor);
        for role in ModelRoles::ALL {
            governor
                .reroute(role, next.get(role).alias.as_deref(), self.open.as_ref())
                .map_err(SetupError::Governor)?;
        }
        *roles = next;
        if let Some(listing) = catalog.listing() {
            catalog::rederive(&listing, governor);
        }
        push_efforts(&roles, governor, catalog);
        let after = running(governor);
        Ok(ModelRoles::ALL
            .into_iter()
            .filter_map(|role| {
                let (before, after) = (before.get(&role).cloned(), after.get(&role).cloned());
                (before != after).then_some(RoleSwap {
                    role,
                    before,
                    after,
                })
            })
            .collect())
    }
}

/// Effective level per routed role against `listing` (see `ModelStack::efforts`).
pub(super) fn efforts(
    roles: &ModelRoles,
    listing: Option<&[ListedModel]>,
) -> BTreeMap<Role, EffortStatus> {
    let listed = |alias: &str| listing.is_some_and(|models| models.iter().any(|m| m.id == alias));
    effort::resolve(
        roles,
        |alias| catalog::published(listing?, alias).cloned(),
        |alias| variant_of(alias, listed).map(|variant| variant.effort),
    )
}

fn push_efforts(roles: &ModelRoles, governor: &Governor, catalog: &CatalogState) {
    let efforts = efforts(roles, catalog.listing().as_deref());
    for role in ModelRoles::ALL {
        governor.set_effort(role, efforts.get(&role).map(|status| status.effort));
    }
}

pub(super) fn running(governor: &Governor) -> BTreeMap<Role, RunningRole> {
    ModelRoles::ALL
        .into_iter()
        .filter_map(|role| {
            let route = governor.route(role)?;
            Some((
                role,
                RunningRole {
                    alias: route.alias,
                    effort: route.effort,
                },
            ))
        })
        .collect()
}
