//! The proposal card desk with the test catalog and difficulty marks,
//! shared by the Components V2 card tests (`v2.rs`).

use super::*;
use kanade::cards::{CardKit, DifficultyMarks};
use kanade_domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};

fn catalog() -> BossTable {
    let boss = |short: &str, full: &str| BossSpec {
        short: short.into(),
        full: Some(full.into()),
        ..BossSpec::default()
    };
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![
            DifficultySpec {
                prefix: "n".into(),
                label: "Normal".into(),
            },
            DifficultySpec {
                prefix: "h".into(),
                label: "Hard".into(),
            },
        ],
        bosses: vec![
            boss("MaleficStar", "Radiant Malefic Star"),
            boss("FA", "The First Adversary"),
        ],
    })
    .expect("catalog")
}

/// A desk over `world` with the test catalog and `marks`, posting Components
/// V2 as serve does.
pub(super) fn styled_desk(world: &World, marks: DifficultyMarks) -> Desk {
    desk_with(
        &world.store,
        world.discord.clone(),
        &world.alerts,
        &world.ids,
        now(),
    )
    .with_cards(CardKit {
        catalog: Some(Arc::new(catalog())),
        marks,
        ..CardKit::default()
    })
}
