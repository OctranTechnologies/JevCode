import { z } from 'zod';
import type { AgentMessage, AgentSession, Bootstrap, Model, PermissionPolicy, Project, Provider, Tool, ToolCall, ToolResult, UsageRecord, Workspace } from '../types/domain';

const count = z.number().int().nonnegative();
const decision = z.enum(['allow', 'ask', 'deny']);
export const permissionSchema: z.ZodType<PermissionPolicy> = z.object({ readFiles: decision, git: decision, writeFiles: decision, shell: decision, maxToolRounds: count });
export const modelSchema: z.ZodType<Model> = z.object({ id: z.string(), providerId: z.string(), name: z.string(), supportsTools: z.boolean(), contextWindow: count.nullable() });
export const providerSchema: z.ZodType<Provider> = z.object({ id: z.string(), name: z.string(), protocol: z.enum(['preview', 'open_ai_responses', 'open_ai_chat', 'anthropic', 'gemini']), baseUrl: z.string().nullable(), models: z.array(modelSchema), connected: z.boolean().default(false) });
export const toolSchema: z.ZodType<Tool> = z.object({ name: z.string(), description: z.string(), category: z.enum(['read_files', 'git', 'write_files', 'shell']), inputSchema: z.record(z.string(), z.unknown()) });
export const toolCallSchema: z.ZodType<ToolCall> = z.object({ id: z.string(), name: z.string(), arguments: z.record(z.string(), z.unknown()) });
export const toolResultSchema: z.ZodType<ToolResult> = z.object({ toolCallId: z.string(), name: z.string(), content: z.string(), isError: z.boolean(), durationMs: count });
export const messageSchema: z.ZodType<AgentMessage> = z.object({ id: z.string(), role: z.enum(['system', 'user', 'assistant', 'tool']), content: z.string(), toolCalls: z.array(toolCallSchema), toolResult: toolResultSchema.nullable(), createdAt: z.string(), providerData: z.unknown() });
export const sessionSchema: z.ZodType<AgentSession> = z.object({ id: z.string(), projectId: z.string(), providerId: z.string(), modelId: z.string(), title: z.string(), status: z.enum(['idle', 'running', 'awaiting_permission', 'completed', 'failed', 'cancelled']), messages: z.array(messageSchema), permissionPolicy: permissionSchema, pendingToolCall: toolCallSchema.nullable(), queuedToolCalls: z.array(toolCallSchema), createdAt: z.string(), updatedAt: z.string(), error: z.string().nullable(), toolRounds: count });
export const projectSchema: z.ZodType<Project> = z.object({ id: z.string(), workspaceId: z.string(), name: z.string(), path: z.string(), createdAt: z.string() });
export const workspaceSchema: z.ZodType<Workspace> = z.object({ id: z.string(), name: z.string(), projects: z.array(projectSchema) });
export const usageSchema: z.ZodType<UsageRecord> = z.object({ id: z.string(), sessionId: z.string(), providerId: z.string(), modelId: z.string(), inputTokens: count, outputTokens: count, costUsd: z.number().nullable(), durationMs: count, createdAt: z.string() });
export const bootstrapSchema: z.ZodType<Bootstrap> = z.object({ workspace: workspaceSchema, providers: z.array(providerSchema), sessions: z.array(sessionSchema), usage: z.array(usageSchema), tools: z.array(toolSchema), permissionPolicy: permissionSchema });
