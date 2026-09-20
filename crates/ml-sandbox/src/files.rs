//! JSON files on disk: the sandbox's only state outside the database.

use crate::Failure;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

/// Read and parse a JSON file.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, Failure> {
    let text = fs::read_to_string(path)
        .map_err(|e| Failure::undecided(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| Failure::undecided(format!("{} is not valid: {e}", path.display())))
}

/// Write a value as pretty JSON, replacing whatever was there — atomically,
/// so a crash mid-write leaves the old file intact rather than half of the
/// new one. The temporary file lives next to the target, so the rename never
/// crosses a filesystem.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), Failure> {
    let failed = |e: &dyn std::fmt::Display| {
        Failure::undecided(format!("cannot write {}: {e}", path.display()))
    };
    let name = path.file_name().ok_or_else(|| failed(&"not a file path"))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);

    let file = fs::File::create(&tmp).map_err(|e| failed(&e))?;
    if let Err(e) = fill(file, &tmp, value) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        failed(&e)
    })
}

/// Write a value as pretty JSON into a new file only the owner can read.
/// Refuses if the file exists: this is for secrets, and a secret is never
/// silently replaced.
pub fn create_private_json<T: Serialize>(path: &Path, value: &T) -> Result<(), Failure> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|e| Failure::undecided(format!("cannot create {}: {e}", path.display())))?;
    fill(file, path, value)
}

fn fill<T: Serialize>(mut file: fs::File, path: &Path, value: &T) -> Result<(), Failure> {
    let failed = |e: &dyn std::fmt::Display| {
        Failure::undecided(format!("cannot write {}: {e}", path.display()))
    };
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| failed(&e))?;
    file.write_all(b"\n").map_err(|e| failed(&e))
}
