//! Provider-neutral extension boundary. Tool integrations are independent of
//! model providers and only receive calls after AgentRuntime permission checks.
use crate::{
    domain::{Tool, ToolCall, ToolResult},
    error::AppResult,
};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait ToolIntegration: Send + Sync {
    fn owns(&self, name: &str) -> bool;
    async fn definitions(&self, project_id: &str) -> AppResult<Vec<Tool>>;
    async fn execute(&self, project_id: &str, call: &ToolCall) -> ToolResult;
}

#[derive(Default, Clone)]
pub struct ExtensionRegistry {
    integrations: Vec<Arc<dyn ToolIntegration>>,
}

impl ExtensionRegistry {
    pub fn with(mut self, integration: Arc<dyn ToolIntegration>) -> Self {
        self.integrations.push(integration);
        self
    }

    pub async fn definitions(&self, project_id: &str) -> AppResult<Vec<Tool>> {
        let mut tools = Vec::new();
        for integration in &self.integrations {
            tools.extend(integration.definitions(project_id).await?);
        }
        Ok(tools)
    }

    pub fn owns(&self, name: &str) -> bool {
        self.integrations
            .iter()
            .any(|integration| integration.owns(name))
    }

    pub async fn execute(&self, project_id: &str, call: &ToolCall) -> Option<ToolResult> {
        for integration in &self.integrations {
            if integration.owns(&call.name) {
                return Some(integration.execute(project_id, call).await);
            }
        }
        None
    }
}
