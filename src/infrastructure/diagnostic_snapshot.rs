//! A stable observational copy, never an immutable assertion about a live database.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::Path,
};

type Fingerprints = BTreeMap<String, (u64, blake3::Hash)>;

/// Never block on a FIFO, and never follow a link where a regular file was
/// expected. Named so the two flags are one asserted value rather than an
/// expression no test can see.
#[cfg(unix)]
const DIAGNOSTIC_OPEN_FLAGS: i32 = libc::O_NONBLOCK | libc::O_NOFOLLOW;

/// FILE_FLAG_OPEN_REPARSE_POINT: inspect the opened link itself.
#[cfg(windows)]
const DIAGNOSTIC_OPEN_FLAGS: u32 = 0x0020_0000;

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
        for (suffix, (length, _)) in &before {
            let mut input = open_regular(&sidecar(source, suffix))?;
            let mut output = File::create(sidecar(&destination, suffix))?;
            transfer(&mut input, &mut output, *length)?;
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
    fingerprint_with(path, open_regular)
}

fn open_regular(path: &Path) -> std::io::Result<File> {
    open_regular_with(path, || Ok(()))
}

fn open_regular_with(
    path: &Path,
    before_open: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<File> {
    let expected = std::fs::symlink_metadata(path)?;
    if !expected.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "diagnostic inputs must be regular files",
        ));
    }
    before_open()?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(DIAGNOSTIC_OPEN_FLAGS);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.custom_flags(DIAGNOSTIC_OPEN_FLAGS);
    }
    let input = options.open(path)?;
    let opened = input.metadata()?;
    if !opened.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "diagnostic input changed type before opening",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if expected.dev() != opened.dev() || expected.ino() != opened.ino() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "diagnostic input changed before opening",
            ));
        }
    }
    Ok(input)
}

fn fingerprint_with(
    path: &Path,
    mut open: impl FnMut(&Path) -> std::io::Result<File>,
) -> std::io::Result<Fingerprints> {
    let mut files = BTreeMap::new();
    for suffix in ["", "-wal", "-journal"] {
        let file = sidecar(path, suffix);
        let mut input = match open(&file) {
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
        let length = input.metadata()?.len();
        let hash = transfer(&mut input, &mut std::io::sink(), length)?;
        files.insert(suffix.to_owned(), (length, hash));
    }
    Ok(files)
}

fn transfer(
    input: &mut impl Read,
    output: &mut impl Write,
    length: u64,
) -> std::io::Result<blake3::Hash> {
    // A concurrently growing file must not extend an observational scan forever.
    // Read one extra byte to distinguish growth from a stable exact-size input.
    let limit = length
        .checked_add(1)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
    let mut input = input.take(limit);
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        bytes += u64::try_from(read).map_err(std::io::Error::other)?;
    }
    if bytes != length {
        return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
    }
    Ok(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growing_or_truncated_inputs_are_inconclusive_and_reads_are_bounded() {
        for actual in [7, 9, 128] {
            let mut input = std::io::Cursor::new(vec![1; actual]);
            let mut output = Vec::new();
            assert_eq!(
                transfer(&mut input, &mut output, 8).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert!(
                input.position() <= 9,
                "read must stop at the initial length plus one byte"
            );
        }
        let mut output = Vec::new();
        assert_eq!(
            transfer(&mut &b"database"[..], &mut output, 8).unwrap(),
            blake3::hash(b"database")
        );
        assert_eq!(output, b"database");
    }

    #[test]
    fn only_a_missing_optional_sidecar_can_be_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("db");
        std::fs::write(&database, b"database").unwrap();
        for suffix in ["", "-wal", "-journal"] {
            for kind in [
                std::io::ErrorKind::NotFound,
                std::io::ErrorKind::PermissionDenied,
            ] {
                let mut fired = 0;
                let result = fingerprint_with(&database, |path| {
                    if path == sidecar(&database, suffix) {
                        fired += 1;
                        Err(std::io::Error::from(kind))
                    } else {
                        open_regular(path)
                    }
                });
                assert_eq!(fired, 1);
                if suffix.is_empty() || kind != std::io::ErrorKind::NotFound {
                    assert_eq!(result.unwrap_err().kind(), kind);
                } else {
                    let files = result.unwrap();
                    assert_eq!(files.len(), 1);
                    assert_eq!(files[""].0, 8);
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_input_replaced_by_a_fifo_or_symlink_cannot_block_or_follow_the_replacement() {
        for fifo in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let database = temp.path().join("db");
            std::fs::write(&database, b"database").unwrap();
            let outside = temp.path().join("outside");
            std::fs::write(&outside, b"private").unwrap();
            let mut fired = false;
            let result = open_regular_with(&database, || {
                std::fs::remove_file(&database)?;
                if fifo {
                    assert!(
                        std::process::Command::new("mkfifo")
                            .arg(&database)
                            .status()?
                            .success()
                    );
                } else {
                    std::os::unix::fs::symlink(&outside, &database)?;
                }
                fired = true;
                Ok(())
            });
            assert!(fired);
            assert!(result.is_err());
            assert_eq!(std::fs::read(outside).unwrap(), b"private");
        }
    }
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
    fn an_empty_rollback_journal_does_not_hide_a_stable_database() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("db");
        std::fs::write(&database, b"database").unwrap();
        std::fs::write(sidecar(&database, "-journal"), []).unwrap();
        let snapshot = capture(&database).unwrap();
        assert_eq!(
            std::fs::read(snapshot.path().join("database.sqlite3")).unwrap(),
            b"database"
        );
        assert_eq!(
            std::fs::metadata(sidecar(&database, "-journal"))
                .unwrap()
                .len(),
            0
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

#[cfg(test)]
mod regular_file_tests {
    use super::*;

    /// The flags are the whole defence against a FIFO that never returns and
    /// a link that points somewhere else, so they are asserted rather than
    /// trusted to an expression nothing reads.
    #[cfg(unix)]
    #[test]
    fn a_diagnostic_input_is_opened_without_blocking_and_without_following() {
        assert_eq!(DIAGNOSTIC_OPEN_FLAGS, libc::O_NONBLOCK | libc::O_NOFOLLOW);
        assert_ne!(DIAGNOSTIC_OPEN_FLAGS & libc::O_NONBLOCK, 0);
        assert_ne!(DIAGNOSTIC_OPEN_FLAGS & libc::O_NOFOLLOW, 0);
    }

    #[cfg(windows)]
    #[test]
    fn a_diagnostic_input_is_opened_as_the_link_itself() {
        assert_eq!(DIAGNOSTIC_OPEN_FLAGS, 0x0020_0000);
    }

    #[test]
    fn only_a_regular_file_is_a_diagnostic_input() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("database.sqlite3");
        std::fs::write(&file, b"content").unwrap();
        open_regular_with(&file, || Ok(())).unwrap();

        let error = open_regular_with(temp.path(), || Ok(())).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        assert!(open_regular_with(&temp.path().join("absent"), || Ok(())).is_err());
    }

    #[test]
    fn a_failure_before_the_open_is_not_swallowed() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("database.sqlite3");
        std::fs::write(&file, b"content").unwrap();

        let error = open_regular_with(&file, || {
            Err(std::io::Error::other("synthetic pre-open failure"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("synthetic pre-open failure"));
    }

    /// The identity check is what makes the descriptor the same file that was
    /// inspected: a replacement in the window has to be refused even when the
    /// new file sits on the same device.
    #[cfg(unix)]
    #[test]
    fn a_diagnostic_input_replaced_before_the_open_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("database.sqlite3");
        std::fs::write(&file, b"original").unwrap();
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, b"substituted").unwrap();

        let error = open_regular_with(&file, || {
            std::fs::rename(&replacement, &file)?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }
}
