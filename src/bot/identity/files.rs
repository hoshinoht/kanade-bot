//! The identity directory: `avatar.<ext>` / `banner.<ext>`, replaced by
//! temp file + rename so the API never serves a half-written image.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use crate::api::assets::IDENTITY_SUFFIXES;

/// Create the directory (owner-only) if missing.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Atomically write `<stem>.<ext>`, then drop the stem's other extensions.
pub fn store(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> io::Result<()> {
    let temp = dir.join(format!(".{stem}.tmp"));
    let written = File::create(&temp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temp, dir.join(format!("{stem}.{ext}"))))
    {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    clear(dir, stem, Some(ext))
}

/// Remove every cached `<stem>.*` except `keep`.
pub fn clear(dir: &Path, stem: &str, keep: Option<&str>) -> io::Result<()> {
    for suffix in IDENTITY_SUFFIXES {
        if Some(suffix) == keep {
            continue;
        }
        match fs::remove_file(dir.join(format!("{stem}.{suffix}"))) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

pub fn cached(dir: &Path, stem: &str) -> bool {
    IDENTITY_SUFFIXES
        .iter()
        .any(|suffix| dir.join(format!("{stem}.{suffix}")).is_file())
}
