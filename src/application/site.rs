use std::sync::Arc;

/// The stylesheet a fresh installation starts with; Settings can put it back.
pub const DEFAULT_THEME_CSS: &str = include_str!("../../static/default-theme.css");

use chrono::{DateTime, Utc};

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
}
