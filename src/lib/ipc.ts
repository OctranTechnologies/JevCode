import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { z } from 'zod';
import defaults from '../../src-tauri/src/providers/defaults.json';
import type { AgentSession, AgentStreamChunk, AppDiagnostics, Bootstrap, McpScope, McpServerConfig, McpServerView, Model, ModelPreferences, ModelReference, PermissionMode, PermissionPolicy, PermissionRule, Project, ProviderAccount, ProviderAccountInfo, ReviewAllAction, ReviewFileAction, SessionChanges, SessionFileDiff, TaskWorktree, TaskWorktreeAction, ToolOutputChunk, UpdateInfo, UsageRecord, WorkspaceMode } from '../types/domain';
import { agentStreamChunkSchema, appDiagnosticsSchema, bootstrapSchema, mcpServerConfigSchema, mcpServerSchema, modelPreferencesSchema, modelSchema, permissionModeSchema, permissionRuleSchema, projectFileSchema, projectOverviewSchema, projectSchema, providerAccountInfoSchema, providerAccountSchema, sessionChangesSchema, sessionFileDiffSchema, sessionSchema, taskWorktreeActionSchema, taskWorktreeSchema, terminalResultSchema, toolOutputChunkSchema, updateInfoSchema, usageSchema } from './schemas';
import { DesktopError, normalizeError } from './errors';

export const desktopAvailable = isTauri();
interface Commands {
  bootstrap: { args: undefined; result: Bootstrap };
  get_app_diagnostics: { args: undefined; result: AppDiagnostics };
  check_for_updates: { args: undefined; result: UpdateInfo };
  open_latest_release: { args: undefined; result: null };
  reveal_application_logs: { args: undefined; result: null };
  open_project: { args: { path: string }; result: Project };
  create_project: { args: { name: string; parentPath: string }; result: Project };
  remove_project_from_recents: { args: { projectId: string }; result: Project };
  update_project_settings: { args: { input: { projectId: string; projectInstructions: string; preferredModel: string | null; permissions: PermissionPolicy } }; result: Project };
  list_project_directory: { args: { projectId: string; path: string }; result: import('../types/domain').ProjectFileEntry[] };
  project_overview: { args: { projectId: string }; result: import('../types/domain').ProjectOverview };
  project_branches: { args: { projectId: string }; result: string[] };
  switch_project_branch: { args: { projectId: string; branch: string }; result: Project };
  reveal_project: { args: { projectId: string }; result: null };
  run_project_terminal: { args: { projectId: string; commandText: string }; result: import('../types/domain').TerminalResult };
  project_branch: { args: { projectId: string }; result: string | null };
  project_task_worktrees: { args: { projectId: string }; result: TaskWorktree[] };
  task_worktree_diff: { args: { sessionId: string }; result: string };
  commit_task_worktree: { args: { sessionId: string }; result: TaskWorktreeAction };
  apply_task_worktree: { args: { sessionId: string }; result: TaskWorktreeAction };
  remove_task_worktree: { args: { sessionId: string }; result: AgentSession };
  create_session: { args: { input: { projectId: string; providerId: string; modelId: string; permissionPolicy: PermissionPolicy; workspaceMode?: WorkspaceMode; baseBranch?: string | null; taskRequest?: string } }; result: AgentSession };
  update_session_model: { args: { input: { sessionId: string; providerId: string; modelId: string } }; result: AgentSession };
  set_default_model: { args: { selection: ModelReference | null }; result: ModelPreferences };
  toggle_model_favorite: { args: { selection: ModelReference }; result: ModelPreferences };
  send_message: { args: { sessionId: string; content: string }; result: AgentSession };
  rename_session: { args: { input: { sessionId: string; title: string } }; result: AgentSession };
  archive_session: { args: { sessionId: string; archived: boolean }; result: AgentSession };
  resume_session: { args: { sessionId: string }; result: AgentSession };
  delete_session: { args: { sessionId: string }; result: null };
  duplicate_session: { args: { sessionId: string }; result: AgentSession };
  fork_session: { args: { sessionId: string }; result: AgentSession };
  resolve_permission: { args: { sessionId: string; toolCallId: string; resolution: import('../types/domain').PermissionResolution }; result: AgentSession };
  set_permission_mode: { args: { mode: PermissionMode }; result: PermissionMode };
  list_permission_rules: { args: undefined; result: PermissionRule[] };
  revoke_permission_rule: { args: { ruleId: string }; result: null };
  list_mcp_servers: { args: { projectId: string | null }; result: McpServerView[] };
  save_mcp_server: { args: { input: { config: McpServerConfig; secrets: Record<string, string> } }; result: McpServerConfig };
  set_mcp_server_enabled: { args: { input: { scope: McpScope; projectId: string | null; serverId: string }; enabled: boolean }; result: McpServerView[] };
  connect_mcp_server: { args: { input: { scope: McpScope; projectId: string | null; serverId: string }; trust: boolean }; result: McpServerView };
  disconnect_mcp_server: { args: { input: { scope: McpScope; projectId: string | null; serverId: string } }; result: McpServerView[] };
  delete_mcp_server: { args: { input: { scope: McpScope; projectId: string | null; serverId: string } }; result: null };
  cancel_session: { args: { sessionId: string }; result: null };
  session_changes: { args: { sessionId: string }; result: SessionChanges };
  session_file_diff: { args: { sessionId: string; path: string }; result: SessionFileDiff };
  review_file_action: { args: { sessionId: string; path: string; action: ReviewFileAction }; result: SessionChanges };
  review_all_action: { args: { sessionId: string; action: ReviewAllAction }; result: SessionChanges };
  connect_provider: { args: { providerId: string; secret: string }; result: ProviderAccount };
  disconnect_provider: { args: { providerId: string }; result: ProviderAccount };
  validate_provider_auth: { args: { providerId: string }; result: ProviderAccount };
  refresh_provider_auth: { args: { providerId: string }; result: ProviderAccount };
  get_provider_auth_status: { args: { providerId: string }; result: ProviderAccount };
  get_provider_account_info: { args: { providerId: string }; result: ProviderAccountInfo };
  get_provider_available_models: { args: { providerId: string }; result: Model[] };
  frontend_log: { args: { level: 'info' | 'error'; event: string }; result: null };
}
const responses = {
  bootstrap: bootstrapSchema, get_app_diagnostics: appDiagnosticsSchema, check_for_updates: updateInfoSchema, open_latest_release: z.null(), reveal_application_logs: z.null(), open_project: projectSchema, create_project: projectSchema, remove_project_from_recents: projectSchema, update_project_settings: projectSchema,
  update_session_model: sessionSchema, set_default_model: modelPreferencesSchema, toggle_model_favorite: modelPreferencesSchema,
  list_project_directory: z.array(projectFileSchema), project_overview: projectOverviewSchema, project_branches: z.array(z.string()), switch_project_branch: projectSchema,
  reveal_project: z.null(), run_project_terminal: terminalResultSchema, project_branch: z.string().nullable(), project_task_worktrees: z.array(taskWorktreeSchema), task_worktree_diff: z.string(), commit_task_worktree: taskWorktreeActionSchema, apply_task_worktree: taskWorktreeActionSchema, remove_task_worktree: sessionSchema, create_session: sessionSchema, send_message: sessionSchema,
  rename_session: sessionSchema, archive_session: sessionSchema, resume_session: sessionSchema, delete_session: z.null(), duplicate_session: sessionSchema, fork_session: sessionSchema,
  resolve_permission: sessionSchema, set_permission_mode: permissionModeSchema, list_permission_rules: z.array(permissionRuleSchema), revoke_permission_rule: z.null(), list_mcp_servers: z.array(mcpServerSchema), save_mcp_server: mcpServerConfigSchema, set_mcp_server_enabled: z.array(mcpServerSchema), connect_mcp_server: mcpServerSchema, disconnect_mcp_server: z.array(mcpServerSchema), delete_mcp_server: z.null(), cancel_session: z.null(), session_changes: sessionChangesSchema, session_file_diff: sessionFileDiffSchema, review_file_action: sessionChangesSchema, review_all_action: sessionChangesSchema, connect_provider: providerAccountSchema,
  disconnect_provider: providerAccountSchema, validate_provider_auth: providerAccountSchema,
  refresh_provider_auth: providerAccountSchema, get_provider_auth_status: providerAccountSchema,
  get_provider_account_info: providerAccountInfoSchema, get_provider_available_models: z.array(modelSchema),
  frontend_log: z.null(),
} satisfies { [K in keyof Commands]: z.ZodType<Commands[K]['result']> };

export async function command<K extends keyof Commands>(name: K, args: Commands[K]['args']): Promise<Commands[K]['result']> {
  if (!desktopAvailable) throw new DesktopError('desktop_required', 'Run npm run tauri dev to use local projects, agent sessions and credentials.');
  try {
    const result: unknown = await invoke(name, args);
    const parsed = responses[name].safeParse(result);
    if (!parsed.success) throw new DesktopError('ipc_contract', 'The desktop service returned unexpected data. Restart JevCode.');
    return parsed.data as Commands[K]['result'];
  } catch (error) { throw normalizeError(error); }
}

export async function loadBootstrap(): Promise<Bootstrap> {
  if (desktopAvailable) return command('bootstrap', undefined);
  // Browser preview displays configuration metadata only. Native actions stay disabled.
  return bootstrapSchema.parse({ workspace: { id: 'local', name: 'Local workspace', projects: [] }, providers: defaults.providers.map(provider => ({ ...provider, connected: provider.protocol === 'preview' })), accounts: [], sessions: [], usage: [], tools: [], permissionPolicy: { mode: 'ask', readFiles: 'allow', git: 'allow', writeFiles: 'allow', shell: 'allow', externalFiles: 'ask', maxToolRounds: defaults.maxToolRounds }, modelPreferences: { defaultModel: null, favorites: [], recent: [] } });
}

export interface ProviderAuthAdapter {
  connect(secret: string): Promise<ProviderAccount>;
  disconnect(): Promise<ProviderAccount>;
  refresh(): Promise<ProviderAccount>;
  validate(): Promise<ProviderAccount>;
  getAccountInfo(): Promise<ProviderAccountInfo>;
  getAvailableModels(): Promise<Model[]>;
  getAuthStatus(): Promise<ProviderAccount>;
}

/** Provider-neutral auth facade. Provider-specific protocol stays in Rust. */
export function providerAuthAdapter(providerId: string): ProviderAuthAdapter {
  return {
    connect: secret => command('connect_provider', { providerId, secret }),
    disconnect: () => command('disconnect_provider', { providerId }),
    refresh: () => command('refresh_provider_auth', { providerId }),
    validate: () => command('validate_provider_auth', { providerId }),
    getAccountInfo: () => command('get_provider_account_info', { providerId }),
    getAvailableModels: () => command('get_provider_available_models', { providerId }),
    getAuthStatus: () => command('get_provider_auth_status', { providerId }),
  };
}

export async function subscribeEvents(onSession: (session: AgentSession) => void, onUsage: (record: UsageRecord) => void, onError: () => void, onStream?: (chunk: AgentStreamChunk) => void, onToolOutput?: (chunk: ToolOutputChunk) => void, onProviderAccount?: (account: ProviderAccount) => void): Promise<UnlistenFn> {
  if (!desktopAvailable) return () => {};
  const listeners: UnlistenFn[] = [];
  try {
    listeners.push(await listen<unknown>('session:updated', event => { const parsed = sessionSchema.safeParse(event.payload); if (parsed.success) onSession(parsed.data); else onError(); }));
    listeners.push(await listen<unknown>('usage:updated', event => { const parsed = usageSchema.safeParse(event.payload); if (parsed.success) onUsage(parsed.data); else onError(); }));
    if (onStream) listeners.push(await listen<unknown>('agent:stream', event => { const parsed = agentStreamChunkSchema.safeParse(event.payload); if (parsed.success) onStream(parsed.data); else onError(); }));
    if (onToolOutput) listeners.push(await listen<unknown>('agent:tool-output', event => { const parsed = toolOutputChunkSchema.safeParse(event.payload); if (parsed.success) onToolOutput(parsed.data); else onError(); }));
    if (onProviderAccount) listeners.push(await listen<unknown>('provider:account-updated', event => { const parsed = providerAccountSchema.safeParse(event.payload); if (parsed.success) onProviderAccount(parsed.data); else onError(); }));
    return () => listeners.forEach(unlisten => unlisten());
  } catch (error) { listeners.forEach(unlisten => unlisten()); throw normalizeError(error); }
}
