//! Scouter boss keys → catalog tokens: the one table.
//!
//! The sidecar keys each card by the boss icon its result page draws,
//! `<difficulty>_<boss>` (`hard_maleficStar`, `extreme_kaling`), read from
//! the private `./scouter` source and its recorded answers (2026-10-12).
//! A key outside this table (`destiny_*`, `champion_*`, Verus Hilla, …)
//! and a token the running catalog lacks are dropped: estimates show
//! catalog bosses only.

use crate::domain::catalog::BossTable;

/// `(scouter key, catalog token)`. The scouter spells Baldrix both ways
/// (`bardrix` on its cards, `baldrix` in its party-size mapping).
pub const BOSS_KEYS: &[(&str, &str)] = &[
    ("extreme_lotus", "XLotus"),
    ("normal_seren", "NSeren"),
    ("hard_seren", "HSeren"),
    ("extreme_seren", "XSeren"),
    ("easy_kalos", "EKalos"),
    ("normal_kalos", "NKalos"),
    ("chaos_kalos", "CKalos"),
    ("extreme_kalos", "XKalos"),
    ("easy_adversary", "EFA"),
    ("normal_adversary", "NFA"),
    ("hard_adversary", "HFA"),
    ("extreme_adversary", "XFA"),
    ("easy_kaling", "ECarling"),
    ("normal_kaling", "NCarling"),
    ("hard_kaling", "HCarling"),
    ("extreme_kaling", "XCarling"),
    ("hard_blackMage", "HBM"),
    ("extreme_blackMage", "XBM"),
    ("normal_maleficStar", "NMaleficStar"),
    ("hard_maleficStar", "HMaleficStar"),
    ("normal_limbo", "NLimbo"),
    ("hard_limbo", "HLimbo"),
    ("normal_bardrix", "NBaldrix"),
    ("hard_bardrix", "HBaldrix"),
    ("normal_baldrix", "NBaldrix"),
    ("hard_baldrix", "HBaldrix"),
    ("normal_jupiter", "NJupiter"),
    ("hard_jupiter", "HJupiter"),
];

/// The catalog token for scouter `key`, while `catalog` has that boss at
/// that difficulty; `None` drops the card.
pub fn catalog_token(key: &str, catalog: &BossTable) -> Option<&'static str> {
    BOSS_KEYS
        .iter()
        .find(|(scouter, _)| *scouter == key)
        .map(|(_, token)| *token)
        .filter(|token| known(token, catalog))
}

/// The scouter keys that map to catalog `token` (empty when the scouter
/// has no card for it).
pub fn scouter_keys(token: &str) -> impl Iterator<Item = &'static str> + '_ {
    BOSS_KEYS
        .iter()
        .filter(move |(_, mapped)| *mapped == token)
        .map(|(scouter, _)| *scouter)
}

fn known(token: &str, catalog: &BossTable) -> bool {
    catalog.split(token).is_some_and(|(difficulty, boss)| {
        boss.difficulties()
            .iter()
            .any(|letter| letter == difficulty.letter())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::files::load_catalog;

    fn tracked() -> BossTable {
        load_catalog(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/bosses.yaml"))
            .expect("tracked catalog")
    }

    #[test]
    fn every_mapped_token_is_a_tracked_catalog_boss_and_difficulty() {
        let catalog = tracked();
        for (key, token) in BOSS_KEYS {
            assert_eq!(catalog_token(key, &catalog), Some(*token), "{key}");
        }
    }

    #[test]
    fn unknown_keys_and_tokens_outside_the_catalog_are_dropped() {
        let catalog = tracked();
        for key in [
            "destiny_kalos",
            "champion_seren",
            "champion_verusHilla",
            "hard_future",
            "hard_kalos",
            "",
        ] {
            assert_eq!(catalog_token(key, &catalog), None, "{key}");
        }
        let mut keys: Vec<_> = scouter_keys("HBaldrix").collect();
        keys.sort_unstable();
        assert_eq!(keys, ["hard_baldrix", "hard_bardrix"]);
        assert_eq!(scouter_keys("NBellona").count(), 0);
    }
}
