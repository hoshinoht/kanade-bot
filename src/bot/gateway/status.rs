//! The gateway connection state, shared with health.

use std::sync::{Arc, Mutex, PoisonError};
#[cfg(test)]
use std::{future::Future, pin::Pin};

use tokio::sync::watch;
use twilight_gateway::Event;
use twilight_model::id::{Id, marker::UserMarker};

/// Where the session stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    /// Before the first `READY`, or reconnecting after a fresh identify.
    Connecting,
    /// `READY` or `RESUMED` arrived since the last close.
    Ready,
    /// A close frame arrived or a reconnect failed; Twilight is reconnecting.
    Disconnected,
    /// A fatal close ended the session for good (serve does not reconnect).
    Closed,
}

impl Connection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Disconnected => "disconnected",
            Self::Closed => "closed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Readiness {
    #[default]
    Pending,
    Fresh,
    Resumed,
}

#[derive(Debug)]
struct State {
    connection: Connection,
    generation: u64,
    admission_epoch: u64,
    next_operation_id: u64,
    active_operation: Option<ActiveOperation>,
    readiness: Readiness,
    guild_generation: Option<u64>,
    roster_generation: Option<u64>,
    self_id: Option<Id<UserMarker>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveOperation {
    id: u64,
    epoch: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            connection: Connection::Connecting,
            generation: 0,
            admission_epoch: 0,
            next_operation_id: 1,
            active_operation: None,
            readiness: Readiness::Pending,
            guild_generation: None,
            roster_generation: None,
            self_id: None,
        }
    }
}

/// A cloneable handle the runner updates from raw gateway events.
#[derive(Clone, Debug)]
pub struct ConnectionStatus(Arc<Mutex<State>>, watch::Sender<Option<ReplayReady>>);

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReplayReady {
    pub generation: u64,
    pub self_id: Id<UserMarker>,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self(Arc::default(), watch::Sender::new(None))
    }
}

#[cfg(test)]
type ClaimResultHook = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[must_use = "eligibility must be atomically admitted before a side effect"]
pub(crate) struct DeliveryEligibility {
    status: Option<ConnectionStatus>,
    epoch: u64,
    generation: u64,
    readiness: Readiness,
    validator: Option<Box<dyn FnOnce() -> bool + Send>>,
}

impl DeliveryEligibility {
    pub(crate) fn unguarded() -> Self {
        Self {
            status: None,
            epoch: 0,
            generation: 0,
            readiness: Readiness::Pending,
            validator: None,
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn checked(validator: impl FnOnce() -> bool + Send + 'static) -> Self {
        Self {
            status: None,
            epoch: 0,
            generation: 0,
            readiness: Readiness::Pending,
            validator: Some(Box::new(validator)),
        }
    }

    /// Atomically claim the single operation slot for the snapshotted epoch.
    pub(crate) fn admit(self) -> Option<DeliveryOperation> {
        if let Some(validator) = self.validator
            && !validator()
        {
            return None;
        }
        let Some(status) = self.status else {
            return Some(DeliveryOperation::unguarded());
        };
        let mut state = status.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.admission_epoch != self.epoch
            || state.generation != self.generation
            || state.readiness != self.readiness
            || !delivery_allowed(&state)
            || state.active_operation.is_some()
        {
            return None;
        }
        let id = state.next_operation_id;
        state.next_operation_id = state.next_operation_id.wrapping_add(1);
        let owner = ActiveOperation {
            id,
            epoch: state.admission_epoch,
        };
        state.active_operation = Some(owner);
        Some(DeliveryOperation {
            status: Some(status.clone()),
            owner: Some(owner),
            begun: false,
            settled: false,
            #[cfg(test)]
            claim_result_hook: None,
        })
    }
}

#[must_use = "an admitted operation must be settled after its effect"]
pub(crate) struct DeliveryOperation {
    status: Option<ConnectionStatus>,
    owner: Option<ActiveOperation>,
    begun: bool,
    settled: bool,
    #[cfg(test)]
    claim_result_hook: Option<ClaimResultHook>,
}

impl DeliveryOperation {
    fn unguarded() -> Self {
        Self {
            status: None,
            owner: None,
            begun: false,
            settled: false,
            #[cfg(test)]
            claim_result_hook: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_claim_result_hook(
        &mut self,
        hook: impl Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    ) {
        self.claim_result_hook = Some(Arc::new(hook));
    }

    #[cfg(test)]
    pub(crate) async fn after_claim_result(&mut self) {
        if let Some(hook) = self.claim_result_hook.take() {
            hook().await;
        }
    }

    /// Begin an already-owned operation. Close/READY does not revoke ownership.
    pub(crate) fn begin(&mut self) -> bool {
        if self.settled || self.begun {
            return false;
        }
        let (Some(status), Some(owner)) = (&self.status, self.owner) else {
            self.begun = true;
            return true;
        };
        let state = status.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.active_operation != Some(owner) {
            self.settled = true;
            return false;
        }
        self.begun = true;
        true
    }

    /// Release the one-operation slot after the effect and its outcome settle.
    pub(crate) fn settle(mut self) {
        self.release_owner();
        self.settled = true;
    }

    fn release_owner(&mut self) {
        let (Some(status), Some(owner)) = (&self.status, self.owner) else {
            return;
        };
        let mut state = status.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.active_operation == Some(owner) {
            state.active_operation = None;
        }
    }
}

impl Drop for DeliveryOperation {
    fn drop(&mut self) {
        if !self.settled {
            self.release_owner();
        }
    }
}

impl ConnectionStatus {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn mark_ready_for_test(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.connection = Connection::Ready;
        state.generation = 1;
        state.admission_epoch = 1;
        state.readiness = Readiness::Resumed;
    }

    pub fn get(&self) -> Connection {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .connection
    }

    /// New Discord sends remain paused until the current session is usable.
    pub fn delivery_claims_allowed(&self) -> bool {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        delivery_allowed(&state)
    }

    /// Snapshot eligibility; [`DeliveryEligibility::admit`] rechecks the epoch
    /// and atomically assigns the unique operation owner.
    pub(crate) fn delivery_eligibility(&self) -> Option<DeliveryEligibility> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (delivery_allowed(&state) && state.active_operation.is_none()).then(|| {
            DeliveryEligibility {
                status: Some(self.clone()),
                epoch: state.admission_epoch,
                generation: state.generation,
                readiness: state.readiness,
                validator: None,
            }
        })
    }

    /// The session ended for good; later events cannot revive it.
    pub fn closed(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.connection != Connection::Closed {
            state.connection = Connection::Closed;
            state.admission_epoch = state.admission_epoch.wrapping_add(1);
        }
    }

    /// Only a ready session drops to disconnected; connecting stays so.
    pub(super) fn disconnected(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.connection == Connection::Ready {
            state.connection = Connection::Disconnected;
            state.admission_epoch = state.admission_epoch.wrapping_add(1);
        }
    }

    /// Begin readiness for the current fresh IDENTIFY/READY generation.
    pub(crate) fn guild_available(&self) -> Option<u64> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.connection != Connection::Ready || state.readiness != Readiness::Fresh {
            return None;
        }
        let generation = state.generation;
        state.guild_generation = Some(generation);
        Some(generation)
    }

    /// Complete readiness only for the generation that requested reconciliation.
    pub(crate) fn roster_reconciled(&self, generation: u64) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.readiness == Readiness::Fresh
            && state.generation == generation
            && state.guild_generation == Some(generation)
            && state.roster_generation != Some(generation)
        {
            state.roster_generation = Some(generation);
            if let Some(self_id) = state.self_id {
                self.1.send_replace(Some(ReplayReady {
                    generation,
                    self_id,
                }));
            }
        }
    }

    pub(crate) fn reaction_replays(&self) -> watch::Receiver<Option<ReplayReady>> {
        self.1.subscribe()
    }

    pub(crate) fn reaction_replay_allowed(&self, generation: u64) -> bool {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.connection != Connection::Closed
            && state.generation == generation
            && state.roster_generation == Some(generation)
    }

    pub(crate) fn is_current_fresh_generation(&self, generation: u64) -> bool {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.connection == Connection::Ready
            && state.readiness == Readiness::Fresh
            && state.generation == generation
            && state.guild_generation == Some(generation)
    }

    pub(super) fn observe(&self, event: &Event) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            Event::Ready(ready) if state.connection != Connection::Closed => {
                state.connection = Connection::Ready;
                state.generation = state.generation.wrapping_add(1);
                state.admission_epoch = state.admission_epoch.wrapping_add(1);
                state.readiness = Readiness::Fresh;
                state.guild_generation = None;
                state.roster_generation = None;
                state.self_id = Some(ready.user.id);
            }
            Event::Resumed if state.connection != Connection::Closed => {
                state.connection = Connection::Ready;
                state.readiness = Readiness::Resumed;
            }
            Event::GatewayClose(_) if state.connection != Connection::Closed => {
                state.connection = Connection::Disconnected;
                state.admission_epoch = state.admission_epoch.wrapping_add(1);
            }
            _ => {}
        }
    }
}

fn delivery_allowed(state: &State) -> bool {
    state.connection == Connection::Ready
        && match state.readiness {
            Readiness::Pending => false,
            Readiness::Resumed => true,
            Readiness::Fresh => {
                state.guild_generation == Some(state.generation)
                    && state.roster_generation == Some(state.generation)
            }
        }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use twilight_gateway::Event;
    use twilight_model::gateway::payload::incoming::Ready;

    use super::{ActiveOperation, Connection, ConnectionStatus, PoisonError};

    fn ready() -> Event {
        Event::Ready(
            serde_json::from_value::<Ready>(json!({
                "application": { "id": "9", "flags": 0 },
                "guilds": [],
                "resume_gateway_url": "wss://gateway.invalid",
                "session_id": "session",
                "user": {
                    "id": "800", "username": "kanade", "discriminator": "0",
                    "avatar": null, "bot": true, "mfa_enabled": false
                },
                "v": 10
            }))
            .unwrap(),
        )
    }

    #[test]
    fn claims_wait_for_each_fresh_generation_but_resume_reopens_them() {
        let status = ConnectionStatus::new();
        status.observe(&ready());
        assert_eq!(status.get(), Connection::Ready);
        assert!(!status.delivery_claims_allowed());
        assert!(status.delivery_eligibility().is_none());

        let first = status.guild_available().unwrap();
        assert!(!status.delivery_claims_allowed());
        assert!(status.delivery_eligibility().is_none());
        status.roster_reconciled(first);
        assert!(status.delivery_claims_allowed());
        let stale = status.delivery_eligibility().unwrap();
        let mut owned = status.delivery_eligibility().unwrap().admit().unwrap();
        assert!(
            status.delivery_eligibility().is_none(),
            "only one owner at a time"
        );

        status.observe(&Event::GatewayClose(None));
        assert!(!status.delivery_claims_allowed());
        assert!(
            owned.begin(),
            "an owner admitted before close can start afterward"
        );
        status.observe(&Event::Resumed);
        assert!(status.delivery_claims_allowed());
        assert!(
            status.delivery_eligibility().is_none(),
            "resume waits for the owner"
        );
        owned.settle();
        assert!(
            stale.admit().is_none(),
            "close→RESUMED cannot revive eligibility"
        );
        let mut resumed = status.delivery_eligibility().unwrap().admit().unwrap();
        assert!(resumed.begin());
        resumed.settle();

        status.observe(&ready());
        assert!(!status.delivery_claims_allowed());
        let second = status.guild_available().unwrap();
        assert_ne!(first, second);
        status.roster_reconciled(first);
        assert!(
            !status.delivery_claims_allowed(),
            "old reconciliation cannot open a new generation"
        );
        status.roster_reconciled(second);
        assert!(status.delivery_claims_allowed());
        let stale_generation = status.delivery_eligibility().unwrap();
        status.observe(&ready());
        assert!(
            !status.delivery_claims_allowed(),
            "fresh READY requires a new reconcile"
        );
        assert!(stale_generation.admit().is_none());
        status.observe(&Event::GatewayClose(None));
        status.observe(&Event::Resumed);
        assert!(status.delivery_claims_allowed());
    }

    #[test]
    fn dropping_a_begun_operation_during_unwind_releases_its_slot() {
        let status = ConnectionStatus::new();
        status.mark_ready_for_test();

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut operation = status.delivery_eligibility().unwrap().admit().unwrap();
            assert!(operation.begin());
            panic!("cancel between begin and settle");
        }));

        assert!(unwind.is_err());
        assert!(status.delivery_eligibility().is_some());
    }

    #[test]
    fn reaction_replay_survives_close_resume_but_not_a_new_generation_or_fatal_close() {
        let status = ConnectionStatus::new();
        let mut requests = status.reaction_replays();
        status.observe(&ready());
        let generation = status.guild_available().unwrap();
        assert!(!status.reaction_replay_allowed(generation));
        status.roster_reconciled(generation);
        assert_eq!(requests.borrow_and_update().unwrap().generation, generation);
        status.observe(&Event::GatewayClose(None));
        assert!(!status.delivery_claims_allowed());
        assert!(
            status.reaction_replay_allowed(generation),
            "HTTP does not need the gateway"
        );
        status.observe(&Event::Resumed);
        assert!(status.reaction_replay_allowed(generation));
        assert!(
            !requests.has_changed().unwrap(),
            "resume never creates another pass"
        );
        status.observe(&ready());
        assert!(!status.reaction_replay_allowed(generation));
        let next = status.guild_available().unwrap();
        status.roster_reconciled(next);
        assert!(status.reaction_replay_allowed(next));
        status.closed();
        assert!(!status.reaction_replay_allowed(next));
    }

    #[test]
    fn dropping_a_stale_begun_operation_preserves_the_current_owner() {
        let status = ConnectionStatus::new();
        status.mark_ready_for_test();
        let mut stale = status.delivery_eligibility().unwrap().admit().unwrap();
        assert!(stale.begin());
        let current = ActiveOperation {
            id: stale.owner.unwrap().id.wrapping_add(1),
            epoch: stale.owner.unwrap().epoch,
        };
        status
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active_operation = Some(current);

        drop(stale);

        assert_eq!(
            status
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .active_operation,
            Some(current)
        );
    }
}
