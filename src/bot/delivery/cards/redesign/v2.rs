//! Components V2 layouts (phase 2 of the redesign; only for messages that
//! ping nobody, since V2 pings arrive as empty push notifications): the
//! builders every V2 surface shares, the custom ids of the bot's buttons and
//! Discord's per-message budget. A layout over budget is never sent: the
//! caller falls back to its embed rendering.

use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::{
    ActionRow, Button, ButtonStyle, Container, Section, Separator, SeparatorSpacingSize,
    TextDisplay, Thumbnail, UnfurledMediaItem,
};

/// Components per message, nested ones (and a section's accessory) counted.
pub const MAX_COMPONENTS: usize = 40;
/// Characters across every text display of one message.
pub const MAX_TEXT_CHARS: usize = 4000;
/// One text display's length as Twilight validates it (bytes).
pub const MAX_TEXT_DISPLAY_BYTES: usize = 2000;
/// Discord's custom id and button label limits.
pub const MAX_CUSTOM_ID: usize = 100;
pub const MAX_BUTTON_LABEL: usize = 80;

/// The digest's "My runs" button: an ephemeral `/schedule scope:mine`.
pub const DIGEST_MINE: &str = "digest:mine";
/// The one disabled button a decided proposal card keeps.
pub const CARD_CLOSED: &str = "card:closed";
const CARD_APPLY: &str = "card:apply:";
const CARD_REJECT: &str = "card:reject:";

/// What one of the bot's buttons asks for, read back from its custom id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonId {
    DigestMine,
    CardApply(String),
    CardReject(String),
}

/// Proposal ids are short ASCII tokens; anything else is not ours.
fn proposal_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

impl ButtonId {
    /// Strict: exactly one of the shapes the bot builds.
    pub fn parse(custom_id: &str) -> Option<Self> {
        if custom_id == DIGEST_MINE {
            return Some(Self::DigestMine);
        }
        if let Some(id) = custom_id.strip_prefix(CARD_APPLY) {
            return proposal_id(id).then(|| Self::CardApply(id.to_owned()));
        }
        if let Some(id) = custom_id.strip_prefix(CARD_REJECT) {
            return proposal_id(id).then(|| Self::CardReject(id.to_owned()));
        }
        None
    }

    pub fn custom_id(&self) -> String {
        match self {
            Self::DigestMine => DIGEST_MINE.to_owned(),
            Self::CardApply(id) => format!("{CARD_APPLY}{id}"),
            Self::CardReject(id) => format!("{CARD_REJECT}{id}"),
        }
    }
}

/// Every component, nested ones and section accessories included.
pub fn component_count(components: &[Component]) -> usize {
    components
        .iter()
        .map(|component| {
            1 + match component {
                Component::ActionRow(row) => component_count(&row.components),
                Component::Container(container) => component_count(&container.components),
                Component::Section(section) => {
                    component_count(&section.components)
                        + component_count(std::slice::from_ref(&section.accessory))
                }
                _ => 0,
            }
        })
        .sum()
}

/// Every text display, nested ones included.
fn text_displays(components: &[Component]) -> Vec<&str> {
    let mut out = Vec::new();
    for component in components {
        match component {
            Component::TextDisplay(text) => out.push(text.content.as_str()),
            Component::Container(container) => out.extend(text_displays(&container.components)),
            Component::Section(section) => out.extend(text_displays(&section.components)),
            _ => {}
        }
    }
    out
}

/// Characters Discord counts towards [`MAX_TEXT_CHARS`].
pub fn text_chars(components: &[Component]) -> usize {
    text_displays(components)
        .iter()
        .map(|text| text.chars().count())
        .sum()
}

/// Every button's custom id, nested ones included.
fn custom_ids(components: &[Component]) -> Vec<&str> {
    let mut out = Vec::new();
    for component in components {
        match component {
            Component::Button(button) => out.extend(button.custom_id.as_deref()),
            Component::ActionRow(row) => out.extend(custom_ids(&row.components)),
            Component::Container(container) => out.extend(custom_ids(&container.components)),
            _ => {}
        }
    }
    out
}

/// Whether Discord (and Twilight's validation) would take this layout:
/// at most [`MAX_COMPONENTS`] components and [`MAX_TEXT_CHARS`] characters,
/// no empty text display or one over [`MAX_TEXT_DISPLAY_BYTES`], no custom
/// id over [`MAX_CUSTOM_ID`].
pub fn within_budget(components: &[Component]) -> bool {
    !components.is_empty()
        && component_count(components) <= MAX_COMPONENTS
        && text_chars(components) <= MAX_TEXT_CHARS
        && text_displays(components)
            .iter()
            .all(|text| !text.is_empty() && text.len() <= MAX_TEXT_DISPLAY_BYTES)
        && custom_ids(components)
            .iter()
            .all(|id| id.len() <= MAX_CUSTOM_ID)
}

/// A Discord link button only takes an http(s) URL.
pub fn is_http_url(url: &str) -> bool {
    let url = url.trim();
    url.len() > "https://".len() && (url.starts_with("https://") || url.starts_with("http://"))
}

pub fn text(content: impl Into<String>) -> Component {
    Component::TextDisplay(TextDisplay {
        id: None,
        content: content.into(),
    })
}

/// A thin divider.
pub fn separator() -> Component {
    Component::Separator(Separator {
        id: None,
        divider: Some(true),
        spacing: Some(SeparatorSpacingSize::Small),
    })
}

/// The one top-level container, its stripe in `accent`.
pub fn container(accent: u32, components: Vec<Component>) -> Component {
    Component::Container(Container {
        id: None,
        accent_color: Some(Some(accent)),
        spoiler: None,
        components,
    })
}

/// Text beside a small picture referenced by an http(s) URL (never an
/// upload: edits cannot add one).
pub fn section_with_thumbnail(texts: Vec<String>, url: &str) -> Component {
    Component::Section(Section {
        id: None,
        components: texts.into_iter().map(text).collect(),
        accessory: Box::new(Component::Thumbnail(Thumbnail {
            id: None,
            media: UnfurledMediaItem {
                url: url.to_owned(),
                proxy_url: None,
                height: None,
                width: None,
                content_type: None,
            },
            description: None,
            spoiler: None,
        })),
    })
}

pub fn action_row(buttons: Vec<Component>) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components: buttons,
    })
}

/// `label` cut to Discord's 80 characters.
fn label(text: &str) -> String {
    super::limits::clip(text, MAX_BUTTON_LABEL)
}

pub fn button(style: ButtonStyle, text: &str, custom_id: String, disabled: bool) -> Component {
    Component::Button(Button {
        id: None,
        custom_id: Some(custom_id),
        disabled,
        emoji: None,
        label: Some(label(text)),
        style,
        url: None,
        sku_id: None,
    })
}

pub fn link_button(text: &str, url: &str) -> Component {
    Component::Button(Button {
        id: None,
        custom_id: None,
        disabled: false,
        emoji: None,
        label: Some(label(text)),
        style: ButtonStyle::Link,
        url: Some(url.to_owned()),
        sku_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_ids_parse_strictly() {
        assert_eq!(ButtonId::parse("digest:mine"), Some(ButtonId::DigestMine));
        assert_eq!(
            ButtonId::parse("card:apply:p-1_a"),
            Some(ButtonId::CardApply("p-1_a".into()))
        );
        assert_eq!(
            ButtonId::parse("card:reject:abc"),
            Some(ButtonId::CardReject("abc".into()))
        );
        for bad in [
            "",
            "digest:mine ",
            "digest:mine:x",
            "card:apply:",
            "card:apply:a b",
            "card:apply:a:b",
            "card:closed",
            "card:maybe:abc",
            &format!("card:apply:{}", "x".repeat(65)),
        ] {
            assert_eq!(ButtonId::parse(bad), None, "{bad:?}");
        }
        let id = ButtonId::CardApply("abc".into());
        assert_eq!(ButtonId::parse(&id.custom_id()), Some(id));
    }

    #[test]
    fn the_budget_counts_nested_components_and_text() {
        let layout = vec![container(
            1,
            vec![
                section_with_thumbnail(vec!["a".into(), "b".into()], "https://x/y.png"),
                separator(),
                action_row(vec![link_button("Open", "https://x")]),
            ],
        )];
        // container, section, 2 texts, thumbnail, separator, row, button.
        assert_eq!(component_count(&layout), 8);
        assert_eq!(text_chars(&layout), 2);
        assert!(within_budget(&layout));
        let many = vec![container(1, (0..39).map(|_| text("x")).collect())];
        assert_eq!(component_count(&many), 40);
        assert!(within_budget(&many));
        let too_many = vec![container(1, (0..40).map(|_| text("x")).collect())];
        assert!(!within_budget(&too_many));
        let too_long = vec![container(
            1,
            vec![text("é".repeat(1500)), text("y".repeat(2600))],
        )];
        assert!(!within_budget(&too_long), "over 2000 bytes in one display");
        let wordy = vec![container(
            1,
            (0..3).map(|_| text("w".repeat(1500))).collect(),
        )];
        assert!(!within_budget(&wordy), "4500 characters in total");
        assert!(!within_budget(&[]));
    }

    #[test]
    fn only_http_urls_make_links() {
        assert!(is_http_url("https://kanade.example"));
        assert!(is_http_url("http://127.0.0.1:8080"));
        assert!(!is_http_url("attachment://avatar.png"));
        assert!(!is_http_url("https://"));
        assert!(!is_http_url(""));
    }
}
