use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    Preview,
    OpenAiResponses,
    OpenAiChat,
    Anthropic,
    Gemini,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: ProviderProtocol,
    pub base_url: Option<String>,
    pub models: Vec<Model>,
    #[serde(default)]
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub supports_tools: bool,
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
    AwaitingPermission,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub project_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub title: String,
    pub status: SessionStatus,
    pub messages: Vec<AgentMessage>,
    pub permission_policy: PermissionPolicy,
    pub pending_tool_call: Option<ToolCall>,
    pub queued_tool_calls: Vec<ToolCall>,
    pub created_at: String,
    pub updated_at: String,
    pub error: Option<String>,
    pub tool_rounds: u32,
}

impl AgentSession {
    /// Keep provider history valid when a run is interrupted or cancelled.
    pub fn close_pending_tools(&mut self, reason: &str) {
        let completed: std::collections::HashSet<_> = self
            .messages
            .iter()
            .filter_map(|message| {
                message
                    .tool_result
                    .as_ref()
                    .map(|result| result.tool_call_id.clone())
            })
            .collect();
        let calls: Vec<_> = self
            .messages
            .iter()
            .flat_map(|message| message.tool_calls.clone())
            .filter(|call| !completed.contains(&call.id))
            .collect();
        for call in calls {
            let result = ToolResult {
                tool_call_id: call.id,
                name: call.name,
                content: reason.into(),
                is_error: true,
                duration_ms: 0,
            };
            let mut message = AgentMessage::text(MessageRole::Tool, result.content.clone());
            message.tool_result = Some(result);
            self.messages.push(message);
        }
        self.pending_tool_call = None;
        self.queued_tool_calls.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_result: Option<ToolResult>,
    pub created_at: String,
    #[serde(default)]
    pub provider_data: Option<Value>,
}

impl AgentMessage {
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            id: id(),
            role,
            content: content.into(),
            tool_calls: vec![],
            tool_result: None,
            created_at: now(),
            provider_data: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    ReadFiles,
    Git,
    WriteFiles,
    Shell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub category: ToolCategory,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub id: String,
    pub session_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub duration_ms: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPolicy {
    pub read_files: PermissionDecision,
    pub git: PermissionDecision,
    pub write_files: PermissionDecision,
    pub shell: PermissionDecision,
    pub max_tool_rounds: u32,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            read_files: PermissionDecision::Allow,
            git: PermissionDecision::Ask,
            write_files: PermissionDecision::Deny,
            shell: PermissionDecision::Deny,
            max_tool_rounds: 8,
        }
    }
}

impl PermissionPolicy {
    pub fn decision(&self, category: &ToolCategory) -> PermissionDecision {
        match category {
            ToolCategory::ReadFiles => self.read_files,
            ToolCategory::Git => self.git,
            ToolCategory::WriteFiles => self.write_files,
            ToolCategory::Shell => self.shell,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_typescript_fixture_roundtrips_without_field_drift() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/session.json")).unwrap();
        let session: AgentSession = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(session).unwrap(), fixture);
    }
}
