use std::{collections::VecDeque, sync::Mutex, time::Duration};

use super::{
    ChatRequest, CompletionFuture, CompletionResponse, LlmProvider, ProviderFailure,
    ProviderFailureKind,
};

#[derive(Clone, Debug)]
pub enum FakeAction {
    Response(CompletionResponse),
    Transient,
    Permanent,
    Authentication,
    Malformed,
    Delayed {
        delay: Duration,
        action: Box<FakeAction>,
    },
}

pub struct FakeProvider {
    actions: Mutex<VecDeque<FakeAction>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl FakeProvider {
    pub fn new(actions: impl IntoIterator<Item = FakeAction>) -> Self {
        Self {
            actions: Mutex::new(actions.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests
            .lock()
            .expect("fake provider request lock poisoned")
            .clone()
    }
}

impl LlmProvider for FakeProvider {
    fn complete(&self, request: &ChatRequest) -> CompletionFuture<'_> {
        self.requests
            .lock()
            .expect("fake provider request lock poisoned")
            .push(request.clone());
        let action = self
            .actions
            .lock()
            .expect("fake provider action lock poisoned")
            .pop_front()
            .unwrap_or(FakeAction::Permanent);
        Box::pin(async move { run(action).await })
    }
}

async fn run(action: FakeAction) -> Result<CompletionResponse, ProviderFailure> {
    match action {
        FakeAction::Response(response) => Ok(response),
        FakeAction::Transient => Err(failure(ProviderFailureKind::Transient, "transient")),
        FakeAction::Permanent => Err(failure(ProviderFailureKind::Permanent, "permanent")),
        FakeAction::Authentication => Err(failure(
            ProviderFailureKind::Authentication,
            "authentication",
        )),
        FakeAction::Malformed => Err(failure(ProviderFailureKind::InvalidOutput, "malformed")),
        FakeAction::Delayed { delay, action } => {
            tokio::time::sleep(delay).await;
            Box::pin(run(*action)).await
        }
    }
}

fn failure(kind: ProviderFailureKind, reason_code: &'static str) -> ProviderFailure {
    ProviderFailure { kind, reason_code }
}
