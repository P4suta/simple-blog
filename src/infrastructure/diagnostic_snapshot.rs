//! A stable observational copy, never an immutable assertion about a live database.
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

type Fingerprints = BTreeMap<String, (u64, blake3::Hash)>;

pub(super) fn capture(source: &Path) -> std::io::Result<tempfile::TempDir> {
    capture_with(source, || Ok(()))
}

fn capture_with(
    source: &Path,
    mut after_copy: impl FnMut() -> std::io::Result<()>,
) -> std::io::Result<tempfile::TempDir> {
    // Compare the whole source set before AND after copying, including WAL
    // creation/removal. Busy writers yield an explicit inconclusive result.
    for _ in 0..3 {
        let before = fingerprint(source)?;
        let snapshot = tempfile::tempdir()?;
        let destination = snapshot.path().join("database.sqlite3");
        for suffix in before.keys() {
            std::fs::copy(sidecar(source, suffix), sidecar(&destination, suffix))?;
        }
        after_copy()?;
        if before == fingerprint(&destination)? && before == fingerprint(source)? {
            return Ok(snapshot);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "database changed during diagnostic capture; retry when writes are idle",
    ))
}

fn sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

fn fingerprint(path: &Path) -> std::io::Result<Fingerprints> {
    let mut files = BTreeMap::new();
    for suffix in ["", "-wal", "-journal"] {
        let file = sidecar(path, suffix);
        match std::fs::symlink_metadata(&file) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "diagnostic database and sidecars must be regular files",
                ));
            }
            Ok(_) => {}
            Err(error) if !suffix.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(error),
        }
        let mut input = match File::open(&file) {
            Ok(input) => input,
            Err(error) if !suffix.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(error),
        };
        // Hot rollback journals require recovery; an observational doctor must
        // report this instead of silently ignoring uncommitted changes.
        if suffix == "-journal" && input.metadata()?.len() > 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "rollback journal requires a recovery-capable copy",
            ));
        }
        let mut hasher = blake3::Hasher::new();
        let mut bytes = 0;
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            bytes += u64::try_from(read).map_err(std::io::Error::other)?;
        }
        files.insert(suffix.to_owned(), (bytes, hasher.finalize()));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_changes_are_inconclusive_and_never_silently_accepted() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("db");
        std::fs::write(&database, b"initial").unwrap();
        let mut fired = 0;
        let result = capture_with(&database, || {
            fired += 1;
            std::fs::write(&database, format!("change {fired}"))
        });
        assert_eq!(fired, 3);
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
    #[test]
    fn hot_rollback_journal_is_preserved_and_reported_as_inconclusive() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("db");
        std::fs::write(&database, b"database").unwrap();
        std::fs::write(sidecar(&database, "-journal"), b"pending rollback").unwrap();
        assert_eq!(
            capture(&database).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(
            std::fs::read(sidecar(&database, "-journal")).unwrap(),
            b"pending rollback"
        );
    }

    #[test]
    fn non_regular_inputs_are_rejected_before_opening() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            capture(temp.path()).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        let database = temp.path().join("db");
        std::fs::write(&database, b"database").unwrap();
        std::fs::create_dir(sidecar(&database, "-wal")).unwrap();
        assert_eq!(
            capture(&database).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
