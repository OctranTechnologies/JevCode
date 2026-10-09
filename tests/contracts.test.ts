import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { bootstrapSchema, sessionSchema } from '../src/lib/schemas';
import { mergeSession } from '../src/app/store';
import { normalizeError } from '../src/lib/errors';
import { displayPath } from '../src/lib/paths';
import defaults from '../src-tauri/src/providers/defaults.json';

const session = JSON.parse(readFileSync(new URL('./fixtures/session.json', import.meta.url), 'utf8')) as unknown;
describe('desktop contracts', () => {
  it('accepts the shared Rust wire fixture and the provider catalog', () => {
    const parsed = sessionSchema.parse(session);
    expect(parsed.pendingToolCall?.name).toBe('git_status');
    const result = bootstrapSchema.parse({ workspace: { id: 'local', name: 'Local workspace', projects: [] }, providers: defaults.providers, accounts: [], sessions: [parsed], usage: [], tools: [], permissionPolicy: parsed.permissionPolicy, modelPreferences: { defaultModel: null, favorites: [], recent: [] } });
    expect(result.providers.map(provider => provider.id)).toEqual(['preview', 'openai', 'anthropic', 'gemini', 'opencode-zen', 'opencode-go']);
    expect(result.providers[1].models[0]).toMatchObject({ provider: 'openai', displayName: expect.any(String), supportsStreaming: true, status: 'available' });
  });
  it('rejects malformed desktop payloads', () => {
    const parsed = sessionSchema.parse(session);
    expect(sessionSchema.safeParse({ ...parsed, status: 'made_up' }).success).toBe(false);
    expect(sessionSchema.safeParse({ ...parsed, messages: [{ role: 'user' }] }).success).toBe(false);
  });
  it('does not let a late command response overwrite newer events', () => {
    const parsed = sessionSchema.parse(session);
    const newer = { ...parsed, status: 'completed' as const, updatedAt: '2026-10-09T01:01:00+00:00' };
    expect(mergeSession([newer], parsed)[0].status).toBe('completed');
  });
  it('keeps actionable backend errors and redacts unexpected exception details', () => {
    expect(normalizeError({ code: 'provider_auth', message: 'Update the API key.' }).message).toBe('Update the API key.');
    expect(normalizeError(new Error('secret-value')).message).not.toContain('secret-value');
  });
  it('shows extended Windows and UNC paths without their filesystem prefixes', () => {
    expect(displayPath('\\\\?\\C:\\Users\\dev\\project')).toBe('C:\\Users\\dev\\project');
    expect(displayPath('\\\\?\\UNC\\server\\share\\project')).toBe('\\\\server\\share\\project');
    expect(displayPath('/home/dev/project')).toBe('/home/dev/project');
  });
});
