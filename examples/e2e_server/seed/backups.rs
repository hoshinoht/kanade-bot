//! `KANADE_BACKUP_DIR` as the mock's History → Checkpoints lists it: one
//! backup for each anchor state the server reports, newest first. `kanade
//! backup` writes these while serve is stopped; here each is a placeholder
//! snapshot (the page only checks it exists) beside a manifest naming the
//! seeded history head. The directory lives outside the store, so every
//! generation lists the same three.

use std::{fs, path::Path};

use chrono::{DateTime, Datelike, Timelike, Utc};
use kanade::{
    domain::history::ChangeHistory, infrastructure::store::sqlite::BACKUP_MANIFEST_FORMAT,
};
use serde_json::json;

use super::{ScheduleStore, Scope, Seed};
use crate::{Fail, controls::iso};

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    let Some(dir) = seed.state.backups.dir.as_deref() else {
        return Err("the harness lists backups: KANADE_BACKUP_DIR is unset".into());
    };
    let head = seed.store.history_head().await?;
    let schema = seed.state.backups.schema_version;
    let revision = seed.store.load(&Scope::All).await?.revision;
    // (taken, deploy commit, head hash, schema): the mock's three, newest first.
    let forked: String = head.hash.chars().rev().collect();
    for (taken, sha, hash, schema) in [
        (seed.ago(3 * 3600), "8c30b19", head.hash.as_str(), schema),
        // A head from a forked history: plausible, never in this chain.
        (seed.ago(26 * 3600), "a59bc67", forked.as_str(), schema),
        (
            seed.ago(50 * 3600),
            "630803c",
            head.hash.as_str(),
            schema - 1,
        ),
    ] {
        let file = format!("kanade-{}-pre-{sha}.sqlite", stamp(taken));
        let manifest = json!({
            "format": BACKUP_MANIFEST_FORMAT,
            "history_head": { "seq": head.seq, "hash": hash },
            "revision": revision,
            "schema_version": schema,
            "created_at": iso(taken),
        });
        write(dir, &file, b"placeholder snapshot")?;
        write(
            dir,
            &format!("{file}.manifest.json"),
            manifest.to_string().as_bytes(),
        )?;
    }
    Ok(())
}

/// `YYYYMMDD-HHMM` in UTC, as the deploy runbook names snapshots (chrono's
/// `format` is not compiled in).
fn stamp(at: DateTime<Utc>) -> String {
    format!(
        "{:04}{:02}{:02}-{:02}{:02}",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute()
    )
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), Fail> {
    fs::write(dir.join(name), bytes).map_err(|error| format!("{name}: {error}").into())
}
