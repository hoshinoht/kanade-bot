//! The run directory is private: owned by the current user, mode 0700, and
//! every file in it 0600. An existing directory is used only when it is an
//! empty directory (not a symlink) the user owns.

use std::{
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Create `dir` (and missing parents) 0700, or accept an existing empty
/// directory the user owns and tighten it to 0700; refuse anything else.
pub fn prepare(dir: &Path) -> Result<(), String> {
    let shown = dir.display();
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if !meta.file_type().is_dir() {
                return Err(format!("{shown} exists and is not a directory"));
            }
            if meta.uid() != current_uid() {
                return Err(format!("{shown} is owned by another user"));
            }
            let mut entries = fs::read_dir(dir).map_err(|e| format!("{shown}: {e}"))?;
            if entries.next().is_some() {
                return Err(format!("{shown} is not empty; pick a new --out"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| format!("{shown}: {e}"))?;
        }
        Err(error) => return Err(format!("{shown}: {error}")),
    }
    fs::set_permissions(dir, Permissions::from_mode(0o700)).map_err(|e| format!("{shown}: {e}"))
}

/// Write `bytes` to `path` as a 0600 file (an older file is truncated and
/// re-moded first).
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let shown = path.display();
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{shown}: {e}"))?;
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(|e| format!("{shown}: {e}"))?;
    file.write_all(bytes).map_err(|e| format!("{shown}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attempt::TempDir;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn an_existing_permissive_empty_dir_is_tightened_and_files_are_private() {
        let temp = TempDir::new("out-test").unwrap();
        let out = temp.0.join("run");
        fs::create_dir(&out).unwrap();
        fs::set_permissions(&out, Permissions::from_mode(0o755)).unwrap();
        prepare(&out).unwrap();
        assert_eq!(mode(&out), 0o700);

        let file = out.join("summary.md");
        fs::write(&file, "old").unwrap();
        fs::set_permissions(&file, Permissions::from_mode(0o644)).unwrap();
        write(&file, b"new").unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        write(&out.join("C01-1.json"), b"{}").unwrap();
        assert_eq!(mode(&out.join("C01-1.json")), 0o600);
    }

    #[test]
    fn a_missing_dir_is_created_private() {
        let temp = TempDir::new("out-test").unwrap();
        let out = temp.0.join("a/b");
        prepare(&out).unwrap();
        assert_eq!(mode(&out), 0o700);
        assert_eq!(mode(&temp.0.join("a")), 0o700);
    }

    #[test]
    fn a_non_empty_dir_a_file_or_a_symlink_is_refused() {
        let temp = TempDir::new("out-test").unwrap();
        let full = temp.0.join("full");
        fs::create_dir(&full).unwrap();
        fs::write(full.join("x"), "x").unwrap();
        assert!(prepare(&full).unwrap_err().contains("not empty"));
        let file = temp.0.join("file");
        fs::write(&file, "x").unwrap();
        assert!(prepare(&file).unwrap_err().contains("not a directory"));
        let link = temp.0.join("link");
        let target = temp.0.join("empty");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(prepare(&link).unwrap_err().contains("not a directory"));
    }
}
