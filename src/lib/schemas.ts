import { z } from 'zod';
import type { AgentActivityEvent, AgentMessage, AgentSession, AgentStreamChunk, Bootstrap, Model, ModelPreferences, PermissionPolicy, Project, Provider, ProviderAccount, ProviderAccountInfo, Tool, ToolCall, ToolResult, UsageRecord, Workspace } from '../types/domain';

const count = z.number().int().nonnegative();
const decision = z.enum(['allow', 'ask', 'deny']);
export const permissionSchema: z.ZodType<PermissionPolicy> = z.object({ readFiles: decision, git: decision, writeFiles: decision, shell: decision, maxToolRounds: count });
export const modelSchema: z.ZodType<Model> = z.object({ id: z.string(), provider: z.string(), displayName: z.string(), apiProtocol: z.enum(['preview', 'open_ai_responses', 'open_ai_chat', 'anthropic', 'gemini']).nullable().default(null), capabilities: z.array(z.enum(['tools', 'vision', 'reasoning', 'streaming'])), supportsTools: z.boolean(), supportsVision: z.boolean(), supportsReasoning: z.boolean(), supportsStreaming: z.boolean(), contextWindow: count.nullable(), inputPrice: z.number().nonnegative().nullable(), outputPrice: z.number().nonnegative().nullable(), status: z.enum(['available', 'deprecated', 'unavailable']) });
export const modelReferenceSchema = z.object({ providerId: z.string(), modelId: z.string() });
export const modelPreferencesSchema: z.ZodType<ModelPreferences> = z.object({ defaultModel: modelReferenceSchema.nullable(), favorites: z.array(modelReferenceSchema), recent: z.array(modelReferenceSchema) });
export const providerSchema: z.ZodType<Provider> = z.object({ id: z.string(), name: z.string(), protocol: z.enum(['preview', 'open_ai_responses', 'open_ai_chat', 'anthropic', 'gemini']), baseUrl: z.string().nullable(), models: z.array(modelSchema), connected: z.boolean().default(false) });
export const providerAuthMethodSchema = z.enum(['api_key', 'google_oauth', 'openai_chatgpt']);
export const providerAccountSchema: z.ZodType<ProviderAccount> = z.object({
  providerId: z.string(), providerName: z.string(), state: z.enum(['not_connected', 'connected', 'needs_attention']),
  authMethod: providerAuthMethodSchema.nullable(), accountLabel: z.string().nullable(), connectedAt: z.string().nullable(),
  lastValidatedAt: z.string().nullable(), lastErrorCode: z.string().nullable(), availableMethods: z.array(z.object({
    method: providerAuthMethodSchema, label: z.string(), available: z.boolean(), unavailableReason: z.string().nullable(),
  })),
});
export const providerAccountInfoSchema: z.ZodType<ProviderAccountInfo> = z.object({ providerId: z.string(), providerName: z.string(), authMethod: providerAuthMethodSchema, accountLabel: z.string() });
export const toolSchema: z.ZodType<Tool> = z.object({ name: z.string(), description: z.string(), category: z.enum(['read_files', 'git', 'write_files', 'shell', 'user_interaction']), parallelSafe: z.boolean().default(false), inputSchema: z.record(z.string(), z.unknown()) });
export const toolCallSchema: z.ZodType<ToolCall> = z.object({ id: z.string(), name: z.string(), arguments: z.record(z.string(), z.unknown()) });
export const toolResultSchema: z.ZodType<ToolResult> = z.object({ toolCallId: z.string(), name: z.string(), content: z.string(), isError: z.boolean(), durationMs: count });
export const messageSchema: z.ZodType<AgentMessage> = z.object({ id: z.string(), role: z.enum(['system', 'user', 'assistant', 'tool']), content: z.string(), toolCalls: z.array(toolCallSchema), toolResult: toolResultSchema.nullable(), createdAt: z.string(), providerData: z.unknown() });
const agentActivitySchema: z.ZodType<AgentActivityEvent> = z.object({ id: z.string(), sessionId: z.string(), kind: z.enum(['plan', 'progress', 'tool_started', 'tool_completed', 'file_inspected', 'search_performed', 'command_executed', 'file_edited', 'test_run']), summary: z.string(), toolCallId: z.string().nullable(), createdAt: z.string() });
export const sessionSchema: z.ZodType<AgentSession> = z.object({ id: z.string(), projectId: z.string(), providerId: z.string(), modelId: z.string(), title: z.string(), status: z.enum(['queued', 'planning', 'working', 'waiting_for_permission', 'waiting_for_user', 'completed', 'failed', 'cancelled']), messages: z.array(messageSchema), permissionPolicy: permissionSchema, pendingToolCall: toolCallSchema.nullable().default(null), pendingUserInput: toolCallSchema.nullable().default(null), queuedToolCalls: z.array(toolCallSchema), iterations: count.default(0), toolCalls: count.default(0), activityEvents: z.array(agentActivitySchema).default([]), createdAt: z.string(), updatedAt: z.string(), error: z.string().nullable(), toolRounds: count });
export const agentStreamChunkSchema: z.ZodType<AgentStreamChunk> = z.object({ sessionId: z.string(), delta: z.string(), reset: z.boolean() });
export const projectSchema: z.ZodType<Project> = z.object({
  id: z.string(), workspaceId: z.string(), name: z.string(), path: z.string(), repositoryRoot: z.string().nullable(), activeBranch: z.string().nullable(), lastOpenedAt: z.string(), projectInstructions: z.string(), preferredModel: z.string().nullable(), permissions: permissionSchema, isRecent: z.boolean(), createdAt: z.string(),
});
export const projectFileSchema: z.ZodType<import('../types/domain').ProjectFileEntry> = z.object({ name: z.string(), path: z.string(), kind: z.enum(['file', 'directory']), sizeBytes: count, isSymlink: z.boolean() });
export const changedFileSchema: z.ZodType<import('../types/domain').ChangedFile> = z.object({ path: z.string(), status: z.string() });
export const languageCountSchema: z.ZodType<import('../types/domain').LanguageCount> = z.object({ name: z.string(), files: count });
export const projectOverviewSchema: z.ZodType<import('../types/domain').ProjectOverview> = z.object({ projectId: z.string(), repositoryRoot: z.string().nullable(), activeBranch: z.string().nullable(), gitStatusAvailable: z.boolean(), isDirty: z.boolean(), changedFiles: z.array(changedFileSchema), repositorySizeBytes: count, scanLimited: z.boolean(), languages: z.array(languageCountSchema) });
export const terminalResultSchema: z.ZodType<import('../types/domain').TerminalResult> = z.object({ output: z.string(), exitCode: z.number().int().nullable(), timedOut: z.boolean() });
export const workspaceSchema: z.ZodType<Workspace> = z.object({ id: z.string(), name: z.string(), projects: z.array(projectSchema) });
export const usageSchema: z.ZodType<UsageRecord> = z.object({ id: z.string(), sessionId: z.string(), providerId: z.string(), modelId: z.string(), inputTokens: count, outputTokens: count, costUsd: z.number().nullable(), durationMs: count, createdAt: z.string() });
export const bootstrapSchema: z.ZodType<Bootstrap> = z.object({ workspace: workspaceSchema, providers: z.array(providerSchema), accounts: z.array(providerAccountSchema), sessions: z.array(sessionSchema), usage: z.array(usageSchema), tools: z.array(toolSchema), permissionPolicy: permissionSchema, modelPreferences: modelPreferencesSchema });
