//! What each build (cargo features `bot` and `web`) lets `serve` run:
//! `runtime.components` may name only compiled components, unset runs all
//! of them, and the admin CLI client exists only with `web`.

use std::collections::BTreeMap;

use kanade::{cli, runtime::config::Components};

fn components(list: Option<&str>) -> Result<Components, String> {
    let values = list
        .map(|list| BTreeMap::from([("KANADE_COMPONENTS".to_owned(), list.to_owned())]))
        .unwrap_or_default();
    Components::from_mapping(&values).map_err(|error| error.to_string())
}

fn parse(arguments: &[&str]) -> Result<cli::Command, String> {
    cli::parse(arguments.iter().map(|&argument| argument.to_owned()))
        .map_err(|error| error.to_string())
}

#[test]
fn unset_runs_every_compiled_component() {
    assert_eq!(
        components(None),
        Ok(Components {
            bot: cfg!(feature = "bot"),
            web: cfg!(feature = "web"),
        })
    );
}

#[cfg(all(feature = "bot", feature = "web"))]
#[test]
fn a_full_build_runs_either_component_alone() {
    assert_eq!(
        components(Some("bot")),
        Ok(Components {
            bot: true,
            web: false
        })
    );
    assert_eq!(
        components(Some("web")),
        Ok(Components {
            bot: false,
            web: true
        })
    );
}

#[cfg(not(feature = "web"))]
#[test]
fn a_bot_only_build_refuses_the_web_component() {
    let refusal = "KANADE_COMPONENTS: this build has no `web` component (it has `bot`)";
    assert_eq!(components(Some("web")), Err(refusal.to_owned()));
    assert_eq!(components(Some("bot,web")), Err(refusal.to_owned()));
    assert_eq!(
        components(Some("bot")),
        Ok(Components {
            bot: true,
            web: false
        })
    );
}

#[cfg(not(feature = "bot"))]
#[test]
fn a_web_only_build_refuses_the_bot_component() {
    let refusal = "KANADE_COMPONENTS: this build has no `bot` component (it has `web`)";
    assert_eq!(components(Some("bot")), Err(refusal.to_owned()));
    assert_eq!(components(Some("web,bot")), Err(refusal.to_owned()));
    assert_eq!(
        components(Some("web")),
        Ok(Components {
            bot: false,
            web: true
        })
    );
}

#[cfg(not(feature = "web"))]
#[test]
fn a_bot_only_build_has_no_offline_shell_nor_admin_client() {
    assert!(components(None).unwrap().offline_allowed().is_err());
    for command in [&["portal", "status"][..], &["sessions", "list"]] {
        let refused = parse(command).unwrap_err();
        assert!(refused.starts_with("usage: kanade "), "{refused}");
        assert!(!refused.contains("portal"), "{refused}");
    }
}

#[cfg(feature = "web")]
#[test]
fn a_build_with_web_parses_the_admin_client() {
    assert!(matches!(
        parse(&["sessions", "list"]),
        Ok(cli::Command::Admin(_))
    ));
}
