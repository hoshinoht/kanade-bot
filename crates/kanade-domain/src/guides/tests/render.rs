use super::super::{
    ArtKind, Block, BlockError, EMBED_TOTAL_LIMIT, MAX_UPLOAD_BYTES, RenderWarning, Rendered,
    SPACER, render,
};
use super::support::{Sources, bosses, catalog, entry, id, text};
use crate::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};

fn rendered(blocks: &[Block], sources: &Sources) -> Rendered {
    render(blocks, &catalog(), sources).unwrap()
}

/// `count` plain bosses `B0..`, with the large entry image or not.
fn plain(count: usize, large_image: bool) -> Vec<super::super::BossEntry> {
    (0..count)
        .map(|n| entry(&format!("B{n}"), large_image))
        .collect()
}

/// Portrait and entry art for `B0..B<count>`, `size` bytes each, distinct.
fn plain_art(count: usize, size: usize) -> Sources {
    (0..count).fold(Sources::default(), |sources, n| {
        let bytes = |kind: u8| {
            let mut bytes = vec![0u8; size];
            bytes[0] = u8::try_from(n).unwrap();
            bytes[1] = kind;
            bytes
        };
        sources
            .with_art(ArtKind::Portrait, &format!("B{n}"), bytes(1))
            .with_art(ArtKind::Entry, &format!("B{n}"), bytes(2))
    })
}

#[test]
fn text_and_spacer_blocks_are_one_message_each() {
    let out = rendered(
        &[text(1, "# Hello"), Block::Spacer { id: id(2) }],
        &Sources::default(),
    );
    let contents: Vec<(&str, &str, u32)> = out
        .slots
        .iter()
        .map(|slot| (slot.block_id.as_str(), slot.content.as_str(), slot.part))
        .collect();
    assert_eq!(
        contents,
        [(id(1).as_str(), "# Hello", 0), (id(2).as_str(), SPACER, 0)]
    );
    assert!(out.slots.iter().all(|slot| slot.embeds.is_empty()));
}

#[test]
fn boss_embeds_carry_the_v4_fields_lead_and_note() {
    let sources = Sources::default()
        .with_art(ArtKind::Portrait, "MaleficStar", b"star".to_vec())
        .with_lead("MaleficStar", "  A falling star.  ");
    let mut star = entry("MaleficStar", false);
    star.note = Some("Bring potions.".into());
    let out = rendered(
        &[bosses(
            1,
            Some("## Bosses"),
            vec![star, entry("Kalos", false)],
        )],
        &sources,
    );
    let [slot] = out.slots.as_slice() else {
        panic!("{:?}", out.slots)
    };
    assert_eq!(slot.content, "## Bosses");
    let [star, kalos] = slot.embeds.as_slice() else {
        panic!("{:?}", slot.embeds)
    };
    assert_eq!(star.title, "Radiant Malefic Star · Lv280");
    assert_eq!(
        star.description,
        "Hard · Extreme\n`hmaleficstar · xmaleficstar`\nalso answers to: Shiny\n\nA falling star.\n\nBring potions."
    );
    assert_eq!(star.colour, 0x39BFFF);
    assert_eq!(star.thumbnail.as_deref(), Some("MaleficStar-portrait.png"));
    assert_eq!(star.image, None);
    assert_eq!(kalos.title, "Gatekeeper Kalos");
    assert_eq!(
        kalos.description,
        "Normal · Hard · Extreme\n`nkalos · hkalos · xkalos`"
    );
    assert_eq!(kalos.colour, 0x98A1B3);
    assert_eq!(
        slot.manifest()
            .iter()
            .map(|upload| upload.file_name.as_str())
            .collect::<Vec<_>>(),
        ["MaleficStar-portrait.png"]
    );
    // The portrait is always wanted: Kalos has none here, so it is reported.
    assert_eq!(
        out.warnings,
        [RenderWarning::MissingArt {
            block_id: id(1),
            entry: 1,
            key: "Kalos".into(),
            kind: ArtKind::Portrait,
        }]
    );
}

#[test]
fn portraits_use_the_catalog_basename_and_entry_art_the_key() {
    let sources = Sources::default()
        .with_art(ArtKind::Portrait, "KalosFace", b"face".to_vec())
        .with_art(ArtKind::Entry, "Kalos", b"entry".to_vec());
    let out = rendered(&[bosses(1, None, vec![entry("Kalos", true)])], &sources);
    let embed = &out.slots[0].embeds[0];
    assert_eq!(embed.thumbnail.as_deref(), Some("Kalos-portrait.png"));
    assert_eq!(embed.image.as_deref(), Some("Kalos-entry.png"));
    let uploads: Vec<(&str, &[u8])> = out.slots[0]
        .uploads
        .iter()
        .map(|upload| (upload.file_name.as_str(), &*upload.bytes))
        .collect();
    assert_eq!(
        uploads,
        [
            ("Kalos-entry.png", b"entry".as_slice()),
            ("Kalos-portrait.png", b"face".as_slice())
        ]
    );
}

#[test]
fn mixed_case_art_keys_resolve_exactly() {
    // Art filed under the wrong case is missing, not found.
    let sources = Sources::default().with_art(ArtKind::Portrait, "maleficstar", b"x".to_vec());
    let out = rendered(
        &[bosses(1, None, vec![entry("MaleficStar", false)])],
        &sources,
    );
    assert_eq!(out.slots[0].embeds[0].thumbnail, None);
    assert_eq!(
        out.warnings,
        [RenderWarning::MissingArt {
            block_id: id(1),
            entry: 0,
            key: "MaleficStar".into(),
            kind: ArtKind::Portrait,
        }]
    );
}

#[test]
fn missing_art_drops_the_picture_with_a_warning() {
    let out = rendered(
        &[bosses(1, None, vec![entry("Kalos", true)])],
        &Sources::default(),
    );
    let embed = &out.slots[0].embeds[0];
    assert_eq!((&embed.thumbnail, &embed.image), (&None, &None));
    assert!(out.slots[0].uploads.is_empty());
    let kinds: Vec<ArtKind> = out
        .warnings
        .iter()
        .map(|RenderWarning::MissingArt { kind, .. }| *kind)
        .collect();
    assert_eq!(kinds, [ArtKind::Portrait, ArtKind::Entry]);
}

#[test]
fn duplicate_art_is_uploaded_once_per_message() {
    let sources = Sources::default().with_art(ArtKind::Portrait, "MaleficStar", b"s".to_vec());
    let out = rendered(
        &[bosses(
            1,
            None,
            vec![entry("MaleficStar", false), entry("MaleficStar", false)],
        )],
        &sources,
    );
    let slot = &out.slots[0];
    assert_eq!(slot.uploads.len(), 1);
    assert!(
        slot.embeds
            .iter()
            .all(|embed| embed.thumbnail.as_deref() == Some("MaleficStar-portrait.png"))
    );
}

#[test]
fn ten_embeds_fit_one_message_and_eleven_take_two() {
    let none = Sources::default();
    let ten = rendered(&[bosses(1, Some("h"), plain(10, false))], &none);
    assert_eq!(ten.slots.len(), 1);
    assert_eq!(ten.slots[0].embeds.len(), 10);
    let eleven = rendered(&[bosses(1, Some("h"), plain(11, false))], &none);
    let shape: Vec<(u32, &str, usize)> = eleven
        .slots
        .iter()
        .map(|slot| (slot.part, slot.content.as_str(), slot.embeds.len()))
        .collect();
    assert_eq!(shape, [(0, "h", 10), (1, "", 1)]);
}

#[test]
fn messages_split_before_6000_embed_units() {
    let entries: Vec<_> = (0..8)
        .map(|n| {
            let mut entry = entry(&format!("B{n}"), false);
            entry.note = Some("w".repeat(1000));
            entry
        })
        .collect();
    let out = rendered(&[bosses(1, None, entries)], &Sources::default());
    assert!(out.slots.len() >= 2);
    for slot in &out.slots {
        let total: usize = slot.embeds.iter().map(|embed| embed.units()).sum();
        assert!(total <= EMBED_TOTAL_LIMIT, "{total}");
    }
    let first: usize = out.slots[0].embeds.iter().map(|embed| embed.units()).sum();
    assert!(first + out.slots[1].embeds[0].units() > EMBED_TOTAL_LIMIT);
}

#[test]
fn messages_split_at_ten_files() {
    let out = rendered(&[bosses(1, None, plain(6, true))], &plain_art(6, 16));
    let shape: Vec<(usize, usize)> = out
        .slots
        .iter()
        .map(|slot| (slot.embeds.len(), slot.uploads.len()))
        .collect();
    assert_eq!(shape, [(5, 10), (1, 2)]);
}

#[test]
fn messages_split_at_the_upload_byte_budget() {
    let size = 3 * 512 * 1024;
    let out = rendered(&[bosses(1, None, plain(6, false))], &plain_art(6, size));
    let shape: Vec<usize> = out.slots.iter().map(|slot| slot.uploads.len()).collect();
    assert_eq!(shape, [5, 1]);
    assert!(5 * size <= MAX_UPLOAD_BYTES && 6 * size > MAX_UPLOAD_BYTES);
}

#[test]
fn derived_descriptions_over_the_limit_are_refused() {
    let sources = Sources::default().with_lead("Kalos", &"l".repeat(4000));
    let mut kalos = entry("Kalos", false);
    kalos.note = Some("n".repeat(100));
    let error = render(&[bosses(1, None, vec![kalos])], &catalog(), &sources).unwrap_err();
    assert!(
        matches!(
            error.as_slice(),
            [BlockError::TooLong { field, limit: 4096, .. }] if field == "entries[0].description"
        ),
        "{error:?}"
    );
}

#[test]
fn a_guide_renders_at_most_200_messages() {
    let entries: Vec<_> = (0..2001).map(|_| entry("B0", false)).collect();
    let error = render(&[bosses(1, None, entries)], &catalog(), &Sources::default()).unwrap_err();
    assert_eq!(
        error,
        [BlockError::TooManySlots {
            count: 201,
            limit: 200
        }]
    );
}

#[test]
fn invalid_drafts_are_refused_before_rendering() {
    let error = render(&[text(1, "")], &catalog(), &Sources::default()).unwrap_err();
    assert_eq!(
        error,
        [BlockError::Empty {
            block_id: id(1),
            field: "content".into()
        }]
    );
}

#[test]
fn slot_hashes_are_stable_and_follow_catalog_and_art() {
    let blocks = [
        text(1, "Hello"),
        bosses(2, None, vec![entry("MaleficStar", false)]),
    ];
    let sources =
        || Sources::default().with_art(ArtKind::Portrait, "MaleficStar", b"star".to_vec());
    let hashes = |out: &Rendered| -> Vec<String> {
        out.slots.iter().map(|slot| slot.hash.clone()).collect()
    };
    let first = hashes(&rendered(&blocks, &sources()));
    assert_eq!(first, hashes(&rendered(&blocks, &sources())));
    // Pinned: a changed canonical form marks every published slot out of
    // date. The text slot is sha256 of
    // `{"content":"Hello","embeds":[],"flags":"silent","uploads":[]}`.
    // Unchanged by the 2026-10-16 narrowing of `image` to `large_image`:
    // the hash covers the rendered output, and a portrait-only entry
    // renders exactly as the old `thumbnail` choice did.
    assert_eq!(
        first,
        [
            "85cc23e51cbb724737faedb67e1befada97028456277264a85bdbb08ca6aaa0b",
            "c0bec53a2987e97b5d1e9dfffda8f64c9d62d1c64756f88bbca272d081333076",
        ]
    );

    let art = Sources::default().with_art(ArtKind::Portrait, "MaleficStar", b"new".to_vec());
    let changed = hashes(&rendered(&blocks, &art));
    assert_eq!(changed[0], first[0]);
    assert_ne!(changed[1], first[1]);

    let lead = sources().with_lead("MaleficStar", "Now with a lead.");
    assert_ne!(hashes(&rendered(&blocks, &lead))[1], first[1]);

    // The same boss without its guide colour (the default colour instead).
    let uncoloured = BossTable::from_spec(&CatalogSpec {
        difficulties: ["h", "x"]
            .map(|prefix| DifficultySpec {
                prefix: prefix.into(),
                label: if prefix == "h" { "Hard" } else { "Extreme" }.into(),
            })
            .to_vec(),
        bosses: vec![BossSpec {
            short: "MaleficStar".into(),
            full: Some("Radiant Malefic Star".into()),
            level: Some(280),
            aliases: vec!["Shiny".into()],
            ..BossSpec::default()
        }],
    })
    .unwrap();
    let recoloured = hashes(&render(&blocks, &uncoloured, &sources()).unwrap());
    assert_eq!(recoloured[0], first[0]);
    assert_ne!(recoloured[1], first[1]);
}
