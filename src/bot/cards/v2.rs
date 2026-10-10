//! The redesigned proposal card as Components V2 (user decision 2026-10-07
//! 19:55): one container in the card's colour with the question, the
//! changes, who has not answered and the footer, and an Apply / Reject row
//! while it is open. Its buttons replace the bot-seeded ✅/❌; a decided
//! card keeps one disabled button saying who decided. Cards ping nobody.

use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::ButtonStyle;

use super::styled::StyledCard;
use crate::bot::delivery::cards::redesign::{
    ButtonId, CARD_CLOSED, action_row, button, container, subtext, text, within_budget,
};

pub const APPLY: &str = "Apply";
pub const REJECT: &str = "Reject";

/// The changes' fields as text: an inline field is one `**Name** value`
/// line, a wide one (a multi-change card's heading) starts a new display.
fn field_texts(card: &StyledCard) -> Vec<String> {
    let mut texts: Vec<String> = Vec::new();
    for field in &card.fields {
        if !field.inline || texts.is_empty() {
            texts.push(String::new());
        }
        let current = texts.last_mut().expect("pushed above");
        if !current.is_empty() {
            current.push('\n');
        }
        if field.inline {
            current.push_str(&format!("**{}** {}", field.name, field.value));
        } else {
            current.push_str(&format!("**{}**\n{}", field.name, field.value));
        }
    }
    texts
}

/// The V2 layout of `card`, its buttons answering `proposal_id` (the
/// first proposal on the message; a press answers every proposal on it, as
/// a reaction does). `None` when it would not fit Discord's budget or the
/// id cannot be carried by a button.
pub fn card_components(card: &StyledCard, proposal_id: &str) -> Option<Vec<Component>> {
    ButtonId::parse(&ButtonId::CardApply(proposal_id.to_owned()).custom_id())?;
    let mut head = vec![card.title.clone()];
    head.extend(card.lines.iter().cloned());
    let mut children = vec![text(head.join("\n"))];
    children.extend(field_texts(card).into_iter().map(text));
    if let Some(description) = &card.description {
        children.push(text(description.clone()));
    }
    let row = match &card.decided {
        None => vec![
            button(
                ButtonStyle::Success,
                APPLY,
                ButtonId::CardApply(proposal_id.to_owned()).custom_id(),
                false,
            ),
            button(
                ButtonStyle::Danger,
                REJECT,
                ButtonId::CardReject(proposal_id.to_owned()).custom_id(),
                false,
            ),
        ],
        Some(label) => vec![button(
            ButtonStyle::Secondary,
            label,
            CARD_CLOSED.to_owned(),
            true,
        )],
    };
    children.push(action_row(row));
    if !card.footer_v2.is_empty() {
        children.push(text(subtext(&card.footer_v2)));
    }
    let layout = vec![container(card.colour, children)];
    within_budget(&layout).then_some(layout)
}
