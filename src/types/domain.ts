export type ProviderProtocol = 'preview' | 'open_ai_responses' | 'open_ai_chat' | 'anthropic' | 'gemini';
export type PermissionDecision = 'allow' | 'ask' | 'deny';
export type ToolCategory = 'read_files' | 'git' | 'write_files' | 'shell' | 'user_interaction';
export type SessionStatus = 'queued' | 'planning' | 'working' | 'waiting_for_permission' | 'waiting_for_user' | 'completed' | 'failed' | 'cancelled';
export interface Provider {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  baseUrl: string | null;
  models: Model[];
  connected: boolean;
}
export type ProviderAuthMethod = 'api_key' | 'google_oauth' | 'openai_chatgpt';
export type ProviderAuthState = 'not_connected' | 'connected' | 'needs_attention';
export interface ProviderAuthMethodOption {
  method: ProviderAuthMethod;
  label: string;
  available: boolean;
  unavailableReason: string | null;
}
/** Non-secret provider metadata. Tokens and API keys never cross back to the webview. */
export interface ProviderAccount {
  providerId: string;
  providerName: string;
  state: ProviderAuthState;
  authMethod: ProviderAuthMethod | null;
  accountLabel: string | null;
  connectedAt: string | null;
  lastValidatedAt: string | null;
  lastErrorCode: string | null;
  availableMethods: ProviderAuthMethodOption[];
}
export interface ProviderAccountInfo {
  providerId: string;
  providerName: string;
  authMethod: ProviderAuthMethod;
  accountLabel: string;
}
export interface Model {
  id: string;
  provider: string;
  displayName: string;
  /** Internal wire protocol for multi-protocol gateways; null uses provider default. */
  apiProtocol: ProviderProtocol | null;
  capabilities: ModelCapability[];
  supportsTools: boolean;
  supportsVision: boolean;
  supportsReasoning: boolean;
  supportsStreaming: boolean;
  contextWindow: number | null;
  inputPrice: number | null;
  outputPrice: number | null;
  status: ModelStatus;
}
export type ModelCapability = 'tools' | 'vision' | 'reasoning' | 'streaming';
export type ModelStatus = 'available' | 'deprecated' | 'unavailable';
export interface ModelReference { providerId: string; modelId: string }
export interface ModelPreferences { defaultModel: ModelReference | null; favorites: ModelReference[]; recent: ModelReference[] }
export interface AgentSession {
  id: string;
  projectId: string;
  providerId: string;
  modelId: string;
  title: string;
  status: SessionStatus;
  messages: AgentMessage[];
  permissionPolicy: PermissionPolicy;
  pendingToolCall: ToolCall | null;
  pendingUserInput: ToolCall | null;
  queuedToolCalls: ToolCall[];
  iterations: number;
  toolCalls: number;
  activityEvents: AgentActivityEvent[];
  createdAt: string;
  updatedAt: string;
  error: string | null;
  toolRounds: number;
}
export interface AgentStreamChunk { sessionId: string; delta: string; reset: boolean }
export interface AgentMessage {
  id: string;
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  toolCalls: ToolCall[];
  toolResult: ToolResult | null;
  createdAt: string;
  /** Opaque continuation data; interpreted only by the Rust provider adapter. */
  providerData: unknown;
}
export interface Tool {
  name: string;
  description: string;
  category: ToolCategory;
  parallelSafe: boolean;
  inputSchema: Record<string, unknown>;
}
export type AgentActivityKind = 'plan' | 'progress' | 'tool_started' | 'tool_completed' | 'file_inspected' | 'search_performed' | 'command_executed' | 'file_edited' | 'test_run';
export interface AgentActivityEvent {
  id: string;
  sessionId: string;
  kind: AgentActivityKind;
  summary: string;
  toolCallId: string | null;
  createdAt: string;
}
export interface ToolCall { id: string; name: string; arguments: Record<string, unknown> }
export interface ToolResult {
  toolCallId: string;
  name: string;
  content: string;
  isError: boolean;
  durationMs: number;
}
export interface Workspace { id: string; name: string; projects: Project[] }
export interface Project {
  id: string;
  workspaceId: string;
  name: string;
  path: string;
  repositoryRoot: string | null;
  activeBranch: string | null;
  lastOpenedAt: string;
  projectInstructions: string;
  preferredModel: string | null;
  permissions: PermissionPolicy;
  isRecent: boolean;
  createdAt: string;
}
export interface ProjectFileEntry { name: string; path: string; kind: 'file' | 'directory'; sizeBytes: number; isSymlink: boolean }
export interface ChangedFile { path: string; status: string }
export interface LanguageCount { name: string; files: number }
export interface ProjectOverview {
  projectId: string;
  repositoryRoot: string | null;
  activeBranch: string | null;
  gitStatusAvailable: boolean;
  isDirty: boolean;
  changedFiles: ChangedFile[];
  repositorySizeBytes: number;
  scanLimited: boolean;
  languages: LanguageCount[];
}
export interface TerminalResult { output: string; exitCode: number | null; timedOut: boolean }
export interface UsageRecord {
  id: string;
  sessionId: string;
  providerId: string;
  modelId: string;
  inputTokens: number;
  outputTokens: number;
  costUsd: number | null;
  durationMs: number;
  createdAt: string;
}
export interface PermissionPolicy {
  readFiles: PermissionDecision;
  git: PermissionDecision;
  writeFiles: PermissionDecision;
  shell: PermissionDecision;
  maxToolRounds: number;
}
export interface Bootstrap {
  workspace: Workspace;
  providers: Provider[];
  accounts: ProviderAccount[];
  sessions: AgentSession[];
  usage: UsageRecord[];
  tools: Tool[];
  permissionPolicy: PermissionPolicy;
  modelPreferences: ModelPreferences;
}
