import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DownloadEvent, Update } from '@tauri-apps/plugin-updater';
import { createUpdateService, type UpdaterPort } from '../src/services/updater';

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

describe('UpdateService', () => {
  it('does not contact the updater when disabled for development', async () => {
    const { port } = createPort();
    const service = createUpdateService({ enabled: () => false, port });

    const result = await service.checkForUpdate();

    expect(result.phase).toBe('disabled');
    expect(port.check).not.toHaveBeenCalled();
  });

  it('reports an available version and release notes', async () => {
    const { port } = createPort();
    const service = createUpdateService({ enabled: () => true, port });

    const result = await service.checkForUpdate();

    expect(result).toMatchObject({ phase: 'available', currentVersion: '0.1.0', availableVersion: '0.2.0', notes: 'Fixes and improvements' });
    await expect(service.getCurrentVersion()).resolves.toBe('0.1.0');
    expect(service.getAvailableVersion()).toBe('0.2.0');
    expect(service.getUpdateMetadata()).toEqual({ version: '0.2.0', publishedAt: '2026-10-17T00:00:00Z', notes: 'Fixes and improvements' });
  });

  it('reports the installed version when no update exists', async () => {
    const { port } = createPort({ check: vi.fn(async () => null) });
    const service = createUpdateService({ enabled: () => true, port });

    const result = await service.checkForUpdate();

    expect(result).toMatchObject({ phase: 'up_to_date', currentVersion: '0.1.0' });
  });

  it('tracks download progress and reports the update ready before installation', async () => {
    const { port, update } = createPort();
    const service = createUpdateService({ enabled: () => true, port });
    const phases: string[] = [];
    service.subscribe(snapshot => phases.push(snapshot.phase));
    await service.checkForUpdate();

    const result = await service.downloadAvailableUpdate();

    expect(phases).toContain('downloading');
    expect(phases).toContain('ready');
    expect(result.phase).toBe('ready');
    expect(result.downloadedBytes).toBe(10);
    expect(update.install).not.toHaveBeenCalled();
  });

  it('preserves the downloaded update while it is ready to install', async () => {
    const { port, update } = createPort();
    const service = createUpdateService({ enabled: () => true, port });
    await service.checkForUpdate();
    await service.downloadAvailableUpdate();

    const result = await service.checkForUpdate();

    expect(result.phase).toBe('ready');
    expect(port.check).toHaveBeenCalledOnce();
    expect(update.close).not.toHaveBeenCalled();
  });

  it('installs a downloaded update and relaunches', async () => {
    const { port, update } = createPort();
    const service = createUpdateService({ enabled: () => true, port });
    const phases: string[] = [];
    service.subscribe(snapshot => phases.push(snapshot.phase));
    await service.checkForUpdate();
    await service.downloadAvailableUpdate();

    const result = await service.installDownloadedUpdate();

    expect(phases).toContain('installing');
    expect(phases).toContain('completed');
    expect(result.phase).toBe('restarting');
    expect(update.install).toHaveBeenCalledOnce();
    expect(update.close).toHaveBeenCalledOnce();
    expect(port.relaunch).toHaveBeenCalledOnce();
  });

  it('reports when the user must restart after a successful installation', async () => {
    const { port } = createPort({ relaunch: vi.fn(async () => { throw new Error('restart failed'); }) });
    const service = createUpdateService({ enabled: () => true, port });
    await service.checkForUpdate();
    await service.downloadAvailableUpdate();

    const result = await service.installDownloadedUpdate();

    expect(result.phase).toBe('restart_required');
    expect(result.error).toBeNull();
  });

  it('checks automatically only when enabled', async () => {
    vi.useFakeTimers();
    const { port } = createPort();
    const service = createUpdateService({ enabled: () => true, port, initialCheckDelayMs: 50, checkIntervalMs: 100 });
    const stop = service.startAutomaticChecks();

    await vi.advanceTimersByTimeAsync(50);
    expect(port.check).toHaveBeenCalledOnce();
    stop();
  });

  it('uses a safe error message instead of displaying raw update service details', async () => {
    const { port } = createPort({ check: vi.fn(async () => { throw new Error('request failed for https://updates.example/private/path'); }) });
    const service = createUpdateService({ enabled: () => true, port });

    const result = await service.checkForUpdate();

    expect(result.phase).toBe('error');
    expect(result.error).toContain('Check your connection');
    expect(result.error).not.toContain('updates.example');
  });

  it('keeps update status normal when the release service is offline', async () => {
    const { port } = createPort({ check: vi.fn(async () => { throw new TypeError('fetch failed'); }) });
    const service = createUpdateService({ enabled: () => true, port });

    const result = await service.checkForUpdate();

    expect(result).toMatchObject({ phase: 'error', error: 'Could not reach the JevCode update service. Check your connection and try again.' });
    expect(() => service.getAvailableVersion()).not.toThrow();
    expect(service.getUpdateMetadata()).toBeNull();
  });
});
