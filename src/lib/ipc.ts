import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { z } from 'zod';
import defaults from '../../src-tauri/src/providers/defaults.json';
import type { AgentSession, Bootstrap, PermissionPolicy, Project, Provider, UsageRecord } from '../types/domain';
import { bootstrapSchema, projectSchema, providerSchema, sessionSchema, usageSchema } from './schemas';
import { DesktopError, normalizeError } from './errors';

export const desktopAvailable = isTauri();
interface Commands {
  bootstrap: { args: undefined; result: Bootstrap };
  open_project: { args: { path: string }; result: Project };
  create_session: { args: { input: { projectId: string; providerId: string; modelId: string; permissionPolicy: PermissionPolicy } }; result: AgentSession };
  send_message: { args: { sessionId: string; content: string }; result: AgentSession };
  resolve_permission: { args: { sessionId: string; toolCallId: string; approved: boolean }; result: AgentSession };
  cancel_session: { args: { sessionId: string }; result: null };
  save_credential: { args: { providerId: string; secret: string }; result: Provider[] };
  delete_credential: { args: { providerId: string }; result: Provider[] };
  frontend_log: { args: { level: 'info' | 'error'; event: string }; result: null };
}
const responses = {
  bootstrap: bootstrapSchema, open_project: projectSchema, create_session: sessionSchema, send_message: sessionSchema,
  resolve_permission: sessionSchema, cancel_session: z.null(), save_credential: z.array(providerSchema), delete_credential: z.array(providerSchema), frontend_log: z.null(),
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
  return bootstrapSchema.parse({ workspace: { id: 'local', name: 'Local workspace', projects: [] }, providers: defaults.providers.map(provider => ({ ...provider, connected: provider.protocol === 'preview' })), sessions: [], usage: [], tools: [], permissionPolicy: { readFiles: 'allow', git: 'ask', writeFiles: 'deny', shell: 'deny', maxToolRounds: defaults.maxToolRounds } });
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
