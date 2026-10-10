//! Who may use the public portal (D3-A): in the guild, holding the bossing
//! role and not a bot, from the bot's own stored gateway view; staff get no
//! bypass. Unreadable member data is [`Eligibility::Unavailable`], which
//! callers answer with 503 and never treat as a denial.

use twilight_model::id::{Id, marker::UserMarker};

use crate::{api::auth::staff::GateFuture, domain::members::MemberStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    /// Not in the guild, no bossing role, or a bot account.
    NotEligible,
    Unavailable,
}

pub trait EligibilityGate: Send + Sync {
    fn check<'a>(&'a self, user_id: &'a str) -> GateFuture<'a, Eligibility>;
}

/// [`EligibilityGate`] over the persisted member rows. Their `has_role` is
/// the bossing-role flag the roster keeps current (departures clear it), the
/// same flag that decides who may be put on runs.
pub struct StoreEligibility<S> {
    store: S,
}

impl<S> StoreEligibility<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

impl<S: MemberStore + Send + Sync> EligibilityGate for StoreEligibility<S> {
    fn check<'a>(&'a self, user_id: &'a str) -> GateFuture<'a, Eligibility> {
        Box::pin(async move {
            let snowflake = user_id
                .parse::<u64>()
                .ok()
                .and_then(Id::<UserMarker>::new_checked);
            if snowflake.is_none() {
                return Eligibility::NotEligible;
            }
            match self.store.load_member(user_id).await {
                Ok(Some(profile)) if profile.member.has_role && !profile.member.is_bot => {
                    Eligibility::Eligible
                }
                Ok(_) => Eligibility::NotEligible,
                Err(_) => Eligibility::Unavailable,
            }
        })
    }
}
