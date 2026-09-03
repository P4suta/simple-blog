use std::sync::Arc;

/// The stylesheet a fresh installation starts with; Settings can put it back.
pub const DEFAULT_THEME_CSS: &str = include_str!("../../static/default-theme.css");

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::{
    application::ports::{RepositoryError, SiteRepository},
    domain::theme::{NavigationItem, SiteSettings, validate_navigation},
};

#[derive(Clone)]
pub struct SiteService {
    repository: Arc<dyn SiteRepository>,
}

impl SiteService {
    pub fn new(repository: Arc<dyn SiteRepository>) -> Self {
        Self { repository }
    }

    #[tracing::instrument(name = "site.update", skip_all)]
    pub async fn update(
        &self,
        settings: SiteSettings,
        navigation: Vec<NavigationItem>,
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let settings = settings
            .validated()
            .map_err(|error| RepositoryError::Validation(error.to_string()))?;
        let navigation = validate_navigation(navigation)
            .map_err(|error| RepositoryError::Validation(error.to_string()))?;
        self.repository
            .save_configuration(&settings, &navigation, now)
            .await
    }

    /// Adopts the browser's zone offered during setup, but only once: while
    /// the site still sits on the `UTC` default and the candidate is a real
    /// zone other than UTC itself. Answers whether anything changed.
    #[tracing::instrument(name = "site.adopt_timezone", skip(self))]
    pub async fn adopt_timezone_once(
        &self,
        candidate: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let Ok(zone) = candidate.trim().parse::<Tz>() else {
            return Ok(false);
        };
        if zone == Tz::UTC || zone.name().starts_with("Etc/") {
            return Ok(false);
        }
        let settings = self.repository.site_settings().await?;
        if settings.timezone != "UTC" {
            return Ok(false);
        }
        let navigation = self.repository.navigation().await?;
        let settings = SiteSettings {
            timezone: zone.name().to_owned(),
            ..settings
        }
        .validated()
        .map_err(|error| RepositoryError::Validation(error.to_string()))?;
        self.repository
            .save_configuration(&settings, &navigation, now)
            .await?;
        Ok(true)
    }
}
