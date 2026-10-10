use std::{fs, io::Read, path::Path};

use super::LoadError;

/// Reads a regular UTF-8 file of at most `max` bytes.
pub(super) fn read_text(path: &Path, max: u64) -> Result<String, LoadError> {
    let metadata = fs::metadata(path).map_err(|error| LoadError::new(path, error.to_string()))?;
    if !metadata.is_file() {
        return Err(LoadError::new(path, "not a regular file"));
    }
    if metadata.len() > max {
        return Err(LoadError::new(path, format!("larger than {max} bytes")));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(max + 1).read_to_end(&mut bytes))
        .map_err(|error| LoadError::new(path, error.to_string()))?;
    if bytes.len() as u64 > max {
        return Err(LoadError::new(path, format!("larger than {max} bytes")));
    }
    String::from_utf8(bytes).map_err(|_| LoadError::new(path, "not valid UTF-8"))
}
