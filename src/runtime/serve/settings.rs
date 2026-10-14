//! Runtime settings at startup: stored rows over the environment seeds.

use crate::{
    chat::persona::PersonaId,
    domain::settings::{RuntimeSettings, SettingsError, keys, load_settings},
    infrastructure::store::SqliteStore,
    runtime::{config::SettingSeeds, error::Error},
};

/// Code defaults with the environment applied; unset seeds keep the default.
pub fn seed(seeds: &SettingSeeds) -> RuntimeSettings {
    let ids = |list: &[u64]| list.iter().map(u64::to_string).collect::<Vec<_>>();
    let mut settings = RuntimeSettings::default();
    if let Some(id) = seeds.post_channel_id {
        settings.posting.channel_id = Some(id.to_string());
    }
    if !seeds.watch_channel_ids.is_empty() {
        settings.watching.channel_ids = ids(&seeds.watch_channel_ids);
    }
    if !seeds.watch_category_ids.is_empty() {
        settings.watching.category_ids = ids(&seeds.watch_category_ids);
    }
    if !seeds.chat_category_ids.is_empty() {
        settings.chatbot.category_ids = ids(&seeds.chat_category_ids);
    }
    if let Some(enabled) = seeds.extraction_enabled {
        settings.watching.extract_enabled = enabled;
    }
    if let Some(enabled) = seeds.chat_enabled {
        settings.chatbot.enabled = enabled;
    }
    if let Some(day) = seeds.reset_weekday {
        settings.schedule.reset_weekday = day;
    }
    if let Some(time) = seeds.reset_time {
        settings.schedule.reset_time = time;
    }
    if let Some(time) = seeds.day_of_ping_time {
        settings.pings.day_of_ping_time = time;
    }
    if let Some(minutes) = &seeds.countdown_minutes {
        settings.pings.countdown_minutes.clone_from(minutes);
    }
    if let Some(run_lengths) = &seeds.run_lengths {
        settings.run_lengths.clone_from(run_lengths);
    }
    settings
}

/// A malformed stored row fails startup naming its key (never its value).
pub async fn load(store: &SqliteStore, seeds: &SettingSeeds) -> Result<RuntimeSettings, Error> {
    load_settings(store, &seed(seeds))
        .await
        .map_err(|error| match error {
            SettingsError::Malformed { key, .. } => Error::Startup(format!(
                "stored setting `{key}` is malformed; correct it before starting"
            )),
            _ => Error::Startup("runtime settings could not be read".into()),
        })
}

/// The stored persona selection; `""` is none.
pub fn persona(settings: &RuntimeSettings) -> Result<Option<PersonaId>, Error> {
    match settings.persona.active.as_str() {
        "" => Ok(None),
        id => PersonaId::parse(id).map(Some).map_err(|_| {
            Error::Startup(format!(
                "stored setting `{}` is not a persona id; correct it before starting",
                keys::PERSONA
            ))
        }),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        domain::settings::{RunLengthOverride, RunLengths},
        runtime::config::SettingSeeds,
    };

    use super::seed;

    #[test]
    fn run_lengths_seed_replaces_the_code_default_before_any_saved_row() {
        let seeds = SettingSeeds {
            run_lengths: Some(RunLengths {
                default_minutes: 20,
                overrides: vec![RunLengthOverride {
                    boss: "BM".into(),
                    difficulty: "h".into(),
                    minutes: 90,
                }],
            }),
            ..Default::default()
        };
        let settings = seed(&seeds);
        assert_eq!(settings.run_lengths.default_minutes, 20);
        assert_eq!(settings.run_lengths.overrides[0].minutes, 90);
    }
}
