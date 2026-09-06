//! The human account of a failed command.
//!
//! `tracing` keeps the machine account and is emitted for every failure. This
//! is the account a person reads: what was attempted, why it failed, the
//! stable code when the failure carries one, and one thing to try next. It
//! stands down when `SIMPLE_BLOG_LOG_FORMAT=json` declares stderr a machine
//! stream, so that promise is never broken by an explanation.

use std::io::Write;

use crate::{
    application::publication::PublicationServiceError, materialize::MaterializeError,
    operations::OperationError,
};

/// The command ran and failed. A command that succeeds returns normally, so
/// zero needs no name here; `--help` and the README document all three codes.
pub const FAILURE: i32 = 1;
/// The command never ran: it could not be understood, or diagnostics could
/// not be started. `clap` reports a usage error with the same code.
pub const UNUSABLE: i32 = 2;

/// Writes the human account of one failed command.
///
/// # Errors
///
/// Returns the write failure. A caller reporting a failure has nothing better
/// to do with it than ignore it.
pub fn render(error: &anyhow::Error, into: &mut dyn Write) -> std::io::Result<()> {
    writeln!(into, "simple-blog: {error}")?;
    for cause in error.chain().skip(1) {
        writeln!(into, "  caused by: {cause}")?;
    }
    if let Some(code) = error
        .downcast_ref::<PublicationServiceError>()
        .map(PublicationServiceError::code)
    {
        writeln!(into, "  error code: {code}")?;
    }
    writeln!(into, "  next: {}", next_step(error))
}

/// The one step most likely to get the operator past this failure.
fn next_step(error: &anyhow::Error) -> &'static str {
    if let Some(step) = typed_step(error) {
        return step;
    }
    let summary = format!("{error:#}");
    if summary.contains("installation is unhealthy") {
        "each check above names what it found; repair it and run `simple-blog doctor` again"
    } else if summary.contains("installation is not initialized") {
        "run `simple-blog init` here, or point --data-dir at an existing installation"
    } else {
        "read the cause above; `simple-blog doctor` inspects the installation itself"
    }
}

/// The step a typed failure in the chain names for itself.
fn typed_step(error: &anyhow::Error) -> Option<&'static str> {
    if let Some(MaterializeError::OutputExists(_)) = error.downcast_ref::<MaterializeError>() {
        return Some("choose an --output path that does not exist yet, or remove that one");
    }
    match error.downcast_ref::<OperationError>()? {
        OperationError::DestinationExists => {
            Some("pass --force to replace the installation in this data directory")
        }
        OperationError::ExportExists(_) => Some("choose an --output path that does not exist yet"),
        OperationError::Io(_) => Some("check that the path above exists and can be read"),
        OperationError::PortableOriginMismatch { .. } => {
            Some("import into an empty data directory, or set --public-url to the archive's origin")
        }
        OperationError::Migration { .. } => {
            Some("the pre-migration backup named above is intact; `simple-blog restore` reads it")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::anyhow;

    use super::*;
    use crate::{application::ports::RepositoryError, operations::OperationError};

    fn rendered(error: &anyhow::Error) -> String {
        let mut written = Vec::new();
        render(error, &mut written).expect("a vector never fails to accept bytes");
        String::from_utf8(written).expect("the report is UTF-8")
    }

    #[test]
    fn the_report_opens_with_the_operation_and_lists_every_cause() {
        let error = anyhow!("no such file").context("could not import ./writng");
        let report = rendered(&error);

        assert!(report.starts_with("simple-blog: could not import ./writng\n"));
        assert!(report.contains("\n  caused by: no such file\n"));
        assert!(report.contains("\n  next: read the cause above"));
        assert!(!report.contains("error code:"));
    }

    #[test]
    fn a_publication_failure_reports_its_stable_code() {
        let error = anyhow::Error::new(PublicationServiceError::Repository(
            RepositoryError::NotFound,
        ))
        .context("could not build public release");
        let report = rendered(&error);

        assert!(report.contains("\n  error code: publication_repository_failed\n"));
    }

    #[test]
    fn an_occupied_output_path_says_to_choose_another() {
        let error = anyhow::Error::new(MaterializeError::OutputExists(PathBuf::from("./public")))
            .context("could not materialize ./public");

        assert!(rendered(&error).contains("\n  next: choose an --output path"));
    }

    #[test]
    fn every_operation_failure_that_names_a_remedy_offers_it() {
        for (error, expected) in [
            (OperationError::DestinationExists, "pass --force"),
            (
                OperationError::ExportExists("./writing".into()),
                "choose an --output path",
            ),
            (
                OperationError::PortableOriginMismatch {
                    archive_origin: "https://a.example".into(),
                    configured_origin: "https://b.example".into(),
                },
                "set --public-url",
            ),
            (
                OperationError::Migration {
                    message: "checksum mismatch".into(),
                    backup: PathBuf::from("./data/backups/pre-migration.tar.zst"),
                },
                "`simple-blog restore` reads it",
            ),
        ] {
            let wrapped = anyhow::Error::new(error).context("could not run the operation");
            let report = rendered(&wrapped);
            assert!(report.contains(expected), "{report}");
        }
    }

    #[test]
    fn an_uninitialized_installation_is_told_to_initialize() {
        let error = anyhow!("installation is not initialized; run `simple-blog init`");

        assert!(rendered(&error).contains("\n  next: run `simple-blog init`"));
    }

    #[test]
    fn an_unhealthy_installation_is_pointed_back_at_the_checks() {
        let error = anyhow!("installation is unhealthy");

        assert!(rendered(&error).contains("\n  next: each check above"));
    }
}
