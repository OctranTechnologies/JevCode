use crate::{
    domain::{Provider, ProviderProtocol},
    error::{AppError, AppResult},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfig {
    pub providers: Vec<Provider>,
    pub max_tool_rounds: u32,
    /// Public OAuth client ID for Google's registered desktop application.
    #[serde(default, rename = "googleOAuthClientId")]
    pub google_oauth_client_id: Option<String>,
    /// Legacy compatibility field. Gemini OAuth billing projects are selected
    /// explicitly per Google account and this value is never used for requests.
    #[serde(default)]
    pub google_cloud_project_id: Option<String>,
}

impl AppConfig {
    pub fn load(directory: &Path) -> AppResult<Self> {
        let path = directory.join("config.json");
        let config: Self = if path.exists() {
            serde_json::from_str(&std::fs::read_to_string(path)?).map_err(|_| {
                AppError::new(
                    "configuration",
                    "config.json is invalid. Restore the example configuration and restart.",
                )
            })?
        } else {
            serde_json::from_str(include_str!("providers/defaults.json"))?
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> AppResult<()> {
        if !(1..=32).contains(&self.max_tool_rounds) || self.providers.is_empty() {
            return Err(AppError::new(
                "configuration",
                "Configure providers and a tool-round limit between 1 and 32.",
            ));
        }
        if self
            .google_oauth_client_id
            .as_deref()
            .is_some_and(|id| id.len() > 256 || !id.ends_with(".apps.googleusercontent.com"))
            || self
                .google_cloud_project_id
                .as_deref()
                .is_some_and(|project| !valid_google_cloud_project_id(project))
        {
            return Err(AppError::new(
                "configuration",
                "Google OAuth settings must contain a desktop OAuth client ID and a valid Cloud project ID.",
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for provider in &self.providers {
            if provider.id.is_empty()
                || !provider
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || !ids.insert(&provider.id)
                || (provider.models.is_empty() && provider.id != "openai")
            {
                return Err(AppError::new(
                    "configuration",
                    "Provider IDs must be unique and each provider must have a model, except OpenAI which loads its account-specific catalog.",
                ));
            }
            let mut models = std::collections::HashSet::new();
            for model in &provider.models {
                if model.provider != provider.id
                    || model.id.is_empty()
                    || !model
                        .id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
                    || !models.insert(&model.id)
                {
                    return Err(AppError::new("configuration", "Model IDs must be unique within their provider and contain letters, numbers, dashes, dots or underscores."));
                }
            }
            if provider.protocol != ProviderProtocol::Preview {
                let url = provider
                    .base_url
                    .as_ref()
                    .and_then(|url| reqwest::Url::parse(url).ok())
                    .ok_or_else(|| {
                        AppError::new(
                            "configuration",
                            "Every remote provider needs an HTTPS base URL.",
                        )
                    })?;
                if url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(AppError::new("configuration", "Provider URLs must use HTTPS and contain no credentials, query or fragment."));
                }
            }
        }
        Ok(())
    }

    pub fn provider(&self, id: &str) -> AppResult<&Provider> {
        self.providers
            .iter()
            .find(|provider| provider.id == id)
            .ok_or_else(|| AppError::new("unknown_provider", "Choose a configured provider."))
    }
}

fn valid_google_cloud_project_id(value: &str) -> bool {
    (6..=30).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_validate_and_insecure_endpoints_fail() {
        let mut config: AppConfig =
            serde_json::from_str(include_str!("providers/defaults.json")).unwrap();
        config.validate().unwrap();
        assert!(config.providers[1].models.is_empty());
        config.providers[1].base_url = Some("http://example.com".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn google_cloud_project_id_uses_google_project_id_format() {
        assert!(valid_google_cloud_project_id("team-project-123"));
        assert!(!valid_google_cloud_project_id("short"));
        assert!(!valid_google_cloud_project_id("-team-123"));
        assert!(!valid_google_cloud_project_id("team-123-"));
        assert!(!valid_google_cloud_project_id("Team-project"));
    }
}
