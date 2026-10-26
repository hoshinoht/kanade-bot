//! `KANADE_COMPONENTS` (`runtime.components`): which parts of the one binary
//! `serve` runs. `bot` is the Discord side (gateway, roster, tick, chat,
//! extraction); `web` the admin app and the member portal. Each exists only
//! when the build has the cargo feature of the same name.

use std::collections::BTreeMap;

use super::{Error, non_empty};

const KEY: &str = "KANADE_COMPONENTS";

/// Every component this build has, as refusals name them.
const COMPILED: &str = match (cfg!(feature = "bot"), cfg!(feature = "web")) {
    (true, true) => "`bot` and `web`",
    (true, false) => "`bot`",
    _ => "`web`",
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Components {
    pub bot: bool,
    pub web: bool,
}

impl Default for Components {
    /// Everything this build has.
    fn default() -> Self {
        Self {
            bot: cfg!(feature = "bot"),
            web: cfg!(feature = "web"),
        }
    }
}

impl Components {
    /// Unset runs every compiled component; a list must name each at most
    /// once, and only components this build has.
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        let Some(list) = non_empty(values, KEY) else {
            return Ok(Self::default());
        };
        let mut chosen = Self {
            bot: false,
            web: false,
        };
        for name in list.split(',').map(str::trim) {
            let slot = match name {
                "bot" if cfg!(feature = "bot") => &mut chosen.bot,
                "web" if cfg!(feature = "web") => &mut chosen.web,
                "bot" | "web" => return Err(Self::missing(name)),
                "" => {
                    return Err(Error::Configuration(format!(
                        "{KEY} must list `bot`, `web` or both"
                    )));
                }
                // Never echoes the value, as for every configuration error.
                _ => {
                    return Err(Error::Configuration(format!(
                        "{KEY}: this build has no such component (it has {COMPILED})"
                    )));
                }
            };
            if std::mem::replace(slot, true) {
                return Err(Error::Configuration(format!("{KEY} lists `{name}` twice")));
            }
        }
        Ok(chosen)
    }

    /// The refusal of a component this build was compiled without.
    pub fn missing(name: &str) -> Error {
        Error::Configuration(format!(
            "{KEY}: this build has no `{name}` component (it has {COMPILED})"
        ))
    }

    /// `serve --offline` is the admin app's shell alone, so it needs `web`.
    pub fn offline_allowed(self) -> Result<(), Error> {
        if self.web {
            Ok(())
        } else {
            Err(Error::Usage(
                "serve --offline needs the `web` component (kanade.toml `runtime.components`)"
                    .into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &str) -> Result<Components, String> {
        let values = BTreeMap::from([(KEY.to_owned(), value.to_owned())]);
        Components::from_mapping(&values).map_err(|error| error.to_string())
    }

    #[test]
    fn unset_runs_everything_and_a_list_picks_components() {
        let both = Components {
            bot: true,
            web: true,
        };
        assert_eq!(
            Components::from_mapping(&BTreeMap::new()).map_err(|error| error.to_string()),
            Ok(both)
        );
        assert_eq!(parse(""), Ok(both), "Compose's blank variable is unset");
        assert_eq!(parse("bot,web"), Ok(both));
        assert_eq!(parse("web, bot"), Ok(both));
        assert_eq!(
            parse("web"),
            Ok(Components {
                bot: false,
                web: true
            })
        );
        assert_eq!(
            parse("bot"),
            Ok(Components {
                bot: true,
                web: false
            })
        );
    }

    #[test]
    fn unknown_duplicate_and_empty_entries_refuse() {
        let unknown = parse("bot,chat-sentinel").unwrap_err();
        assert_eq!(
            unknown,
            "KANADE_COMPONENTS: this build has no such component (it has `bot` and `web`)"
        );
        assert!(!unknown.contains("sentinel"));
        assert_eq!(
            parse("web,web"),
            Err("KANADE_COMPONENTS lists `web` twice".into())
        );
        assert_eq!(
            parse("bot,,web"),
            Err("KANADE_COMPONENTS must list `bot`, `web` or both".into())
        );
    }

    #[test]
    fn offline_serve_needs_web() {
        assert!(parse("web").unwrap().offline_allowed().is_ok());
        let refused = parse("bot").unwrap().offline_allowed().unwrap_err();
        assert_eq!(refused.exit_code(), 64);
    }
}
