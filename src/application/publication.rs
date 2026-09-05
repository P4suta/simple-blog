//! Publication orchestration: snapshot, compile, verify, then atomic activation.

use std::{sync::Arc, time::Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;
use tracing::Instrument;

use crate::{
    application::{
        ports::{PublicSnapshotRepository, PublicationState, RepositoryError},
        site_compiler::{SiteCompiler, SiteCompilerError},
    },
    observability::codes,
    release::{
        PreparedRelease, ReleaseBuilder, ReleaseError, ReleaseId, ReleasePublisher, ReleaseReader,
        ReleaseStore,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationDisposition {
    Published,
    Unchanged,
}

/// Whether the public site reflects the latest committed state.
///
/// A save is durable the moment its transaction commits; the release that
/// shows it may lag behind while a failed build is retried, and the writer
/// deserves to know which of the two happened.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteState {
    Current,
    Pending,
}

/// Backoff between publication retries after a failed build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetrySchedule {
    pub initial: std::time::Duration,
    pub second: std::time::Duration,
    pub cap: std::time::Duration,
}

impl RetrySchedule {
    pub const DEFAULT: Self = Self {
        initial: std::time::Duration::from_secs(5),
        second: std::time::Duration::from_secs(30),
        cap: std::time::Duration::from_secs(300),
    };

    /// The wait after `failures` consecutive failed attempts (zero-based).
    #[must_use]
    pub const fn delay(self, failures: u32) -> std::time::Duration {
        match failures {
            0 => self.initial,
            1 => self.second,
            _ => self.cap,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationOutcome {
    pub build_id: uuid::Uuid,
    pub disposition: PublicationDisposition,
    pub release_id: ReleaseId,
    pub public_revision: u64,
    pub route_count: usize,
    pub staged_object_count: usize,
}

pub struct PublicationService<R, S> {
    repository: Arc<R>,
    store: Arc<S>,
    compiler: SiteCompiler,
    canonical_origin: String,
}

#[must_use]
pub fn publication_delay(
    state: PublicationState,
    now: DateTime<Utc>,
    maximum_idle: std::time::Duration,
) -> std::time::Duration {
    let Some(next_publish_at) = state.next_publish_at else {
        return maximum_idle;
    };
    if next_publish_at <= now {
        return std::time::Duration::ZERO;
    }
    (next_publish_at - now)
        .to_std()
        .unwrap_or(maximum_idle)
        .min(maximum_idle)
}

impl<R, S> PublicationService<R, S>
where
    R: PublicSnapshotRepository,
    S: ReleaseStore + ReleaseReader,
{
    pub fn new(
        repository: Arc<R>,
        store: Arc<S>,
        compiler: SiteCompiler,
        canonical_origin: &str,
    ) -> Result<Self, PublicationServiceError> {
        let probe = ReleaseBuilder::clean(0, canonical_origin)?;
        Ok(Self {
            repository,
            store,
            compiler,
            canonical_origin: probe.canonical_origin().to_owned(),
        })
    }

    pub async fn publish(
        &self,
        effective_at: DateTime<Utc>,
    ) -> Result<PublicationOutcome, PublicationServiceError> {
        let build_id = uuid::Uuid::new_v4();
        let span = tracing::info_span!(
            "publication.build",
            build_id = %build_id,
            public_revision = tracing::field::Empty,
            release_id = tracing::field::Empty,
            disposition = tracing::field::Empty,
        );
        async move {
            let started = Instant::now();
            tracing::info!(event = "publication.build.started", effective_at = %effective_at);
            let result = self.publish_inner(build_id, effective_at).await;
            match &result {
                Ok(outcome) => {
                    tracing::Span::current()
                        .record("public_revision", outcome.public_revision)
                        .record("release_id", outcome.release_id.as_str())
                        .record("disposition", format_args!("{:?}", outcome.disposition));
                    tracing::info!(
                        event = "publication.build.completed",
                        release_id = %outcome.release_id,
                        public_revision = outcome.public_revision,
                        route_count = outcome.route_count,
                        staged_object_count = outcome.staged_object_count,
                        disposition = ?outcome.disposition,
                        elapsed_ms = started.elapsed().as_millis()
                    );
                }
                Err(error) => {
                    tracing::error!(
                        event = "publication.build.failed",
                        error_code = error.code(),
                        phase = error.phase(),
                        elapsed_ms = started.elapsed().as_millis(),
                        error = %error
                    );
                }
            }
            result
        }
        .instrument(span)
        .await
    }

    /// The compiler this service publishes with, for previews that must
    /// render exactly what a release would.
    #[must_use]
    pub const fn compiler(&self) -> &SiteCompiler {
        &self.compiler
    }

    pub async fn publication_state(&self) -> Result<PublicationState, PublicationServiceError> {
        Ok(self.repository.publication_state().await?)
    }

    async fn publish_inner(
        &self,
        build_id: uuid::Uuid,
        effective_at: DateTime<Utc>,
    ) -> Result<PublicationOutcome, PublicationServiceError> {
        self.repository
            .advance_publication_clock(effective_at)
            .await?;
        let snapshot = self.repository.public_snapshot(effective_at).await?;
        tracing::info!(
            event = "publication.snapshot.loaded",
            public_revision = snapshot.public_revision,
            content_count = snapshot.contents.len(),
            redirect_count = snapshot.redirects.len(),
            media_count = snapshot.media.len()
        );

        let active = self.store.active().await?;
        let active_manifest = if let Some(active) = &active {
            let manifest = self.store.manifest(&active.id).await?;
            verify_objects(self.store.as_ref(), &manifest).await?;
            Some(manifest)
        } else {
            None
        };
        if let (Some(active), Some(manifest)) = (&active, &active_manifest)
            && manifest.public_revision == snapshot.public_revision
            && manifest.canonical_origin == self.canonical_origin
            && manifest.compiler_version == env!("CARGO_PKG_VERSION")
        {
            tracing::debug!(event = "publication.build.unchanged");
            return Ok(PublicationOutcome {
                build_id,
                disposition: PublicationDisposition::Unchanged,
                release_id: active.id.clone(),
                public_revision: snapshot.public_revision,
                route_count: manifest.routes.len(),
                staged_object_count: 0,
            });
        }

        let incremental = active_manifest
            .as_ref()
            .filter(|manifest| manifest.canonical_origin == self.canonical_origin);
        let release = self
            .compiler
            .compile(&snapshot, &self.canonical_origin, incremental)?;
        publish_release(
            self.store.clone(),
            &release,
            active.as_ref().map(|active| &active.id),
        )
        .await?;
        Ok(outcome(build_id, &release))
    }
}

async fn verify_objects<S: ReleaseReader + ?Sized>(
    store: &S,
    manifest: &crate::release::ReleaseManifest,
) -> Result<(), ReleaseError> {
    for object_id in manifest
        .routes
        .values()
        .filter_map(|route| route.object_id())
    {
        store.object(object_id).await?;
    }
    Ok(())
}

async fn publish_release<S: ReleaseStore + ?Sized>(
    store: Arc<S>,
    release: &PreparedRelease,
    expected: Option<&ReleaseId>,
) -> Result<(), ReleaseError> {
    ReleasePublisher::new(store)
        .publish(release, expected)
        .await
}

fn outcome(build_id: uuid::Uuid, release: &PreparedRelease) -> PublicationOutcome {
    PublicationOutcome {
        build_id,
        disposition: PublicationDisposition::Published,
        release_id: release.id.clone(),
        public_revision: release.manifest.public_revision,
        route_count: release.manifest.routes.len(),
        staged_object_count: release.objects.len(),
    }
}

#[derive(Debug, Error)]
pub enum PublicationServiceError {
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error(transparent)]
    Compiler(#[from] SiteCompilerError),
    #[error(transparent)]
    Release(#[from] ReleaseError),
}

impl PublicationServiceError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Repository(_) => codes::PUBLICATION_REPOSITORY_FAILED,
            Self::Compiler(_) => codes::PUBLICATION_COMPILE_FAILED,
            Self::Release(_) => codes::PUBLICATION_RELEASE_STORE_FAILED,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> &'static str {
        match self {
            Self::Repository(_) => "snapshot",
            Self::Compiler(_) => "compile",
            Self::Release(_) => "store",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_publication_failure_has_a_stable_code_and_phase() {
        let cases = [
            (
                PublicationServiceError::Repository(RepositoryError::Storage("offline".into())),
                "publication_repository_failed",
                "snapshot",
            ),
            (
                PublicationServiceError::Compiler(SiteCompilerError::SearchIndex("invalid".into())),
                "publication_compile_failed",
                "compile",
            ),
            (
                PublicationServiceError::Release(ReleaseError::Store("unavailable".into())),
                "publication_release_store_failed",
                "store",
            ),
        ];

        for (error, code, phase) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(error.phase(), phase);
        }
    }
}
