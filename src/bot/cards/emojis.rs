//! The difficulty pills as application emojis: their fixed names, the
//! startup map from the application's emojis to [`DifficultyMarks`] (a
//! missing pill leaves its label written out), and the idempotent upload
//! behind `kanade ctl emojis`, which never deletes or renames an emoji.

use std::path::Path;

use crate::bot::delivery::cards::DifficultyMarks;
use crate::bot::transport::{ApplicationEmoji, DiscordTransport, Outcome};

/// Difficulty letter → application emoji name, in display order. The PNGs
/// are `assets/emojis/<name>.png`.
pub const PILLS: [(&str, &str); 4] = [
    ("n", "diff_n"),
    ("h", "diff_h"),
    ("c", "diff_c"),
    ("x", "diff_x"),
];

/// Marks for the pills among `emojis`, matched by exact name.
pub fn marks_from(emojis: &[ApplicationEmoji]) -> DifficultyMarks {
    let mut marks = DifficultyMarks::new();
    for (letter, name) in PILLS {
        if let Some(emoji) = emojis.iter().find(|emoji| emoji.name == name) {
            marks.insert(letter, &emoji.markup());
        }
    }
    marks
}

/// List the application's emojis and map the pills found.
pub async fn difficulty_marks<T: DiscordTransport>(transport: &T) -> Outcome<DifficultyMarks> {
    transport
        .application_emojis()
        .await
        .map(|emojis| marks_from(&emojis))
}

/// What happened to one pill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PillStep {
    /// Already uploaded; left alone.
    Present(ApplicationEmoji),
    Uploaded(ApplicationEmoji),
    /// Missing; a dry run uploads nothing.
    Missing,
    /// The PNG could not be read.
    Unreadable(String),
    /// Discord refused the upload, or its outcome is unknown (`label`).
    Failed(String),
}

/// One run of the upload: how many application emojis were listed and
/// each pill's step, in [`PILLS`] order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    pub listed: usize,
    pub pills: Vec<(&'static str, PillStep)>,
}

impl std::fmt::Display for SyncReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "application emojis: {} listed", self.listed)?;
        for (name, step) in &self.pills {
            match step {
                PillStep::Present(emoji) => writeln!(f, "{name}: present ({})", emoji.id)?,
                PillStep::Uploaded(emoji) => writeln!(f, "{name}: uploaded ({})", emoji.id)?,
                PillStep::Missing => writeln!(f, "{name}: missing (dry run, not uploaded)")?,
                PillStep::Unreadable(detail) => writeln!(f, "{name}: not uploaded ({detail})")?,
                PillStep::Failed(label) => writeln!(f, "{name}: upload failed ({label})")?,
            }
        }
        Ok(())
    }
}

/// List the application's emojis, then upload each pill missing by name
/// from `dir/<name>.png` (none with `dry_run`). `Err` is the list's failure
/// label: nothing is uploaded without a successful list.
pub async fn sync<T: DiscordTransport>(
    transport: &T,
    dir: &Path,
    dry_run: bool,
) -> Result<SyncReport, String> {
    let listed = match transport.application_emojis().await {
        Outcome::Delivered(emojis) => emojis,
        failed => return Err(failed.failure_label().unwrap_or_default()),
    };
    let mut pills = Vec::with_capacity(PILLS.len());
    for (_, name) in PILLS {
        let step = if let Some(emoji) = listed.iter().find(|emoji| emoji.name == name) {
            PillStep::Present(emoji.clone())
        } else if dry_run {
            PillStep::Missing
        } else {
            let path = dir.join(format!("{name}.png"));
            match std::fs::read(&path) {
                Err(error) => PillStep::Unreadable(format!("{}: {error}", path.display())),
                Ok(png) => match transport.create_application_emoji(name, &png).await {
                    Outcome::Delivered(emoji) => PillStep::Uploaded(emoji),
                    failed => PillStep::Failed(failed.failure_label().unwrap_or_default()),
                },
            }
        };
        pills.push((name, step));
    }
    Ok(SyncReport {
        listed: listed.len(),
        pills,
    })
}
