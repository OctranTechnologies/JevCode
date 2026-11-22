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
  it('passes containment status and the typed confirmation through IPC', async () => {
    const status = {
      projectId: 'project-1',
      platform: 'windows',
      projectOptedIn: true,
      machineAllowed: true,
      profiles: [{ kind: 'inspect', enforcement: { state: 'reduced_safety', unsupported: ['workspace.read'] } }],
    };
    mockIPC((name, args) => {
      expect(name).toBe('containment_status');
      expect(args).toEqual({ projectId: 'project-1' });
      return status;
    });
    const { getContainmentStatus, setProjectReducedSafety } = await import('../src/lib/ipc');
    await expect(getContainmentStatus('project-1')).resolves.toEqual(status);

    mockIPC((name, args) => {
      expect(name).toBe('set_project_reduced_safety');
      expect(args).toEqual({ projectId: 'project-1', enabled: true, confirmation: 'reduced safety' });
      return status;
    });
    await expect(setProjectReducedSafety('project-1', true, 'reduced safety')).resolves.toEqual(status);

    mockIPC(() => ({ ...status, platform: 'solaris' }));
    await expect(getContainmentStatus('project-1')).rejects.toMatchObject({ code: 'ipc_contract' });
  });
  it('sets the machine-level reduced-safety switch through IPC', async () => {
    mockIPC((name, args) => {
      expect(name).toBe('set_reduced_safety_allowed');
      expect(args).toEqual({ enabled: false });
      return { machineAllowed: false };
    });
    const { setReducedSafetyAllowed } = await import('../src/lib/ipc');
    await expect(setReducedSafetyAllowed(false)).resolves.toEqual({ machineAllowed: false });

    mockIPC(() => ({ allowed: false }));
    await expect(setReducedSafetyAllowed(false)).rejects.toMatchObject({ code: 'ipc_contract' });
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
