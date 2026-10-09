import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { z } from 'zod';
import defaults from '../../src-tauri/src/providers/defaults.json';
import type { AgentSession, Bootstrap, Model, ModelPreferences, ModelReference, PermissionPolicy, Project, ProviderAccount, ProviderAccountInfo, UsageRecord } from '../types/domain';
import { bootstrapSchema, modelPreferencesSchema, modelSchema, projectFileSchema, projectOverviewSchema, projectSchema, providerAccountInfoSchema, providerAccountSchema, sessionSchema, terminalResultSchema, usageSchema } from './schemas';
import { DesktopError, normalizeError } from './errors';

export const desktopAvailable = isTauri();
interface Commands {
  bootstrap: { args: undefined; result: Bootstrap };
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
  create_session: { args: { input: { projectId: string; providerId: string; modelId: string; permissionPolicy: PermissionPolicy } }; result: AgentSession };
  update_session_model: { args: { input: { sessionId: string; providerId: string; modelId: string } }; result: AgentSession };
  set_default_model: { args: { selection: ModelReference | null }; result: ModelPreferences };
  toggle_model_favorite: { args: { selection: ModelReference }; result: ModelPreferences };
  send_message: { args: { sessionId: string; content: string }; result: AgentSession };
  resolve_permission: { args: { sessionId: string; toolCallId: string; approved: boolean }; result: AgentSession };
  cancel_session: { args: { sessionId: string }; result: null };
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
  bootstrap: bootstrapSchema, open_project: projectSchema, create_project: projectSchema, remove_project_from_recents: projectSchema, update_project_settings: projectSchema,
  update_session_model: sessionSchema, set_default_model: modelPreferencesSchema, toggle_model_favorite: modelPreferencesSchema,
  list_project_directory: z.array(projectFileSchema), project_overview: projectOverviewSchema, project_branches: z.array(z.string()), switch_project_branch: projectSchema,
  reveal_project: z.null(), run_project_terminal: terminalResultSchema, project_branch: z.string().nullable(), create_session: sessionSchema, send_message: sessionSchema,
  resolve_permission: sessionSchema, cancel_session: z.null(), connect_provider: providerAccountSchema,
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
  return bootstrapSchema.parse({ workspace: { id: 'local', name: 'Local workspace', projects: [] }, providers: defaults.providers.map(provider => ({ ...provider, connected: provider.protocol === 'preview' })), accounts: [], sessions: [], usage: [], tools: [], permissionPolicy: { readFiles: 'allow', git: 'ask', writeFiles: 'deny', shell: 'deny', maxToolRounds: defaults.maxToolRounds }, modelPreferences: { defaultModel: null, favorites: [], recent: [] } });
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

export async function subscribeEvents(onSession: (session: AgentSession) => void, onUsage: (record: UsageRecord) => void, onError: () => void): Promise<UnlistenFn> {
  if (!desktopAvailable) return () => {};
  const listeners: UnlistenFn[] = [];
  try {
    listeners.push(await listen<unknown>('session:updated', event => { const parsed = sessionSchema.safeParse(event.payload); if (parsed.success) onSession(parsed.data); else onError(); }));
    listeners.push(await listen<unknown>('usage:updated', event => { const parsed = usageSchema.safeParse(event.payload); if (parsed.success) onUsage(parsed.data); else onError(); }));
    return () => listeners.forEach(unlisten => unlisten());
  } catch (error) { listeners.forEach(unlisten => unlisten()); throw normalizeError(error); }
}
