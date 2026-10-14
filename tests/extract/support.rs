//! The synthetic boss catalog the extraction tests share.

use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};

/// Malefic Star, First Adversary, Chief Carling, Kalos, Baldrix and Limbo over
/// five difficulties (e, n, h, c, x).
pub fn fixture_catalog() -> BossTable {
    let difficulties = [
        ("e", "Easy"),
        ("n", "Normal"),
        ("h", "Hard"),
        ("c", "Chaos"),
        ("x", "Extreme"),
    ]
    .map(|(prefix, label)| DifficultySpec {
        prefix: prefix.into(),
        label: label.into(),
    })
    .to_vec();
    let boss = |short: &str, full: &str, prefixes: &[&str], aliases: &[&str]| BossSpec {
        short: short.into(),
        full: Some(full.into()),
        difficulties: Some(prefixes.iter().map(|p| (*p).to_owned()).collect()),
        aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
        ..BossSpec::default()
    };
    let bosses = vec![
        boss(
            "MaleficStar",
            "Malefic Star",
            &["n", "h"],
            &["star", "mstar"],
        ),
        boss("FA", "First Adversary", &["n", "h", "x"], &["fa"]),
        boss(
            "Carling",
            "Chief Carling",
            &["n", "h", "x"],
            &["carl", "carling"],
        ),
        boss(
            "Kalos",
            "Kalos the Guardian",
            &["e", "n", "c", "x"],
            &["kalos"],
        ),
        boss("Baldrix", "Baldrix", &["n", "h"], &["bald", "baldguy"]),
        boss("Limbo", "Limbo", &["n", "h"], &["limbo"]),
    ];
    BossTable::from_spec(&CatalogSpec {
        difficulties,
        bosses,
    })
    .expect("fixture catalog is valid")
}
