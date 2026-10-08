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
        let mut ids = std::collections::HashSet::new();
        for provider in &self.providers {
            if provider.id.is_empty()
                || !provider
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || !ids.insert(&provider.id)
                || provider.models.is_empty()
            {
                return Err(AppError::new(
                    "configuration",
                    "Provider IDs must be unique and each provider must have a model.",
                ));
            }
            let mut models = std::collections::HashSet::new();
            for model in &provider.models {
                if model.provider_id != provider.id
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_validate_and_insecure_endpoints_fail() {
        let mut config: AppConfig =
            serde_json::from_str(include_str!("providers/defaults.json")).unwrap();
        config.validate().unwrap();
        config.providers[1].base_url = Some("http://example.com".into());
        assert!(config.validate().is_err());
    }
}
