//! A durable intent record bridges the two directory renames. Previous data is
//! never deleted here, including when recovery or synchronization fails.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::durable_fs::sync_directory;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u8,
    destination: String,
    staging: String,
    previous: String,
    had_previous: bool,
}

/// An OS lock is released on process death. Its persistent, empty lock file
/// deliberately lives beside the installation, so renames cannot replace it.
pub struct InstallationLease {
    _file: File,
}

impl InstallationLease {
    pub fn acquire(destination: &Path) -> io::Result<Self> {
        Self::acquire_kind(destination, "lock")
    }

    fn acquire_kind(destination: &Path, kind: &str) -> io::Result<Self> {
        let path = sibling_path(destination, kind)?;
        std::fs::create_dir_all(parent(destination))?;
        reject_link(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock().map_err(|error| io::Error::other(format!("installation is in use; stop its server or other command before replacing data: {error}")))?;
        Ok(Self { _file: file })
    }
}

pub fn pending(destination: &Path) -> io::Result<bool> {
    sibling_path(destination, "activation.json")?.try_exists()
}

/// Call while holding the installation lease, before loading its configuration.
///
/// An interrupted replacement rolls back; an already installed replacement is
/// kept. Every branch is idempotent and retains the previous installation.
pub fn recover(destination: &Path) -> io::Result<()> {
    let journal = sibling_path(destination, "activation.json")?;
    if !journal.try_exists()? {
        return Ok(());
    }
    let _activation = InstallationLease::acquire_kind(destination, "activation.lock")?;
    reject_link(&journal)?;
    let mut bytes = Vec::new();
    File::open(&journal)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid("activation record is too large"));
    }
    let intent: Intent = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    validate_intent(&intent, destination)?;
    let staging = parent(destination).join(&intent.staging);
    let previous = parent(destination).join(&intent.previous);
    for path in [destination, &staging, &previous] {
        reject_link(path)?;
    }
    let exists = destination.try_exists()?;
    let staged = staging.try_exists()?;
    let retained = previous.try_exists()?;
    if !exists && retained && intent.had_previous {
        std::fs::rename(&previous, destination)?;
        tracing::info!(event = "installation.recovery.rolled_back");
    } else if exists && !staged && (retained == intent.had_previous) {
        tracing::info!(event = "installation.recovery.completed");
    } else if staged && !retained && (exists == intent.had_previous) {
        // No rename happened. Leave the complete staged copy for inspection.
        tracing::info!(event = "installation.recovery.not_activated");
    } else {
        return Err(invalid(
            "ambiguous activation state; preserve all directories and inspect the activation record",
        ));
    }
    sync_directory(parent(destination))?;
    std::fs::remove_file(journal)?;
    sync_directory(parent(destination))
}

pub(super) fn activate(
    staging: &Path,
    destination: &Path,
    replace: bool,
) -> io::Result<Option<PathBuf>> {
    let _activation = InstallationLease::acquire_kind(destination, "activation.lock")?;
    if pending(destination)? {
        return Err(invalid("recover pending activation before replacing data"));
    }
    if parent(staging).canonicalize()? != parent(destination).canonicalize()? {
        return Err(invalid("staging must be beside the destination"));
    }
    for path in [staging, destination] {
        reject_link(path)?;
    }
    let had_previous = destination.try_exists()?;
    if had_previous
        && (!destination.is_dir() || (!replace && std::fs::read_dir(destination)?.next().is_some()))
    {
        return Err(invalid(
            "destination is not empty; explicit replacement is required",
        ));
    }
    let intent = Intent {
        version: 1,
        destination: name(destination)?,
        staging: name(staging)?,
        previous: format!(".simple-blog-previous-{}", Uuid::new_v4()),
        had_previous,
    };
    validate_intent(&intent, destination)?;
    sync_tree(staging)?;
    let journal = sibling_path(destination, "activation.json")?;
    let temporary = parent(destination).join(format!(".simple-blog-intent-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(&intent).map_err(io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, &journal)?;
    sync_directory(parent(destination))?;
    tracing::info!(event = "installation.activation.prepared");
    let previous = parent(destination).join(&intent.previous);
    if had_previous {
        std::fs::rename(destination, &previous)?;
        sync_directory(parent(destination))?;
    }
    tracing::info!(event = "installation.activation.previous_retained");
    std::fs::rename(staging, destination)?;
    sync_directory(parent(destination))?;
    tracing::info!(event = "installation.activation.installed");
    std::fs::remove_file(journal)?;
    sync_directory(parent(destination))?;
    Ok(had_previous.then_some(previous))
}

pub(super) fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    reject_link(source)?;
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.is_file() {
        std::fs::copy(source, destination)?;
    } else if metadata.is_dir() {
        std::fs::create_dir(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        return Err(invalid("replacement data contains a special file"));
    }
    Ok(())
}

fn sync_tree(path: &Path) -> io::Result<()> {
    reject_link(path)?;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
        sync_directory(path)
    } else {
        OpenOptions::new().write(true).open(path)?.sync_all()
    }
}

fn validate_intent(intent: &Intent, destination: &Path) -> io::Result<()> {
    if intent.version != 1
        || intent.destination != name(destination)?
        || !safe_name(&intent.staging)
        || !safe_name(&intent.previous)
        || !intent.staging.starts_with(".simple-blog-")
        || !intent.staging.ends_with(".staging")
        || intent
            .previous
            .strip_prefix(".simple-blog-previous-")
            .and_then(|suffix| Uuid::parse_str(suffix).ok())
            .is_none()
        || intent.destination == intent.staging
        || intent.destination == intent.previous
    {
        return Err(invalid("invalid activation record"));
    }
    Ok(())
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '\\', ':', '\0'])
        && Path::new(value).components().count() == 1
        && matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
}

fn name(path: &Path) -> io::Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| safe_name(value))
        .map(str::to_owned)
        .ok_or_else(|| invalid("destination must have a safe directory name"))
}

fn sibling_path(destination: &Path, suffix: &str) -> io::Result<PathBuf> {
    // Existing aliases must coordinate on the actual installation. Before a
    // directory exists, Windows spelling variants still name the same lock.
    let canonical = destination.canonicalize().ok();
    let location = canonical.as_deref().unwrap_or(destination);
    let filename = location
        .file_name()
        .ok_or_else(|| invalid("installation must have a directory name"))?;
    let key = if cfg!(windows) {
        filename.to_string_lossy().to_lowercase().into_bytes()
    } else {
        filename.as_encoded_bytes().to_vec()
    };
    let hash = blake3::hash(&key).to_hex();
    Ok(parent(location).join(format!(".simple-blog-{}.{}", &hash[..16], suffix)))
}

fn reject_link(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(invalid(
            "symbolic links are not permitted in an installation replacement",
        )),
        Ok(metadata) if !metadata.is_file() && !metadata.is_dir() => Err(invalid(
            "special files are not permitted in an installation replacement",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::BufRead,
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    use tracing_subscriber::{Layer, layer::SubscriberExt};

    struct Barrier(String);
    impl<S: tracing::Subscriber> Layer<S> for Barrier {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _context: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Visitor(String);
            impl tracing::field::Visit for Visitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "event" {
                        self.0 = format!("{value:?}").trim_matches('"').to_owned();
                    }
                }
            }
            let mut visitor = Visitor(String::new());
            event.record(&mut visitor);
            if visitor.0 == self.0 {
                println!("FAULT_REACHED:{}", self.0);
                std::io::stdout().flush().unwrap();
                loop {
                    std::thread::park();
                }
            }
        }
    }

    #[test]
    #[ignore = "subprocess entry point, invoked and killed by crash_boundaries_recover_idempotently"]
    fn crash_child() {
        let destination = PathBuf::from(std::env::var_os("SIMPLE_BLOG_TEST_DESTINATION").unwrap());
        let phase = std::env::var("SIMPLE_BLOG_TEST_BARRIER").unwrap();
        let _subscriber =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(Barrier(phase)));
        let _lease = InstallationLease::acquire(&destination).unwrap();
        activate(
            &parent(&destination).join(".simple-blog-test.staging"),
            &destination,
            true,
        )
        .unwrap();
    }

    #[test]
    fn crash_boundaries_recover_idempotently() {
        for (phase, expected) in [
            ("prepared", "old"),
            ("previous_retained", "old"),
            ("installed", "new"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let destination = temp.path().join("site");
            let staging = temp.path().join(".simple-blog-test.staging");
            for (path, bytes) in [(&destination, "old"), (&staging, "new")] {
                std::fs::create_dir(path).unwrap();
                std::fs::write(path.join("content"), bytes).unwrap();
            }
            let event = format!("installation.activation.{phase}");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "operations::activation::tests::crash_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("SIMPLE_BLOG_TEST_DESTINATION", &destination)
                .env("SIMPLE_BLOG_TEST_BARRIER", &event)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                {
                    if line.starts_with("FAULT_REACHED:") {
                        let _ = sender.send(line);
                        break;
                    }
                }
            });
            let reached = receiver.recv_timeout(Duration::from_secs(30));
            child.kill().unwrap();
            child.wait().unwrap();
            assert_eq!(
                reached.unwrap(),
                format!("FAULT_REACHED:{event}"),
                "injection must actually fire"
            );
            assert!(pending(&destination).unwrap());
            let _lease = InstallationLease::acquire(&destination).unwrap();
            recover(&destination).unwrap();
            recover(&destination).unwrap();
            assert_eq!(
                std::fs::read_to_string(destination.join("content")).unwrap(),
                expected
            );
            assert!(!pending(&destination).unwrap());
            // The committed replacement must retain the original installation.
            if expected == "new" {
                let previous = std::fs::read_dir(temp.path())
                    .unwrap()
                    .map(Result::unwrap)
                    .find(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".simple-blog-previous-")
                    })
                    .unwrap();
                assert_eq!(
                    std::fs::read_to_string(previous.path().join("content")).unwrap(),
                    "old"
                );
            }
        }
    }

    #[test]
    fn competing_commands_are_rejected_and_lease_is_reusable() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("site");
        let first = InstallationLease::acquire(&destination).unwrap();
        assert!(InstallationLease::acquire(&destination).is_err());
        drop(first);
        InstallationLease::acquire(&destination).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn case_variants_cannot_bypass_the_installation_lease() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("CaseSite");
        let alternative = temp.path().join("casesite");
        let _lease = InstallationLease::acquire(&destination).unwrap();
        assert!(InstallationLease::acquire(&alternative).is_err());
        std::fs::create_dir(&destination).unwrap();
        assert!(InstallationLease::acquire(&alternative).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_alias_uses_the_existing_installation_lease() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("site");
        std::fs::create_dir(&destination).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&destination, &alias).unwrap();
        let _lease = InstallationLease::acquire(&destination).unwrap();
        assert!(InstallationLease::acquire(&alias).is_err());
    }

    #[test]
    fn malformed_intent_never_moves_or_removes_data() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("site");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("keep"), "keep").unwrap();
        let journal = sibling_path(&destination, "activation.json").unwrap();
        for malicious in [
            "../outside",
            "C:\\outside",
            "site",
            ".simple-blog-test.staging/../../outside",
        ] {
            let intent = Intent {
                version: 1,
                destination: "site".into(),
                staging: malicious.into(),
                previous: format!(".simple-blog-previous-{}", Uuid::new_v4()),
                had_previous: true,
            };
            std::fs::write(&journal, serde_json::to_vec(&intent).unwrap()).unwrap();
            assert!(recover(&destination).is_err());
            assert!(journal.exists());
            assert_eq!(
                std::fs::read_to_string(destination.join("keep")).unwrap(),
                "keep"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn special_activation_files_are_rejected_before_blocking_io() {
        use std::os::unix::fs::FileTypeExt;

        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("site");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("keep"), "original").unwrap();
        for suffix in ["activation.json", "lock"] {
            let path = sibling_path(&destination, suffix).unwrap();
            assert!(
                Command::new("mkfifo")
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                std::fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_fifo()
            );
            // This first assertion fails immediately in the old implementation,
            // before an attempted FIFO read could block the test process.
            assert_eq!(
                reject_link(&path).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            if suffix == "activation.json" {
                assert_eq!(
                    recover(&destination).unwrap_err().kind(),
                    io::ErrorKind::InvalidData
                );
            } else {
                assert!(InstallationLease::acquire(&destination).is_err());
            }
            assert!(
                std::fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_fifo()
            );
            std::fs::remove_file(path).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(destination.join("keep")).unwrap(),
            "original"
        );
    }
}
