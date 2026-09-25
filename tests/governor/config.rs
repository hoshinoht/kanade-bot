use std::sync::Arc;

use kanade::infrastructure::llm::{
    AdmissionLimits,
    governor::{
        CallKind, ConfigError, ConfigWarning, Governor, GovernorConfig, GovernorPolicy, Priority,
        Refused, Role, RoleConfig, RoleRoute,
    },
};

use crate::support::{ALIAS, FixedRandom, HALF, LONG, group, roles, single, ticket};

fn err(config: &GovernorConfig) -> ConfigError {
    config.validate().expect_err("config must be refused")
}

#[test]
fn roles_resolve_to_alias_group_and_trust_zone() {
    let mut config = GovernorConfig {
        groups: vec![
            group("gpu", 1, 30, &["local-a", "local-b"]),
            group("cloud", 8, 120, &["cloud-a"]),
        ],
        roles: roles("local-a", false),
        policy: GovernorPolicy::default(),
    };
    config.roles.insert(
        Role::Chat,
        RoleConfig {
            alias: "cloud-a".into(),
            external: true,
        },
    );
    let (routes, warnings) = config.validate().expect("valid");
    assert!(warnings.is_empty());
    assert_eq!(
        routes,
        vec![
            RoleRoute {
                role: Role::Extraction,
                alias: "local-a".into(),
                group: Some("gpu".into()),
                external: false,
                unmasked_allowed: false,
            },
            RoleRoute {
                role: Role::Chat,
                alias: "cloud-a".into(),
                group: Some("cloud".into()),
                external: true,
                unmasked_allowed: false,
            },
            RoleRoute {
                role: Role::Rewrite,
                alias: "local-a".into(),
                group: Some("gpu".into()),
                external: false,
                unmasked_allowed: false,
            },
        ]
    );
}

#[test]
fn permits_must_be_whole_numbers_from_1_to_64() {
    for bad in [0, 65, 1_000] {
        let mut config = single(1, 60);
        config.groups[0].permits = bad;
        assert_eq!(
            err(&config),
            ConfigError::Permits {
                group: "local".into(),
                value: bad
            }
        );
        assert!(err(&config).to_string().contains("\"local\""));
    }
    for good in [1, 64] {
        let mut config = single(1, 60);
        config.groups[0].permits = good;
        assert!(config.validate().is_ok());
    }
}

#[test]
fn rate_and_burst_bounds_are_enforced() {
    let mut config = single(2, 0);
    assert!(matches!(
        err(&config),
        ConfigError::RequestsPerMin { value: 0, .. }
    ));
    config.groups[0].requests_per_min = 6_001;
    assert!(matches!(err(&config), ConfigError::RequestsPerMin { .. }));
    config.groups[0].requests_per_min = 60;
    config.groups[0].burst = Some(0);
    assert!(matches!(err(&config), ConfigError::Burst { value: 0, .. }));
    config.groups[0].burst = Some(1_001);
    assert!(matches!(err(&config), ConfigError::Burst { .. }));
    config.groups[0].burst = Some(12);
    assert!(config.validate().is_ok());
}

#[test]
fn group_and_alias_shape_is_enforced() {
    let mut config = single(1, 60);
    config.groups.push(group("local", 1, 60, &["other"]));
    assert_eq!(err(&config), ConfigError::DuplicateGroup("local".into()));

    let mut config = single(1, 60);
    config.groups.push(group("second", 1, 60, &[ALIAS]));
    assert_eq!(
        err(&config),
        ConfigError::DuplicateAlias {
            alias: ALIAS.into()
        }
    );

    let config = GovernorConfig {
        groups: vec![group("twice", 1, 60, &["a", "a"])],
        ..single(1, 60)
    };
    assert!(matches!(err(&config), ConfigError::DuplicateAlias { .. }));

    let config = GovernorConfig {
        groups: vec![group("empty", 1, 60, &[])],
        ..single(1, 60)
    };
    assert!(matches!(err(&config), ConfigError::NoAliases { .. }));

    let config = GovernorConfig {
        groups: vec![group(" ", 1, 60, &["a"])],
        ..single(1, 60)
    };
    assert_eq!(err(&config), ConfigError::EmptyGroupName);

    let config = GovernorConfig {
        groups: vec![group("blank", 1, 60, &[""])],
        ..single(1, 60)
    };
    assert!(matches!(err(&config), ConfigError::EmptyAlias { .. }));

    let mut config = single(1, 60);
    config.roles.get_mut(&Role::Chat).unwrap().alias = String::new();
    assert_eq!(
        err(&config),
        ConfigError::EmptyRoleAlias { role: Role::Chat }
    );
}

type Mutation = fn(&mut GovernorPolicy);

#[test]
fn policy_bounds_are_enforced() {
    let cases: [(Mutation, &str); 5] = [
        (|p| p.retry_permille = 201, "retry_permille"),
        (|p| p.breaker_threshold = 0, "breaker_threshold"),
        (
            |p| p.open_cooldown = std::time::Duration::ZERO,
            "open_cooldown",
        ),
        (
            |p| p.max_open_cooldown = std::time::Duration::from_secs(1),
            "max_open_cooldown",
        ),
        (
            |p| p.retry_window = std::time::Duration::ZERO,
            "retry_window",
        ),
    ];
    for (mutate, field) in cases {
        let mut config = single(1, 60);
        mutate(&mut config.policy);
        assert_eq!(err(&config), ConfigError::Policy(field));
    }
}

#[tokio::test(start_paused = true)]
async fn ungrouped_role_warns_and_is_refused_at_runtime() {
    let mut config = single(1, 60);
    config.roles.insert(
        Role::Rewrite,
        RoleConfig {
            alias: "unlisted".into(),
            external: false,
        },
    );
    config.roles.remove(&Role::Extraction);
    let governor = Governor::new(&config, Arc::new(FixedRandom(HALF))).unwrap();
    assert_eq!(
        governor.warnings(),
        [ConfigWarning::UngroupedRole {
            role: Role::Rewrite,
            alias: "unlisted".into()
        }]
    );
    assert_eq!(
        governor
            .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
            .unwrap_err(),
        Refused::Ungrouped
    );
    assert_eq!(
        governor
            .acquire(
                Role::Extraction,
                ticket(Priority::Extraction, "burst"),
                LONG
            )
            .await
            .unwrap_err(),
        Refused::UnknownRole
    );
}

#[tokio::test(start_paused = true)]
async fn permits_are_checked_against_published_gateway_admission() {
    let mut config = single(1, 60);
    config.groups = vec![
        group("gpu", 3, 60, &["local-model", "other-model"]),
        group("cloud", 8, 60, &["cloud-model"]),
    ];
    let published = |alias: &str| match alias {
        // Route 4, adapter 2: the adapter bounds it.
        "local-model" => Some(AdmissionLimits {
            max_in_flight: 4,
            max_queue: Some(16),
            queue_ms: Some(1_000),
            adapter_max_in_flight: Some(2),
        }),
        "other-model" => Some(AdmissionLimits {
            max_in_flight: 3,
            max_queue: None,
            queue_ms: None,
            adapter_max_in_flight: None,
        }),
        // Nothing published (public listener): operator-declared, not checked here.
        _ => None,
    };
    let expected = [ConfigWarning::PermitsAboveGateway {
        group: "gpu".into(),
        alias: "local-model".into(),
        permits: 3,
        gateway: 2,
    }];
    assert_eq!(config.capacity_warnings(published), expected);
    let governor = Governor::new_checked(&config, Arc::new(FixedRandom(HALF)), published).unwrap();
    assert_eq!(governor.warnings(), expected);

    config.groups[0].permits = 2;
    assert!(config.capacity_warnings(published).is_empty());
}

#[tokio::test(start_paused = true)]
async fn pre_screen_never_uses_an_external_route() {
    let mut config = single(1, 60);
    config.roles = roles(ALIAS, true);
    let governor = Governor::new(&config, Arc::new(FixedRandom(HALF))).unwrap();
    assert_eq!(
        governor
            .try_acquire(Role::Rewrite, CallKind::PreScreen, "msg")
            .unwrap_err(),
        Refused::ExternalForbidden
    );
    assert!(
        governor
            .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
            .is_ok()
    );
}
