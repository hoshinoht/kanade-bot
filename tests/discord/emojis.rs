//! The difficulty pills as application emojis on the fake transport:
//! `kanade ctl emojis` uploads only the missing fixed names (idempotent,
//! never touching other emojis), and the startup map turns the listed
//! pills into card marks, falling back to written labels.

use std::path::{Path, PathBuf};

use kanade::bot::cards::emojis::{self, PILLS};
use kanade::bot::transport::{
    AmbiguousKind, ApplicationEmoji, Call, FakeDiscord, Op, Outcome, RejectionKind, Step,
};
use kanade::cli::emojis::{Args, sync};
use twilight_model::id::Id;

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/emojis")
}

fn emoji(id: u64, name: &str) -> ApplicationEmoji {
    ApplicationEmoji {
        id: Id::new(id),
        name: name.into(),
        animated: false,
    }
}

fn uploads(fake: &FakeDiscord) -> Vec<(String, Vec<u8>)> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::CreateApplicationEmoji { name, png, .. } => Some((name, png)),
            _ => None,
        })
        .collect()
}

fn names(list: &[ApplicationEmoji]) -> Vec<&str> {
    list.iter().map(|emoji| emoji.name.as_str()).collect()
}

async fn run(fake: &FakeDiscord, dir: &Path, dry_run: bool) -> (Result<(), String>, String) {
    let mut out = Vec::new();
    let args = Args {
        dry_run,
        dir: dir.to_path_buf(),
    };
    let result = sync(fake, &args, &mut out)
        .await
        .map_err(|error| error.to_string());
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn the_committed_pills_are_128px_pngs() {
    for (_, name) in PILLS {
        let png = std::fs::read(assets().join(format!("{name}.png"))).expect(name);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{name}");
        assert_eq!(&png[12..16], b"IHDR", "{name}");
        let size = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        assert_eq!((size(16), size(20)), (128, 128), "{name}");
        assert!(png.len() < 256 * 1024, "{name}: Discord's emoji size cap");
    }
}

#[tokio::test]
async fn uploads_only_the_missing_pills_and_leaves_others_alone() {
    let fake = FakeDiscord::new();
    fake.seed_application_emojis(vec![emoji(7, "diff_h"), emoji(8, "party_parrot")]);
    let (result, report) = run(&fake, &assets(), false).await;
    assert_eq!(result, Ok(()));
    let uploaded = uploads(&fake);
    assert_eq!(
        uploaded
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["diff_n", "diff_c", "diff_x"]
    );
    for (name, png) in &uploaded {
        assert_eq!(
            png,
            &std::fs::read(assets().join(format!("{name}.png"))).unwrap(),
            "{name}: the committed bytes"
        );
    }
    assert_eq!(
        names(&fake.application_emojis()),
        ["diff_h", "party_parrot", "diff_n", "diff_c", "diff_x"],
        "nothing deleted or renamed"
    );
    assert!(
        report.starts_with("application emojis: 2 listed\n"),
        "{report}"
    );
    assert!(report.contains("diff_h: present (7)\n"), "{report}");
    assert!(report.contains("diff_n: uploaded ("), "{report}");

    // Idempotent: a second run lists, finds all four and uploads nothing.
    let (again, report) = run(&fake, &assets(), false).await;
    assert_eq!(again, Ok(()));
    assert_eq!(fake.count(Op::CreateApplicationEmoji), 3);
    assert_eq!(report.matches(": present (").count(), 4, "{report}");
}

#[tokio::test]
async fn a_dry_run_lists_and_uploads_nothing() {
    let fake = FakeDiscord::new();
    fake.seed_application_emojis(vec![emoji(7, "diff_x")]);
    let (result, report) = run(&fake, Path::new("/nonexistent"), true).await;
    assert_eq!(result, Ok(()), "missing pills are not a dry-run failure");
    assert_eq!(fake.count(Op::CreateApplicationEmoji), 0);
    assert_eq!(
        report,
        "application emojis: 1 listed\n\
         diff_n: missing (dry run, not uploaded)\n\
         diff_h: missing (dry run, not uploaded)\n\
         diff_c: missing (dry run, not uploaded)\n\
         diff_x: present (7)\n"
    );
}

#[tokio::test]
async fn nothing_is_uploaded_without_a_successful_list() {
    let fake = FakeDiscord::new();
    fake.script(
        Op::ApplicationEmojis,
        Step::Reject(RejectionKind::Unauthorized),
    );
    let (result, report) = run(&fake, &assets(), false).await;
    assert!(result.unwrap_err().contains("unauthorized"));
    assert!(report.is_empty());
    assert_eq!(fake.count(Op::CreateApplicationEmoji), 0);
}

#[tokio::test]
async fn a_failed_upload_is_reported_and_the_rest_continue() {
    let fake = FakeDiscord::new();
    fake.script(
        Op::CreateApplicationEmoji,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    );
    let (result, report) = run(&fake, &assets(), false).await;
    assert!(result.is_err(), "an incomplete set fails the command");
    assert!(
        report.contains("diff_n: upload failed (ambiguous_timeout)"),
        "{report}"
    );
    assert_eq!(
        names(&fake.application_emojis()),
        ["diff_h", "diff_c", "diff_x"]
    );
    // The next run retries only the one still missing (4 attempts, then 1).
    let (again, _) = run(&fake, &assets(), false).await;
    assert_eq!(again, Ok(()));
    let attempted: Vec<String> = uploads(&fake).into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        attempted,
        ["diff_n", "diff_h", "diff_c", "diff_x", "diff_n"]
    );
}

#[tokio::test]
async fn an_unreadable_png_is_reported_not_uploaded() {
    let fake = FakeDiscord::new();
    let (result, report) = run(&fake, Path::new("/nonexistent"), false).await;
    assert!(result.is_err());
    assert_eq!(report.matches(": not uploaded (").count(), 4, "{report}");
    assert_eq!(fake.count(Op::CreateApplicationEmoji), 0);
}

#[tokio::test]
async fn startup_maps_the_pills_found_and_writes_the_rest_out() {
    let fake = FakeDiscord::new();
    fake.seed_application_emojis(vec![
        emoji(11, "diff_h"),
        emoji(12, "diff_x"),
        emoji(13, "diff_hard"),
    ]);
    let Outcome::Delivered(marks) = emojis::difficulty_marks(&fake).await else {
        panic!("listed");
    };
    assert_eq!(marks.get("h"), Some("<:diff_h:11>"));
    assert_eq!(marks.get("X"), Some("<:diff_x:12>"));
    assert_eq!(
        marks.get("n"),
        None,
        "a missing pill keeps its written label"
    );
    assert_eq!(marks.get("c"), None);

    let failing = FakeDiscord::new();
    failing.script(Op::ApplicationEmojis, Step::Reject(RejectionKind::NotSent));
    assert_eq!(
        emojis::difficulty_marks(&failing).await,
        Outcome::DefinitelyRejected(RejectionKind::NotSent)
    );
}

#[test]
fn an_animated_emoji_keeps_its_markup() {
    let animated = ApplicationEmoji {
        animated: true,
        ..emoji(5, "diff_c")
    };
    assert_eq!(
        emojis::marks_from(&[animated]).get("c"),
        Some("<a:diff_c:5>")
    );
}
