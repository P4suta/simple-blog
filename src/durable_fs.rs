use std::{io, path::Path};

/// Flushes directory-entry changes when the platform exposes a documented contract.
///
/// Windows documents `FlushFileBuffers` for writable file handles, but not for directory
/// handles. On Windows we therefore validate that the path is still a directory and report a
/// best-effort boundary instead of calling an unsupported operation that fails every write.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    if !std::fs::metadata(path)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a directory: {}", path.display()),
        ));
    }

    #[cfg(windows)]
    {
        tracing::debug!(
            event = "filesystem.directory_sync.best_effort",
            path = %path.display(),
            "Windows has no documented directory-buffer flush operation"
        );
        Ok(())
    }

    #[cfg(not(windows))]
    {
        std::fs::File::open(path)?.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncs_an_existing_directory() {
        let directory = tempfile::tempdir().expect("temporary directory");

        sync_directory(directory.path()).expect("directory sync");
    }

    #[test]
    fn rejects_a_regular_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let file = directory.path().join("not-a-directory");
        std::fs::write(&file, b"contents").expect("write fixture");

        let error = sync_directory(&file).expect_err("regular file must be rejected");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
