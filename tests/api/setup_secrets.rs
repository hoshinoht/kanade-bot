//! Admin Setup's saved secrets (`/api/admin/setup/secrets…`, `…/restart`):
//! presence only, write-only saves behind a fresh sign-in, the host file
//! winning, checks that keep nothing, History rows without values, and
//! "Restart now" refusing without a supervisor or with a failing next start.
//! Answers are validated against `setup.json`.

use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use chrono::TimeDelta;
use kanade::{
    api::{
        dto::setup::{SecretCheck, SecretCheckResult, SecretName},
        listeners::Site,
        state::{RestartPort, SecretChecks, SecretDesk},
    },
    runtime::secrets::{self, SecretFile},
};
use kanade_domain::settings::{SettingsChangeQuery, SettingsStore};
use serde_json::Value;

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, Fixture, PUBLIC_HOST, request, send, spawn},
};

const LIST: &str = "/api/admin/setup/secrets";
const RESTART: &str = "/api/admin/setup/restart";
const VIEW: &str = "setup.json#/$defs/SetupSecrets";
const CHECK: &str = "setup.json#/$defs/SecretCheck";
const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const VALUE: &str = "saved-secret-value-sentinel";

/// A host directory and a saved one under a fresh temp directory.
struct Dirs(PathBuf);

impl Dirs {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("kanade-setup-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("host")).unwrap();
        Self(root)
    }

    fn files(&self) -> Vec<SecretFile> {
        let values = BTreeMap::from([
            (
                secrets::DIR_KEY.to_owned(),
                self.0.join("host").display().to_string(),
            ),
            (
                secrets::SAVED_DIR_KEY.to_owned(),
                self.0.join("saved").display().to_string(),
            ),
        ]);
        secrets::SAVEABLE
            .iter()
            .map(|name| SecretFile::named(&values, name))
            .collect()
    }

    fn host(&self, name: &str) -> PathBuf {
        self.0.join("host").join(name)
    }

    fn saved(&self, name: &str) -> PathBuf {
        self.0.join("saved").join(name)
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Accepts `good-*` candidates (a bot named Kanade, intents on), refuses the
/// rest; counts calls.
#[derive(Default)]
struct Checks(AtomicUsize);

impl SecretChecks for Checks {
    fn check<'a>(
        &'a self,
        name: SecretName,
        value: &'a str,
    ) -> Pin<Box<dyn Future<Output = SecretCheck> + Send + 'a>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            match name {
                SecretName::DiscordToken if value.starts_with("good-") => SecretCheck {
                    bot_name: Some("Kanade".into()),
                    missing_intents: Some(Vec::new()),
                    ..SecretCheck::new(SecretCheckResult::Ok)
                },
                SecretName::DiscordToken => SecretCheck::new(SecretCheckResult::Refused),
                _ => SecretCheck::new(SecretCheckResult::NotChecked),
            }
        })
    }
}

struct Setup {
    reads: Reads,
    dirs: Dirs,
    checks: Arc<Checks>,
}

async fn setup(restart: Option<RestartPort>) -> Setup {
    let dirs = Dirs::new();
    let checks = Arc::new(Checks::default());
    let desk = Arc::new(SecretDesk::new(
        dirs.files(),
        checks.clone() as Arc<dyn SecretChecks>,
        restart,
    ));
    let reads = Reads::build(|build| build.setup_secrets = Some(desk)).await;
    Setup {
        reads,
        dirs,
        checks,
    }
}

async fn call(
    reads: &Reads,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> crate::support::Reply {
    send(
        reads.admin,
        method,
        ADMIN_HOST,
        path,
        &[
            ORIGIN,
            ("Cookie", &reads.cookie),
            ("X-Kanade-CSRF", &reads.csrf),
        ],
        body,
    )
    .await
}

fn candidate(value: &str) -> String {
    serde_json::json!({ "value": value }).to_string()
}

fn source<'a>(view: &'a Value, name: &str) -> &'a Value {
    &view["secrets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|secret| secret["name"] == name)
        .unwrap_or_else(|| panic!("{name} in {view}"))["source"]
}

#[tokio::test]
async fn a_saved_secret_is_presence_only_and_recorded_without_its_value() {
    let Setup { reads, dirs, .. } = setup(None).await;
    let view = reads.read(LIST, VIEW).await;
    let names: Vec<&str> = view["secrets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|secret| secret["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, secrets::SAVEABLE);
    assert!(
        view["secrets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["source"] == "absent")
    );
    assert_eq!(
        (
            view["restart_pending"].as_bool(),
            view["restart_available"].as_bool()
        ),
        (Some(false), Some(false))
    );

    let reply = call(
        &reads,
        "PUT",
        "/api/admin/setup/secrets/model_api_key",
        Some(&candidate(VALUE)),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let view = reply.json();
    assert_valid(VIEW, "after save", &view);
    assert!(!reply.text().contains(VALUE));
    assert_eq!(source(&view, "model_api_key"), "saved");
    assert_eq!(view["restart_pending"], true);
    assert_eq!(
        std::fs::read_to_string(dirs.saved("model_api_key")).unwrap(),
        format!("{VALUE}\n")
    );

    // Saving again replaces it; History says saved, then replaced, never the value.
    let reply = call(
        &reads,
        "PUT",
        "/api/admin/setup/secrets/model_api_key",
        Some(&candidate("second")),
    )
    .await;
    assert_eq!(reply.status, 200);
    let changes: Vec<_> = reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.section == "secrets")
        .collect();
    let sides: Vec<(String, String)> = changes
        .iter()
        .map(|change| {
            let row = &change.values["secret.model_api_key"];
            (row.from.clone(), row.to.clone())
        })
        .collect();
    assert_eq!(
        sides,
        [
            ("saved".into(), "replaced".into()),
            ("not set".into(), "saved".into())
        ],
        "newest first"
    );
    assert!(
        changes
            .iter()
            .all(|change| !format!("{change:?}").contains(VALUE))
    );
}

#[tokio::test]
async fn a_host_file_wins_and_cannot_be_replaced() {
    let Setup { reads, dirs, .. } = setup(None).await;
    std::fs::write(dirs.host("discord_token"), "host-token\n").unwrap();
    let view = reads.read(LIST, VIEW).await;
    assert_eq!(source(&view, "discord_token"), "host");
    let reply = call(
        &reads,
        "PUT",
        "/api/admin/setup/secrets/discord_token",
        Some(&candidate(VALUE)),
    )
    .await;
    assert_eq!((reply.status, reply.api_error()), (409, "on_host".into()));
    assert!(!dirs.saved("discord_token").exists());
}

#[tokio::test]
async fn bad_values_unknown_names_and_stale_sign_ins_are_refused() {
    let Setup {
        mut reads, dirs, ..
    } = setup(None).await;
    for bad in ["", "two\nlines", &"x".repeat(4097)] {
        let reply = call(
            &reads,
            "PUT",
            "/api/admin/setup/secrets/nexon_api_key",
            Some(&candidate(bad)),
        )
        .await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (422, "invalid".into()),
            "{bad:?}"
        );
    }
    for name in [
        "admin_token",
        "edge_secret",
        "model_ca.pem",
        "../discord_token",
    ] {
        let reply = call(
            &reads,
            "PUT",
            &format!("/api/admin/setup/secrets/{name}"),
            Some(&candidate(VALUE)),
        )
        .await;
        assert_eq!(reply.status, 404, "{name}");
    }
    let reply = call(
        &reads,
        "PUT",
        "/api/admin/setup/secrets/nexon_api_key",
        Some(r#"{"value":"x","extra":1}"#),
    )
    .await;
    assert_eq!(reply.status, 400);
    // Sixteen minutes after sign-in: still signed in, no longer fresh.
    reads.restart(TimeDelta::minutes(16), None).await;
    let reply = call(
        &reads,
        "PUT",
        "/api/admin/setup/secrets/nexon_api_key",
        Some(&candidate(VALUE)),
    )
    .await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (401, "admin_reauth_required".into())
    );
    assert_eq!(reads.read(LIST, VIEW).await["restart_pending"], false);
    assert!(!dirs.saved("nexon_api_key").exists());
}

#[tokio::test]
async fn a_check_answers_without_keeping_the_candidate() {
    let Setup {
        reads,
        dirs,
        checks,
    } = setup(None).await;
    let reply = call(
        &reads,
        "POST",
        "/api/admin/setup/secrets/discord_token/check",
        Some(&candidate("good-token")),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let check = reply.json();
    assert_valid(CHECK, "accepted", &check);
    assert_eq!(
        (check["result"].as_str(), check["bot_name"].as_str()),
        (Some("ok"), Some("Kanade"))
    );
    let reply = call(
        &reads,
        "POST",
        "/api/admin/setup/secrets/discord_token/check",
        Some(&candidate("stolen")),
    )
    .await;
    assert_eq!(reply.json()["result"], "refused");
    assert_eq!(checks.0.load(Ordering::SeqCst), 2);
    assert!(!dirs.0.join("saved").exists(), "a check writes nothing");
    assert_eq!(reads.read(LIST, VIEW).await["restart_pending"], false);
}

#[tokio::test]
async fn restart_needs_a_supervisor_and_a_next_start_that_works() {
    let Setup { reads, .. } = setup(None).await;
    let reply = call(&reads, "POST", RESTART, None).await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (409, "not_supervised".into())
    );

    let failing: RestartPort = Arc::new(|| Err("secrets file `discord_token` is missing".into()));
    let Setup { reads, .. } = setup(Some(failing)).await;
    assert_eq!(reads.read(LIST, VIEW).await["restart_available"], true);
    let reply = call(&reads, "POST", RESTART, None).await;
    assert_eq!(reply.status, 422);
    assert_eq!(
        reply.json()["message"],
        "secrets file `discord_token` is missing"
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let working: RestartPort = Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let Setup { reads, .. } = setup(Some(working)).await;
    let reply = call(&reads, "POST", RESTART, None).await;
    assert_eq!(reply.status, 202, "{}", reply.text());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn without_a_saved_directory_nothing_is_saveable() {
    let reads = Reads::new().await;
    let reply = call(&reads, "GET", LIST, None).await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (409, "not_saveable".into())
    );
}

#[tokio::test]
async fn the_public_origin_never_mounts_setup_secrets() {
    let Setup { reads, .. } = setup(None).await;
    let fixture = Fixture::new();
    let mut site = Site::public(&fixture.http()).unwrap();
    site.state = reads.site.state.clone();
    let public = spawn(site).await;
    for path in [LIST, RESTART] {
        let reply = request(
            public,
            "GET",
            PUBLIC_HOST,
            path,
            &[("Cookie", &reads.cookie)],
        )
        .await;
        assert_eq!(reply.status, 404, "{path}");
    }
}
