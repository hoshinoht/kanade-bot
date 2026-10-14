//! Bounded reads of tracked and operator files. The final path component is
//! never followed as a symlink, a FIFO or device cannot block the open, and
//! every check runs on the opened descriptor, so a path swapped after an
//! earlier look cannot slip past them.

use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use super::LoadError;

/// Why a file was refused; each caller words it for its own error type.
#[derive(Debug)]
pub(crate) enum Refusal {
    Symlink,
    NotRegular,
    /// More than one hard link: the same file is reachable from elsewhere.
    Linked,
    TooLarge,
    Io(io::Error),
}

/// Opens `path` read-only and checks the opened file: regular, one link and
/// at most `max` bytes. The metadata is that of the opened descriptor.
pub(crate) fn open_regular(path: &Path, max: u64) -> Result<(File, Metadata), Refusal> {
    // O_NONBLOCK: opening a FIFO with no writer would otherwise wait forever.
    // Regular files, the only ones read past the checks, ignore it.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP) => Refusal::Symlink,
            _ => Refusal::Io(error),
        })?;
    let metadata = file.metadata().map_err(Refusal::Io)?;
    if !metadata.is_file() {
        return Err(Refusal::NotRegular);
    }
    if metadata.nlink() != 1 {
        return Err(Refusal::Linked);
    }
    if metadata.len() > max {
        return Err(Refusal::TooLarge);
    }
    Ok((file, metadata))
}

/// Reads at most `max` bytes; a file that grew past it since the open is refused.
pub(crate) fn read_capped(file: File, max: u64) -> Result<Vec<u8>, Refusal> {
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(Refusal::Io)?;
    if bytes.len() as u64 > max {
        return Err(Refusal::TooLarge);
    }
    Ok(bytes)
}

/// Reads a regular UTF-8 file of at most `max` bytes.
pub(super) fn read_text(path: &Path, max: u64) -> Result<String, LoadError> {
    let refused = |refusal| {
        let problem = match refusal {
            Refusal::Symlink => "is a symbolic link".to_owned(),
            Refusal::NotRegular => "not a regular file".to_owned(),
            Refusal::Linked => "has more than one hard link".to_owned(),
            Refusal::TooLarge => format!("larger than {max} bytes"),
            Refusal::Io(error) => error.to_string(),
        };
        LoadError::new(path, problem)
    };
    let (file, _) = open_regular(path, max).map_err(refused)?;
    let bytes = read_capped(file, max).map_err(refused)?;
    String::from_utf8(bytes).map_err(|_| LoadError::new(path, "not valid UTF-8"))
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        ffi::CString,
        fs,
        os::unix::{ffi::OsStrExt, fs::symlink},
        path::PathBuf,
        sync::mpsc,
        thread,
        time::Duration,
    };

    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("kanade-read-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mkfifo(path: &Path) {
        let c_path = CString::new(path.as_os_str().as_bytes()).expect("no NUL");
        // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0, "mkfifo");
    }

    #[test]
    fn a_regular_file_reads_within_the_cap() {
        let temp = Temp::new();
        let path = temp.0.join("Doc.yaml");
        fs::write(&path, "boss: Zeta\n").expect("write");
        assert_eq!(read_text(&path, 64).expect("read"), "boss: Zeta\n");
    }

    #[test]
    fn a_symlinked_file_is_refused_even_to_an_inside_target() {
        let temp = Temp::new();
        let target = temp.0.join("Target.yaml");
        fs::write(&target, "boss: Zeta\n").expect("write");
        let link = temp.0.join("Link.yaml");
        symlink(&target, &link).expect("symlink");
        assert!(matches!(open_regular(&link, 64), Err(Refusal::Symlink)));
        assert_eq!(
            read_text(&link, 64).expect_err("symlink").problem,
            "is a symbolic link"
        );
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let temp = Temp::new();
        let path = temp.0.join("Pipe.yaml");
        mkfifo(&path);
        // A blocking open would wait for a writer forever; run it aside so a
        // regression fails the test instead of hanging the suite.
        let (sent, received) = mpsc::channel();
        thread::spawn(move || {
            let _ = sent.send(open_regular(&path, 64).map(|_| ()));
        });
        let result = received
            .recv_timeout(Duration::from_secs(5))
            .expect("open returned promptly");
        assert!(matches!(result, Err(Refusal::NotRegular)), "{result:?}");
    }

    #[test]
    fn a_hard_linked_file_is_refused_by_either_name() {
        let temp = Temp::new();
        let first = temp.0.join("First.yaml");
        fs::write(&first, "boss: Zeta\n").expect("write");
        let second = temp.0.join("second.yaml");
        fs::hard_link(&first, &second).expect("hard link");
        for path in [&first, &second] {
            assert!(matches!(open_regular(path, 64), Err(Refusal::Linked)));
        }
        fs::remove_file(&second).expect("unlink");
        open_regular(&first, 64).expect("one link again");
    }

    #[test]
    fn an_oversize_file_and_a_directory_are_refused() {
        let temp = Temp::new();
        let path = temp.0.join("Big.yaml");
        fs::write(&path, "x".repeat(65)).expect("write");
        assert!(matches!(open_regular(&path, 64), Err(Refusal::TooLarge)));
        assert_eq!(
            read_text(&path, 64).expect_err("oversize").problem,
            "larger than 64 bytes"
        );
        assert!(matches!(
            open_regular(&temp.0, 64),
            Err(Refusal::NotRegular)
        ));
    }
}
