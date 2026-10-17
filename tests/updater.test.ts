import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DownloadEvent, Update } from '@tauri-apps/plugin-updater';
import { createUpdaterService, type UpdaterPort } from '../src/services/updater';

function makeUpdate(overrides: Partial<Update> = {}): Update {
  return {
    rid: 1,
    currentVersion: '0.1.0',
    version: '0.2.0',
    date: '2026-10-17T00:00:00Z',
    body: 'Fixes and improvements',
    rawJson: {},
    download: vi.fn(async (onEvent?: (event: DownloadEvent) => void) => {
      onEvent?.({ event: 'Started', data: { contentLength: 10 } });
      onEvent?.({ event: 'Progress', data: { chunkLength: 4 } });
      onEvent?.({ event: 'Progress', data: { chunkLength: 6 } });
      onEvent?.({ event: 'Finished' });
    }),
    install: vi.fn(async () => undefined),
    close: vi.fn(async () => undefined),
    ...overrides,
  } as unknown as Update;
}

function createPort(overrides: Partial<UpdaterPort> = {}) {
  const update = makeUpdate();
  const port: UpdaterPort = {
    check: vi.fn(async () => update),
    getVersion: vi.fn(async () => '0.1.0'),
    relaunch: vi.fn(async () => undefined),
    ...overrides,
  };
  return { port, update };
}

afterEach(() => vi.useRealTimers());

describe('UpdaterService', () => {
  it('does not contact the updater when disabled for development', async () => {
    const { port } = createPort();
    const service = createUpdaterService({ enabled: () => false, port });

    const result = await service.check();

    expect(result.phase).toBe('disabled');
    expect(port.check).not.toHaveBeenCalled();
  });

  it('reports an available version and release notes', async () => {
    const { port } = createPort();
    const service = createUpdaterService({ enabled: () => true, port });

    const result = await service.check();

    expect(result).toMatchObject({ phase: 'available', currentVersion: '0.1.0', availableVersion: '0.2.0', notes: 'Fixes and improvements' });
  });

  it('reports the installed version when no update exists', async () => {
    const { port } = createPort({ check: vi.fn(async () => null) });
    const service = createUpdaterService({ enabled: () => true, port });

    const result = await service.check();

    expect(result).toMatchObject({ phase: 'up_to_date', currentVersion: '0.1.0' });
  });

  it('tracks download progress, installs, closes the resource, and relaunches', async () => {
    const { port, update } = createPort();
    const service = createUpdaterService({ enabled: () => true, port });
    const phases: string[] = [];
    service.subscribe(snapshot => phases.push(snapshot.phase));
    await service.check();

    const result = await service.installAvailableUpdate();

    expect(phases).toContain('downloading');
    expect(phases).toContain('installing');
    expect(result.phase).toBe('restarting');
    expect(result.downloadedBytes).toBe(10);
    expect(update.install).toHaveBeenCalledOnce();
    expect(update.close).toHaveBeenCalledOnce();
    expect(port.relaunch).toHaveBeenCalledOnce();
  });

  it('reports when the user must restart after a successful installation', async () => {
    const { port } = createPort({ relaunch: vi.fn(async () => { throw new Error('restart failed'); }) });
    const service = createUpdaterService({ enabled: () => true, port });
    await service.check();

    const result = await service.installAvailableUpdate();

    expect(result.phase).toBe('restart_required');
    expect(result.error).toBeNull();
  });

  it('checks automatically only when enabled', async () => {
    vi.useFakeTimers();
    const { port } = createPort();
    const service = createUpdaterService({ enabled: () => true, port, initialCheckDelayMs: 50, checkIntervalMs: 100 });
    const stop = service.startAutomaticChecks();

    await vi.advanceTimersByTimeAsync(50);
    expect(port.check).toHaveBeenCalledOnce();
    stop();
  });

  it('uses a safe error message instead of displaying raw update service details', async () => {
    const { port } = createPort({ check: vi.fn(async () => { throw new Error('request failed for https://updates.example/private/path'); }) });
    const service = createUpdaterService({ enabled: () => true, port });

    const result = await service.check();

    expect(result.phase).toBe('error');
    expect(result.error).toContain('Check your connection');
    expect(result.error).not.toContain('updates.example');
  });
});
