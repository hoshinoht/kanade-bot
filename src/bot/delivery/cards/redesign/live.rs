//! What Components V2 layouts read besides the schedule (the bot's avatar,
//! the portal origin and whether the portal is open) and which posted
//! messages are V2.
//!
//! Discord can add the V2 flag on an edit but never remove it, and a legacy
//! edit (content or embeds) of a V2 message is refused with a 400. The store
//! keeps no format per message, so [`LiveFormats`] remembers what this
//! process posted, edited, saw on a button press or read back with
//! [`DiscordTransport::message_flags`]. Unknown means legacy: callers send
//! their legacy edit and, when Discord refuses it with a 400, ask with
//! [`learn_after_refusal`] whether the message is V2.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use crate::bot::ids::parse_id;
use crate::bot::transport::{COMPONENTS_V2, DiscordTransport, Outcome, RejectionKind};

use super::v2::is_http_url;

/// The bot's avatar as an http(s) URL, read per render.
pub type AvatarSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Whether the public portal is open (`self_service.public_portal`), read
/// per render.
pub type PortalSwitch = Arc<dyn Fn() -> bool + Send + Sync>;

/// Messages remembered; the oldest are forgotten first (and are then
/// treated as unknown again, which only costs a refused edit).
const REMEMBERED: usize = 4096;

/// The V2 parts of the card kit. The default has no avatar and no portal
/// (those pieces are left out) and its own empty registry.
#[derive(Clone, Default)]
pub struct V2Kit {
    pub avatar: Option<AvatarSource>,
    /// The public portal origin (`https://host`) for "Open portal"; `None`
    /// without a public listener (the admin host is tailnet-only).
    pub portal: Option<String>,
    /// The live portal switch; `None` is closed. A closed portal's tunnel
    /// is stopped, so its origin would be a dead link.
    pub portal_open: Option<PortalSwitch>,
    pub formats: Arc<LiveFormats>,
}

impl V2Kit {
    /// The avatar, only when it is an http(s) URL.
    pub fn avatar_url(&self) -> Option<String> {
        self.avatar
            .as_ref()
            .and_then(|avatar| avatar())
            .filter(|url| is_http_url(url))
    }

    /// The portal origin, only when it is an http(s) URL and the portal is
    /// open now.
    pub fn portal_url(&self) -> Option<&str> {
        self.portal
            .as_deref()
            .filter(|url| is_http_url(url))
            .filter(|_| self.portal_open.as_ref().is_some_and(|open| open()))
    }
}

impl std::fmt::Debug for V2Kit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("V2Kit")
            .field("avatar", &self.avatar.is_some())
            .field("portal", &self.portal)
            .field("portal_open", &self.portal_open.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct Formats {
    v2: HashMap<String, bool>,
    order: VecDeque<String>,
    /// Keys a log-once note was already written for (message ids, digest
    /// budget weeks), oldest first; capped like `order`.
    noted: BTreeSet<String>,
    noted_order: VecDeque<String>,
}

/// Which posted messages carry the Components V2 flag, as far as this
/// process knows.
#[derive(Debug, Default)]
pub struct LiveFormats(Mutex<Formats>);

impl LiveFormats {
    /// `Some(true)` for a V2 message, `Some(false)` for a legacy one.
    pub fn get(&self, message_id: &str) -> Option<bool> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .v2
            .get(message_id)
            .copied()
    }

    pub fn is_v2(&self, message_id: &str) -> bool {
        self.get(message_id) == Some(true)
    }

    pub fn record(&self, message_id: &str, v2: bool) {
        let mut formats = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if formats.v2.insert(message_id.to_owned(), v2).is_none() {
            formats.order.push_back(message_id.to_owned());
        }
        while formats.order.len() > REMEMBERED {
            if let Some(old) = formats.order.pop_front() {
                formats.v2.remove(&old);
            }
        }
    }

    /// `true` the first time it is asked for `key`: log once. Bounded on its
    /// own (some keys are never recorded), forgetting the oldest first.
    pub fn first_note(&self, key: &str) -> bool {
        let mut formats = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !formats.noted.insert(key.to_owned()) {
            return false;
        }
        formats.noted_order.push_back(key.to_owned());
        while formats.noted_order.len() > REMEMBERED {
            if let Some(old) = formats.noted_order.pop_front() {
                formats.noted.remove(&old);
            }
        }
        true
    }
}

/// After Discord refused a legacy edit of `message_id`: whether that is
/// because the message is V2 (read back and recorded). Other refusals and
/// failed reads say `false`.
pub async fn learn_after_refusal<T: DiscordTransport>(
    formats: &LiveFormats,
    transport: &T,
    channel_id: &str,
    message_id: &str,
    refused: &Outcome<()>,
) -> bool {
    if !matches!(
        refused,
        Outcome::DefinitelyRejected(RejectionKind::Http { status: 400, .. })
    ) {
        return false;
    }
    known_or_fetched(formats, transport, channel_id, message_id)
        .await
        .unwrap_or(false)
}

/// Whether `message_id` is V2: the registry, else read back (and recorded).
/// `None` when it cannot be told.
pub async fn known_or_fetched<T: DiscordTransport>(
    formats: &LiveFormats,
    transport: &T,
    channel_id: &str,
    message_id: &str,
) -> Option<bool> {
    if let Some(v2) = formats.get(message_id) {
        return Some(v2);
    }
    let (channel, message) = (parse_id(channel_id)?, parse_id(message_id)?);
    match transport.message_flags(channel, message).await {
        Outcome::Delivered(flags) => {
            let v2 = flags.contains(COMPONENTS_V2);
            formats.record(message_id, v2);
            Some(v2)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_and_its_notes_stay_bounded() {
        let formats = LiveFormats::default();
        assert_eq!(formats.get("1"), None);
        formats.record("1", true);
        formats.record("2", false);
        assert!(formats.is_v2("1"));
        assert_eq!(formats.get("2"), Some(false));
        for id in 3..=(REMEMBERED + 1) {
            formats.record(&id.to_string(), false);
        }
        assert_eq!(formats.get("1"), None, "the oldest message is forgotten");

        // Notes are bounded on their own: keys that are never recorded
        // (digest budget weeks) are forgotten oldest first too.
        assert!(formats.first_note("digest-budget:0"));
        assert!(!formats.first_note("digest-budget:0"));
        for week in 1..=REMEMBERED {
            assert!(formats.first_note(&format!("digest-budget:{week}")));
        }
        assert!(
            formats.first_note("digest-budget:0"),
            "the oldest note is forgotten"
        );
        let held = formats.0.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(held.noted.len(), REMEMBERED);
        assert_eq!(held.noted_order.len(), REMEMBERED);
    }

    #[test]
    fn only_http_avatars_and_portals_are_used() {
        let kit = V2Kit {
            avatar: Some(Arc::new(|| Some("attachment://a.png".to_owned()))),
            portal: Some("kanade.example".into()),
            portal_open: Some(Arc::new(|| true)),
            formats: Arc::default(),
        };
        assert_eq!(kit.avatar_url(), None);
        assert_eq!(kit.portal_url(), None);
        let kit = V2Kit {
            avatar: Some(Arc::new(|| Some("https://cdn/a.png".to_owned()))),
            portal: Some("https://kanade.example".into()),
            portal_open: Some(Arc::new(|| true)),
            formats: Arc::default(),
        };
        assert_eq!(kit.avatar_url().as_deref(), Some("https://cdn/a.png"));
        assert_eq!(kit.portal_url(), Some("https://kanade.example"));
    }
}
