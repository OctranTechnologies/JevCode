import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import fixture from './fixtures/session.json';

describe('typed Tauri command boundary', () => {
  beforeEach(() => { vi.resetModules(); vi.stubGlobal('window', { crypto: globalThis.crypto }); vi.stubGlobal('isTauri', true); });
  afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });
  it('passes command arguments and validates the response', async () => {
    mockIPC((name, args) => {
      expect(name).toBe('send_message');
      expect(args).toEqual({ sessionId: 'test-session', content: 'Review Git status' });
      return fixture;
    });
    const { command } = await import('../src/lib/ipc');
    const result = await command('send_message', { sessionId: 'test-session', content: 'Review Git status' });
    expect(result.status).toBe('waiting_for_permission');
  });
  it('copies the OpenAI sign-in link through native IPC without returning its URL', async () => {
    mockIPC((name, args) => {
      expect(name).toBe('openai_auth_copy_link');
      expect(args).toEqual({ attemptId: 'd8f8a8ef-4df0-4c58-b20d-61cda4482f60' });
      return null;
    });
    const { copyOpenAIAuthLink } = await import('../src/lib/ipc');
    await expect(copyOpenAIAuthLink('d8f8a8ef-4df0-4c58-b20d-61cda4482f60')).resolves.toBeNull();

    mockIPC(() => 'https://auth.openai.com/authorize?state=should-not-cross-ipc');
    await expect(copyOpenAIAuthLink('d8f8a8ef-4df0-4c58-b20d-61cda4482f60')).rejects.toMatchObject({ code: 'ipc_contract' });
  });
  it('rejects malformed results and preserves structured backend errors', async () => {
    mockIPC(() => ({ status: 'oops' }));
    const { command } = await import('../src/lib/ipc');
    await expect(command('send_message', { sessionId: 'x', content: 'hello' })).rejects.toMatchObject({ code: 'ipc_contract' });
    mockIPC(() => { throw { code: 'session_busy', message: 'This session is already running.' }; });
    await expect(command('send_message', { sessionId: 'x', content: 'hello' })).rejects.toMatchObject({ code: 'session_busy' });
  });
  it('validates emitted session snapshots and cleans up listeners', async () => {
    mockIPC(() => null, { shouldMockEvents: true });
    const { subscribeEvents } = await import('../src/lib/ipc');
    const onSession = vi.fn();
    const onError = vi.fn();
    const cleanup = await subscribeEvents(onSession, vi.fn(), onError);
    await emit('session:updated', fixture);
    expect(onSession).toHaveBeenCalledTimes(1);
    await emit('session:updated', { malformed: true });
    expect(onError).toHaveBeenCalledTimes(1);
    cleanup();
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await emit('session:updated', fixture);
    expect(onSession).toHaveBeenCalledTimes(1);
    warning.mockRestore();
  });
});
