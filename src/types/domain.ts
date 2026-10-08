export type ProviderProtocol = 'preview' | 'open_ai_responses' | 'open_ai_chat' | 'anthropic' | 'gemini';
export type PermissionDecision = 'allow' | 'ask' | 'deny';
export type ToolCategory = 'read_files' | 'git' | 'write_files' | 'shell';
export type SessionStatus = 'idle' | 'running' | 'awaiting_permission' | 'completed' | 'failed' | 'cancelled';
export interface Provider {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  baseUrl: string | null;
  models: Model[];
  connected: boolean;
}
export interface Model {
  id: string;
  providerId: string;
  name: string;
  supportsTools: boolean;
  contextWindow: number | null;
}
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
  queuedToolCalls: ToolCall[];
  createdAt: string;
  updatedAt: string;
  error: string | null;
  toolRounds: number;
}
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
  inputSchema: Record<string, unknown>;
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
export interface Project { id: string; workspaceId: string; name: string; path: string; createdAt: string }
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
  sessions: AgentSession[];
  usage: UsageRecord[];
  tools: Tool[];
  permissionPolicy: PermissionPolicy;
}
