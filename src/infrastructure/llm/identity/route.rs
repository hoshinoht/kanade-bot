use std::fmt;

use super::codec::{CodecMode, IdentityCodec, IdentitySession, Member};
use crate::infrastructure::llm::governor::{Role, RoleRoute};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteRefused {
    /// An `external` route needs pseudonymization on; fails closed otherwise.
    ExternalWithoutPseudonymization { role: Role, alias: String },
}

impl fmt::Display for RouteRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExternalWithoutPseudonymization { role, alias } => write!(
                f,
                "role {} routes to external model {alias:?} but pseudonymization is off",
                role.as_str()
            ),
        }
    }
}

impl std::error::Error for RouteRefused {}

pub fn guard(route: &RoleRoute, codec: &dyn IdentityCodec) -> Result<(), RouteRefused> {
    if route.external && codec.mode() == CodecMode::Passthrough {
        return Err(RouteRefused::ExternalWithoutPseudonymization {
            role: route.role,
            alias: route.alias.clone(),
        });
    }
    Ok(())
}

/// Startup check over every resolved route; the first refusal wins.
pub fn check_routes(routes: &[RoleRoute], codec: &dyn IdentityCodec) -> Result<(), RouteRefused> {
    routes.iter().try_for_each(|route| guard(route, codec))
}

/// The only way ports should open a session: the guard runs on every call,
/// so a route flipped to `external` cannot bypass the startup check.
pub fn open_session(
    codec: &dyn IdentityCodec,
    route: &RoleRoute,
    roster: &[Member],
) -> Result<Box<dyn IdentitySession>, RouteRefused> {
    guard(route, codec)?;
    Ok(codec.open(roster))
}
