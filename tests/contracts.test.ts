import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { bootstrapSchema, googleAuthAttemptEventSchema, googleAuthStartSchema, googleAuthStatusSchema, openAIAuthAttemptEventSchema, openAIAuthStartSchema, openAIAuthStatusSchema, sessionSchema } from '../src/lib/schemas';
import { mergeSession } from '../src/app/store';
import { normalizeError } from '../src/lib/errors';
import { displayPath } from '../src/lib/paths';
import defaults from '../src-tauri/src/providers/defaults.json';

const session = JSON.parse(readFileSync(new URL('./fixtures/session.json', import.meta.url), 'utf8')) as unknown;
describe('desktop contracts', () => {
  it('accepts the shared Rust wire fixture and the provider catalog', () => {
    const parsed = sessionSchema.parse(session);
    expect(parsed.pendingToolCall?.name).toBe('git_status');
    const result = bootstrapSchema.parse({ workspace: { id: 'local', name: 'Local workspace', projects: [] }, providers: defaults.providers, accounts: [], sessions: [parsed], usage: [], tools: [], permissionPolicy: parsed.permissionPolicy, modelPreferences: { defaultModel: null, favorites: [], recent: [] }, openaiAuthStatus: { apiKey: { providerId: 'openai', providerName: 'OpenAI', state: 'not_connected', authMethod: null, accountLabel: null, connectedAt: null, lastValidatedAt: null, lastErrorCode: null, availableMethods: [] }, chatgpt: { available: true, connectedAccounts: [], selectedAccountId: null, selectedAuthMethod: null, unavailableReason: null } }, updatePreferences: { automaticallyCheck: true, automaticallyDownload: false, channel: 'stable', deferredVersion: null, skippedVersion: null, remindAfter: null, downloadRequestedVersion: null } });
    expect(result.providers.map(provider => provider.id)).toEqual(['preview', 'openai', 'anthropic', 'gemini', 'opencode-zen', 'opencode-go']);
    // OpenAI models are account-scoped and loaded from the authenticated
    // account's official model catalog instead of a guessed static list.
    expect(result.providers[1].models).toEqual([]);
  });
  it('rejects malformed desktop payloads', () => {
    const parsed = sessionSchema.parse(session);
    expect(sessionSchema.safeParse({ ...parsed, status: 'made_up' }).success).toBe(false);
    expect(sessionSchema.safeParse({ ...parsed, messages: [{ role: 'user' }] }).success).toBe(false);
  });
  it('keeps OpenAI status token-free across the typed IPC contract', () => {
    const safeStatus = {
      apiKey: { providerId: 'openai', providerName: 'OpenAI', state: 'not_connected', authMethod: null, accountLabel: null, connectedAt: null, lastValidatedAt: null, lastErrorCode: null, availableMethods: [] },
      chatgpt: { available: false, connectedAccounts: [], selectedAccountId: null, selectedAuthMethod: null, unavailableReason: 'Sign in is not enabled yet.' },
    };
    expect(openAIAuthStatusSchema.parse(safeStatus).chatgpt.connectedAccounts).toEqual([]);
    expect(openAIAuthStatusSchema.safeParse({ ...safeStatus, accessToken: 'must-not-cross-ipc' }).success).toBe(false);
    expect(openAIAuthStatusSchema.safeParse({ ...safeStatus, apiKey: { ...safeStatus.apiKey, refreshToken: 'must-not-cross-ipc' } }).success).toBe(false);
    const account = { id: 'd8f8a8ef-4df0-4c58-b20d-61cda4482f60', connected: true, accessTokenValid: true, requiresReconnect: false, displayName: 'Example', email: 'user@example.test', organization: null, clientId: 'registration-label', grantedScopes: ['openid'], connectedAt: '2026-10-14T12:00:00Z', expiresAt: null };
    expect(openAIAuthStatusSchema.safeParse({ ...safeStatus, chatgpt: { ...safeStatus.chatgpt, available: true, connectedAccounts: [account], selectedAccountId: account.id, selectedAuthMethod: 'openai_chatgpt' } }).success).toBe(true);
    expect(openAIAuthStatusSchema.safeParse({ ...safeStatus, chatgpt: { ...safeStatus.chatgpt, available: true, connectedAccounts: [{ ...account, refreshToken: 'must-not-cross-ipc' }], selectedAccountId: account.id, selectedAuthMethod: 'openai_chatgpt' } }).success).toBe(false);
  });
  it('does not register a renderer command for reading OpenAI session credentials', () => {
    const nativeCommands = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');
    const handler = nativeCommands.slice(nativeCommands.indexOf('generate_handler!'));
    expect(handler).toContain('commands::openai_auth_status');
    expect(handler).toContain('commands::openai_auth_select_account');
    expect(handler).not.toMatch(/openai_auth_(?:get|read)_credentials|commands::get_openai_session/);
  });
  it('keeps OpenAI authorization attempt material behind the native boundary', () => {
    const safeAttempt = { attemptId: 'd8f8a8ef-4df0-4c58-b20d-61cda4482f60', expiresAt: '2026-10-14T12:00:00+00:00' };
    expect(openAIAuthStartSchema.parse(safeAttempt)).toEqual(safeAttempt);
    expect(openAIAuthStartSchema.safeParse({ ...safeAttempt, authorizationUrl: 'https://auth.openai.com/?state=private' }).success).toBe(false);
    expect(openAIAuthStartSchema.safeParse({ ...safeAttempt, codeVerifier: 'private-verifier' }).success).toBe(false);
    expect(openAIAuthAttemptEventSchema.parse({ attemptId: safeAttempt.attemptId, state: 'callback_received' })).toEqual({ attemptId: safeAttempt.attemptId, state: 'callback_received', failureReason: null });
    expect(openAIAuthAttemptEventSchema.parse({ attemptId: safeAttempt.attemptId, state: 'failed', failureReason: 'network_error' }).failureReason).toBe('network_error');
  });
  it('keeps Google OAuth status and attempt handles token-free and registers narrow method commands', () => {
    const safeAttempt = { attemptId: '5b5b0e1a-8127-4e62-9fb7-f3a3dd9d09ce', expiresAt: '2026-10-14T12:00:00+00:00' };
    expect(googleAuthStartSchema.parse(safeAttempt)).toEqual(safeAttempt);
    expect(googleAuthStartSchema.safeParse({ ...safeAttempt, authorizationUrl: 'https://accounts.google.com/?state=private' }).success).toBe(false);
    const safeStatus = {
      account: { id: 'google-profile', email: 'user@gmail.com', displayName: 'Google User', authType: 'google_oauth', accountType: 'individual', cloudProjectId: null, connected: true, requiresReconnect: false, expiresAt: null, grantedScopes: ['openid', 'email'] },
      accounts: [{ id: 'google-profile', email: 'user@gmail.com', displayName: 'Google User', authType: 'google_oauth', accountType: 'individual', cloudProjectId: null, connected: true, requiresReconnect: false, expiresAt: null, grantedScopes: ['openid', 'email'] }],
      selectedAccountId: 'google-profile',
      codeAssist: { accountType: 'individual', availability: 'consumer_access_retired', requiresCloudProject: false, cloudProjectId: null, userTier: null, inferenceAvailable: false },
      selectedAuthType: null,
      apiKeyConnected: false,
      oauthAvailable: true,
      unavailableReason: null,
    };
    expect(googleAuthStatusSchema.parse(safeStatus).account?.email).toBe('user@gmail.com');
    expect(googleAuthStatusSchema.safeParse({ ...safeStatus, accessToken: 'must-not-cross-ipc' }).success).toBe(false);
    expect(googleAuthStatusSchema.safeParse({ ...safeStatus, account: { ...safeStatus.account, refreshToken: 'must-not-cross-ipc' } }).success).toBe(false);
    expect(googleAuthAttemptEventSchema.parse({ attemptId: safeAttempt.attemptId, state: 'failed', failureReason: 'network_error' }).failureReason).toBe('network_error');
    const nativeCommands = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');
    const handler = nativeCommands.slice(nativeCommands.indexOf('generate_handler!'));
    expect(handler).toContain('commands::google_auth_select_account');
    expect(handler).toContain('commands::google_auth_select_method');
    expect(handler).toContain('commands::google_auth_disconnect_api_key');
    expect(handler).not.toMatch(/google_auth_(?:get|read)_credentials|commands::get_google_session/);
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
