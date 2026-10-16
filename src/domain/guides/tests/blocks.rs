use super::super::{Block, BlockError, MAX_BLOCKS, units, validate};
use super::support::{bosses, catalog, entry, id, text};

fn errors(blocks: &[Block]) -> Vec<BlockError> {
    validate(blocks, &catalog())
}

fn too_long(block_id: &str, field: &str, units: usize, limit: usize) -> BlockError {
    BlockError::TooLong {
        block_id: block_id.to_owned(),
        field: field.to_owned(),
        units,
        limit,
    }
}

#[test]
fn text_is_capped_at_2000_utf16_units() {
    assert!(errors(&[text(1, &"a".repeat(2000))]).is_empty());
    assert_eq!(
        errors(&[text(1, &"a".repeat(2001))]),
        [too_long(&id(1), "content", 2001, 2000)]
    );
    // A surrogate pair counts 2, as Discord counts it.
    assert_eq!(units("😀"), 2);
    assert!(errors(&[text(1, &"😀".repeat(1000))]).is_empty());
    assert_eq!(
        errors(&[text(1, &format!("{}a", "😀".repeat(1000)))]),
        [too_long(&id(1), "content", 2001, 2000)]
    );
}

#[test]
fn blank_text_heading_and_note_are_refused() {
    let empty = |n: usize, field: &str| BlockError::Empty {
        block_id: id(n),
        field: field.to_owned(),
    };
    let mut noted = entry("MaleficStar", false);
    noted.note = Some(" \n".into());
    assert_eq!(
        errors(&[
            text(1, ""),
            text(2, "  \n\t"),
            bosses(3, Some(" "), vec![noted]),
            bosses(4, None, vec![]),
        ]),
        [
            empty(1, "content"),
            empty(2, "content"),
            empty(3, "heading"),
            empty(3, "entries[0].note"),
            empty(4, "entries"),
        ]
    );
}

#[test]
fn notes_are_capped_at_1000_units() {
    let mut noted = entry("MaleficStar", false);
    noted.note = Some("n".repeat(1000));
    assert!(errors(&[bosses(1, None, vec![noted.clone()])]).is_empty());
    noted.note = Some("n".repeat(1001));
    assert_eq!(
        errors(&[bosses(1, None, vec![noted])]),
        [too_long(&id(1), "entries[0].note", 1001, 1000)]
    );
}

#[test]
fn boss_keys_match_the_catalog_exactly() {
    let blocks = [bosses(
        1,
        None,
        vec![
            entry("MaleficStar", false),
            entry("maleficstar", false),
            entry("Shiny", false),
        ],
    )];
    let unknown = |at: usize, key: &str| BlockError::UnknownBoss {
        block_id: id(1),
        field: format!("entries[{at}].key"),
        key: key.to_owned(),
    };
    assert_eq!(
        errors(&blocks),
        [unknown(1, "maleficstar"), unknown(2, "Shiny")]
    );
}

#[test]
fn ids_are_unique_uuids_and_blocks_are_bounded() {
    let bad = Block::Spacer {
        id: "spacer".into(),
    };
    assert_eq!(
        errors(&[text(1, "a"), bad, text(1, "b")]),
        [
            BlockError::BadId {
                block_id: "spacer".into()
            },
            BlockError::DuplicateId { block_id: id(1) },
        ]
    );
    let many: Vec<Block> = (0..=MAX_BLOCKS)
        .map(|n| Block::Spacer { id: id(n) })
        .collect();
    assert_eq!(
        errors(&many),
        [BlockError::TooManyBlocks {
            count: 51,
            limit: 50
        }]
    );
    let large: Vec<Block> = (0..40).map(|n| text(n, &"x".repeat(1700))).collect();
    assert!(matches!(
        errors(&large).as_slice(),
        [BlockError::TooLarge { limit: 65536, .. }]
    ));
}

#[test]
fn blocks_round_trip_as_tagged_json() {
    let blocks = vec![
        text(1, "Hello"),
        Block::Spacer { id: id(2) },
        bosses(3, Some("Bosses"), vec![entry("Kalos", true)]),
    ];
    let json = serde_json::to_string(&blocks).unwrap();
    assert_eq!(
        json,
        format!(
            r#"[{{"kind":"text","id":"{}","content":"Hello"}},{{"kind":"spacer","id":"{}"}},{{"kind":"bosses","id":"{}","heading":"Bosses","entries":[{{"key":"Kalos","large_image":true}}]}}]"#,
            id(1),
            id(2),
            id(3)
        )
    );
    assert_eq!(serde_json::from_str::<Vec<Block>>(&json).unwrap(), blocks);
    // The portrait thumbnail alone is the default (no large image, and
    // `large_image: false` is not written); unknown fields fail, so the old
    // `image` choices (incl. `none`) are refused rather than misread.
    let parsed: Block = serde_json::from_str(&format!(
        r#"{{"kind":"bosses","id":"{}","entries":[{{"key":"Kalos"}}]}}"#,
        id(4)
    ))
    .unwrap();
    assert_eq!(parsed, bosses(4, None, vec![entry("Kalos", false)]));
    assert_eq!(
        serde_json::to_string(&parsed).unwrap(),
        format!(
            r#"{{"kind":"bosses","id":"{}","entries":[{{"key":"Kalos"}}]}}"#,
            id(4)
        )
    );
    for bad in [
        r#"{"kind":"text","id":"x","content":"a","extra":1}"#,
        r#"{"kind":"bosses","id":"x","entries":[{"key":"Kalos","colour":1}]}"#,
        r#"{"kind":"bosses","id":"x","entries":[{"key":"Kalos","image":"none"}]}"#,
        r#"{"kind":"bosses","id":"x","entries":[{"key":"Kalos","image":"thumbnail"}]}"#,
        r#"{"kind":"bosses","id":"x","entries":[{"key":"Kalos","large_image":"yes"}]}"#,
        r#"{"kind":"poll","id":"x"}"#,
    ] {
        assert!(serde_json::from_str::<Block>(bad).is_err(), "{bad}");
    }
}
