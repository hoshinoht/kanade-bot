//! The pilot's own channel allow-list (v4 `gate.is_chat_channel` over
//! `watch.is_watched`): explicit channels, channels under an allowed
//! category, and threads of either.

/// A guild channel as the gateway knows it; a thread names its parent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: String,
    pub name: Option<String>,
    pub category_id: Option<String>,
    pub parent_id: Option<String>,
}

impl ChannelInfo {
    /// A channel known only by id, as a message always carries one.
    pub fn bare(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            ..Self::default()
        }
    }
}

/// Channels the bot can see (the gateway cache).
pub trait ChannelDirectory {
    fn channel(&self, id: &str) -> Option<ChannelInfo>;
}

/// The chatbot's `CHAT_PILOT_*` settings; never the extractor's watch list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PilotSettings {
    pub guild_id: String,
    pub channel_ids: Vec<String>,
    pub category_ids: Vec<String>,
    pub role_id: Option<String>,
}

impl PilotSettings {
    /// Both gates set: a role, and at least one channel or category.
    pub fn configured(&self) -> bool {
        self.role_id.is_some() && !(self.channel_ids.is_empty() && self.category_ids.is_empty())
    }
}

/// Python `int()` of an id; ids compare as integers in v4.
pub(crate) fn as_int(value: &str) -> Option<u64> {
    value.trim().parse().ok()
}

fn listed(value: Option<&str>, ids: &[u64]) -> bool {
    value.and_then(as_int).is_some_and(|id| ids.contains(&id))
}

fn matches(channel: &ChannelInfo, channels: &[u64], categories: &[u64]) -> bool {
    listed(Some(&channel.id), channels) || listed(channel.category_id.as_deref(), categories)
}

/// Is `channel` one of the pilot's channels? A thread counts as its parent.
pub fn is_chat_channel(
    channel: Option<&ChannelInfo>,
    directory: &(impl ChannelDirectory + ?Sized),
    settings: &PilotSettings,
) -> bool {
    let Some(channel) = channel else {
        return false;
    };
    let ids = |list: &[String]| -> Vec<u64> { list.iter().filter_map(|id| as_int(id)).collect() };
    let (channels, categories) = (ids(&settings.channel_ids), ids(&settings.category_ids));
    if channels.is_empty() && categories.is_empty() {
        return false;
    }
    if matches(channel, &channels, &categories) {
        return true;
    }
    channel
        .parent_id
        .as_deref()
        .and_then(|parent| directory.channel(parent))
        .is_some_and(|parent| matches(&parent, &channels, &categories))
}
