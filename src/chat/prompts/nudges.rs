//! Neutral self-service lead-ins used when neither profile nor bundle has a pool.
//! Code appends the action text and link; these never carry either.

use crate::chat::persona::{NudgeMood, NudgePurpose};

const SELF_SERVICE_PLAYFUL: &[&str] = &[
    "You can change this one yourself.",
    "This one is yours to adjust.",
    "No need to wait for anyone, you can update it directly.",
];
const SELF_SERVICE_GENTLE: &[&str] = &[
    "That didn't go through. You can sort it out here.",
    "Sorry, that didn't work. You can fix it yourself here.",
    "No worries, you can adjust it directly.",
];
const REQUEST_FORM_PLAYFUL: &[&str] = &[
    "Want this changed? Send a request.",
    "This one needs an admin's okay.",
    "You can ask for this change.",
];
const REQUEST_FORM_GENTLE: &[&str] = &[
    "That didn't go through. You can ask for the change instead.",
    "Sorry about that. An admin can help with this one.",
    "No worries, you can send a request for it.",
];

pub fn builtin_nudges(purpose: NudgePurpose, mood: NudgeMood) -> &'static [&'static str] {
    match (purpose, mood) {
        (NudgePurpose::SelfService, NudgeMood::Playful) => SELF_SERVICE_PLAYFUL,
        (NudgePurpose::SelfService, NudgeMood::Gentle) => SELF_SERVICE_GENTLE,
        (NudgePurpose::RequestForm, NudgeMood::Playful) => REQUEST_FORM_PLAYFUL,
        (NudgePurpose::RequestForm, NudgeMood::Gentle) => REQUEST_FORM_GENTLE,
    }
}
