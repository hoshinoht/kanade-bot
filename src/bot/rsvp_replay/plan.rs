//! Pure RSVP-replay decisions after every card snapshot has been collected.

use std::collections::BTreeSet;

use crate::domain::schedule::RsvpState;

#[derive(Clone, Debug, Default)]
pub(crate) struct CardAnswer {
    pub message_id: String,
    pub evidence: bool,
    pub complete: bool,
    pub yes: BTreeSet<String>,
    pub no: BTreeSet<String>,
    pub own_yes: bool,
    pub own_no: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDecision {
    None,
    Conflict,
    Answer(RsvpState),
}

pub(crate) fn remote(cards: &[CardAnswer], user: &str) -> RemoteDecision {
    let yes = cards
        .iter()
        .any(|card| card.complete && card.yes.contains(user));
    let no = cards
        .iter()
        .any(|card| card.complete && card.no.contains(user));
    match (yes, no) {
        (true, true) => RemoteDecision::Conflict,
        (true, false) => RemoteDecision::Answer(RsvpState::Yes),
        (false, true) => RemoteDecision::Answer(RsvpState::No),
        (false, false) => RemoteDecision::None,
    }
}

pub(crate) fn has_evidence(cards: &[CardAnswer], user: &str, state: RsvpState) -> bool {
    cards.iter().any(|card| {
        card.evidence
            && card.complete
            && match state {
                RsvpState::Yes => card.yes.contains(user),
                RsvpState::No => card.no.contains(user),
                RsvpState::Maybe => false,
            }
    })
}

pub(crate) fn evidence_complete(cards: &[CardAnswer]) -> bool {
    cards
        .iter()
        .filter(|card| card.evidence)
        .all(|card| card.complete)
}

pub(crate) fn removal_guard(cards: &[CardAnswer], user: &str, state: RsvpState) -> bool {
    cards.iter().all(|card| {
        card.complete
            && match state {
                RsvpState::Yes => card.own_yes && !card.yes.contains(user),
                RsvpState::No => card.own_no && !card.no.contains(user),
                RsvpState::Maybe => false,
            }
    })
}
