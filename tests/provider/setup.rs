use std::{path::PathBuf, sync::Arc};

use kanade::infrastructure::llm::{
    Effort, HttpConfigError,
    governor::{Role, XorShift},
    setup::{
        CapacityGroup, Listing, ModelRoles, ModelSetup, ModelStack, Models, RoleEffort, RoleModel,
        SetupError, build, build_with_groups,
    },
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::TlsAcceptor;

use super::stub::{Stub, gateway, kanata_models, ollama_models};

const KEY: &str = "sk-setup-secret-sentinel";
const CERT: &[u8] = include_bytes!("../fixtures/provider/tls/loopback-cert.der");

pub(super) fn role(alias: &str, effort: RoleEffort) -> RoleModel {
    RoleModel {
        alias: Some(alias.into()),
        effort,
    }
}

/// Extraction and chat on `alias`, efforts off and inherit.
pub(super) fn roles(alias: &str) -> ModelRoles {
    ModelRoles {
        extraction: role(alias, RoleEffort::Level(Effort::Off)),
        chat: role(alias, RoleEffort::Inherit),
        rewrite: RoleModel::default(),
    }
}

pub(super) fn setup(base_url: Option<String>) -> ModelSetup {
    ModelSetup {
        base_url,
        key: None,
        ca_file: None,
        roles: roles("sumi-structured"),
        permits: 2,
        allow_external_unmasked: false,
    }
}

pub(super) fn ready(setup: ModelSetup) -> ModelStack {
    let _ = kanade::runtime::tls::install_ring_provider();
    match build(setup, Arc::new(XorShift::new(7))).unwrap() {
        Models::Ready(stack) => *stack,
        Models::Unavailable => panic!("models unavailable"),
    }
}

fn build_error(setup: ModelSetup) -> SetupError {
    let _ = kanade::runtime::tls::install_ring_provider();
    build(setup, Arc::new(XorShift::new(7))).unwrap_err()
}

pub(super) fn temp_file(bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("kanade-setup-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn pem(der: &[u8]) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in der.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            text.push(if i <= chunk.len() {
                TABLE[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    let body: Vec<String> = text
        .as_bytes()
        .chunks(64)
        .map(|line| String::from_utf8(line.to_vec()).unwrap())
        .collect();
    format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        body.join("\n")
    )
    .into_bytes()
}

fn tls_acceptor() -> TlsAcceptor {
    let _ = kanade::runtime::tls::install_ring_provider();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        include_bytes!("../fixtures/provider/tls/loopback-key.pk8.der").to_vec(),
    ));
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(CERT.to_vec())], key)
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

#[test]
fn no_base_url_leaves_models_unavailable() {
    let models = build(setup(None), Arc::new(XorShift::new(7))).unwrap();
    assert!(matches!(models, Models::Unavailable));
}

#[test]
fn declared_groups_replace_the_gateway_group() {
    let _ = kanade::runtime::tls::install_ring_provider();
    let mut input = setup(Some("http://127.0.0.1:1".into()));
    input.roles.chat = role("codex-like", RoleEffort::Inherit);
    input.roles.rewrite = role("stray", RoleEffort::Inherit);
    let groups = [
        CapacityGroup {
            name: "local".into(),
            permits: 3,
            aliases: vec!["sumi-structured".into()],
        },
        CapacityGroup {
            name: "cloud".into(),
            permits: 5,
            aliases: vec!["codex-like".into(), "unused".into()],
        },
    ];
    let Models::Ready(stack) =
        build_with_groups(input, &groups, Arc::new(XorShift::new(7))).unwrap()
    else {
        panic!("models unavailable");
    };
    let snapshot = stack.governor.snapshot(chrono::DateTime::UNIX_EPOCH);
    let shape: Vec<_> = snapshot
        .iter()
        .map(|group| (group.name.as_str(), group.permits.total))
        .collect();
    assert_eq!(shape, [("local", 3), ("cloud", 5)]);
    let group = |role| stack.governor.route(role).unwrap().group;
    assert_eq!(group(Role::Extraction).as_deref(), Some("local"));
    assert_eq!(group(Role::Chat).as_deref(), Some("cloud"));
    assert_eq!(group(Role::Rewrite), None);

    let default = ready(setup(Some("http://127.0.0.1:1".into())));
    let names: Vec<_> = default
        .governor
        .snapshot(chrono::DateTime::UNIX_EPOCH)
        .into_iter()
        .map(|group| (group.name, group.permits.total))
        .collect();
    assert_eq!(names, [("gateway".to_owned(), 2)]);
}

#[test]
fn construction_refuses_bad_urls_keys_and_an_inheriting_extraction() {
    for (url, expected) in [
        ("http://10.0.0.5:11434", HttpConfigError::InsecureHttp),
        ("ftp://127.0.0.1", HttpConfigError::InvalidBaseUrl),
        ("https://gw.example//v1", HttpConfigError::InvalidBaseUrl),
    ] {
        match build_error(setup(Some(url.into()))) {
            SetupError::Http(error) => assert_eq!(error, expected, "{url}"),
            other => panic!("{url}: {other:?}"),
        }
    }
    let mut bad_key = setup(Some("http://127.0.0.1:1".into()));
    bad_key.key = Some(b"two words".to_vec());
    assert!(matches!(
        build_error(bad_key),
        SetupError::Http(HttpConfigError::InvalidBearerKey)
    ));
    let mut inherits = setup(Some("http://127.0.0.1:1".into()));
    inherits.roles.extraction.effort = RoleEffort::Inherit;
    assert!(matches!(
        build_error(inherits),
        SetupError::ExtractionInherits
    ));
}

#[tokio::test]
async fn builds_with_a_key_and_sends_it_only_as_a_header() {
    let stub = Stub::start(gateway(kanata_models(), "{}")).await;
    let mut input = setup(Some(stub.url()));
    input.key = Some(format!("{KEY}\n").into_bytes());
    assert!(!format!("{input:?}").contains(KEY));
    let stack = ready(input);
    assert!(!format!("{stack:?}").contains(KEY));
    assert!(stack.has_role(Role::Extraction) && stack.has_role(Role::Chat));
    assert!(!stack.has_role(Role::Rewrite));
    let report = stack.check_startup().await;
    assert!(matches!(report.listing, Listing::Listed { .. }));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        stub.requests()[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
}

#[tokio::test]
async fn builds_without_a_key() {
    let stub = Stub::start(gateway(ollama_models(), "{}")).await;
    let stack = ready(setup(Some(format!("{}/v1", stub.url()))));
    let report = stack.check_startup().await;
    assert!(matches!(report.listing, Listing::Listed { .. }));
    assert_eq!(stub.requests()[0].header("authorization"), None);
}

#[tokio::test]
async fn a_ca_file_in_pem_or_der_verifies_the_gateway() {
    let stub = Stub::start_with(gateway(ollama_models(), "{}"), Some(tls_acceptor())).await;
    for bytes in [pem(CERT), CERT.to_vec()] {
        let path = temp_file(&bytes);
        let mut input = setup(Some(format!("https://localhost:{}", stub.addr.port())));
        input.ca_file = Some(path.clone());
        let report = ready(input).check_startup().await;
        std::fs::remove_file(path).unwrap();
        assert!(matches!(report.listing, Listing::Listed { .. }));
    }
    let without_ca = ready(setup(Some(format!(
        "https://localhost:{}",
        stub.addr.port()
    ))));
    assert_eq!(
        without_ca.check_startup().await.listing,
        Listing::Degraded { reason_code: "tls" }
    );
}

#[test]
fn an_unusable_ca_file_is_refused() {
    let garbage = temp_file(b"not a certificate");
    for path in [garbage.clone(), PathBuf::from("/nonexistent/kanade-ca.pem")] {
        let mut input = setup(Some("https://gw.example".into()));
        input.ca_file = Some(path);
        assert!(matches!(build_error(input), SetupError::CaFile));
    }
    std::fs::remove_file(garbage).unwrap();
}

#[test]
fn stored_settings_map_onto_setup_roles() {
    use kanade::domain::settings::{Models as Stored, Reasoning, RoleModel as StoredRole};
    let stored = Stored {
        extraction: StoredRole {
            alias: Some("kanata/extract".into()),
            reasoning: Reasoning::High,
        },
        chat: StoredRole {
            alias: Some("kanata/chat".into()),
            reasoning: Reasoning::Inherit,
        },
        rewrite: StoredRole {
            alias: None,
            reasoning: Reasoning::Off,
        },
    };
    assert_eq!(
        ModelRoles::from(&stored),
        ModelRoles {
            extraction: role("kanata/extract", RoleEffort::Level(Effort::High)),
            chat: role("kanata/chat", RoleEffort::Inherit),
            rewrite: RoleModel {
                alias: None,
                effort: RoleEffort::Level(Effort::Off),
            },
        }
    );
    assert_eq!(ModelRoles::from(&Stored::default()), ModelRoles::default());
}
