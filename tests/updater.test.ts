import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DownloadEvent, Update } from '@tauri-apps/plugin-updater';
import { createUpdateService, type UpdaterPort } from '../src/services/updater';
import { defaultUpdatePreferences, type UpdatePreferences } from '../src/types/domain';

function preferences(overrides: Partial<UpdatePreferences> = {}): UpdatePreferences {
  return { ...defaultUpdatePreferences, ...overrides };
}

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
    expect(service.getPreferences().downloadRequestedVersion).toBe('0.2.0');
    expect(update.install).not.toHaveBeenCalled();
  });

  it('streams intermediate download progress and the byte count without blocking the service', async () => {
    const update = makeUpdate({
      download: vi.fn(async (onEvent?: (event: DownloadEvent) => void) => {
        onEvent?.({ event: 'Started', data: { contentLength: 100 } });
        onEvent?.({ event: 'Progress', data: { chunkLength: 72 } });
        await Promise.resolve();
        onEvent?.({ event: 'Progress', data: { chunkLength: 28 } });
        onEvent?.({ event: 'Finished' });
      }),
    });
    const { port } = createPort({ check: vi.fn(async () => update) });
    const service = createUpdateService({ enabled: () => true, port });
    const progress: number[] = [];
    service.subscribe(snapshot => progress.push(snapshot.downloadedBytes));
    await service.checkForUpdate();

    const result = await service.downloadAvailableUpdate();

    expect(progress).toContain(72);
    expect(result).toMatchObject({ phase: 'ready', downloadedBytes: 100, contentLength: 100 });
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
    expect(update.install).toHaveBeenCalledWith({ restartAfterInstall: false });
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

  it('allows retrying a required restart', async () => {
    const relaunch = vi.fn().mockRejectedValueOnce(new Error('restart failed')).mockResolvedValueOnce(undefined);
    const { port } = createPort({ relaunch });
    const service = createUpdateService({ enabled: () => true, port });
    await service.checkForUpdate();
    await service.downloadAvailableUpdate();
    await service.installDownloadedUpdate();

    const result = await service.restartApplication();

    expect(result.phase).toBe('restarting');
    expect(relaunch).toHaveBeenCalledTimes(2);
  });

  it('does not install a failed or invalidly signed download', async () => {
    const signedUpdate = makeUpdate({
      download: vi.fn(async () => { throw new Error('signature verification failed'); }),
    });
    const { port } = createPort({ check: vi.fn(async () => signedUpdate) });
    const service = createUpdateService({ enabled: () => true, port });
    await service.checkForUpdate();

    const result = await service.downloadAvailableUpdate();

    expect(result).toMatchObject({ phase: 'error', error: 'The update signature was invalid. JevCode did not install it.' });
    expect(signedUpdate?.install).not.toHaveBeenCalled();
  });

  it('reports a failed download with a useful message and never attempts installation', async () => {
    const update = makeUpdate({ download: vi.fn(async () => { throw new Error('installer package unavailable'); }) });
    const { port } = createPort({ check: vi.fn(async () => update) });
    const service = createUpdateService({ enabled: () => true, port });
    await service.checkForUpdate();

    const result = await service.downloadAvailableUpdate();

    expect(result.phase).toBe('error');
    expect(result.error).toBe('Update download failed. JevCode did not install it. Try again later.');
    expect(update.install).not.toHaveBeenCalled();
  });

  it('rejects invalid update versions and release metadata before downloading', async () => {
    const update = makeUpdate({ version: 'latest', date: 'not a date' });
    const { port } = createPort({ check: vi.fn(async () => update) });
    const service = createUpdateService({ enabled: () => true, port });

    const result = await service.checkForUpdate();

    expect(result).toMatchObject({ phase: 'error', error: 'The update information is invalid, so JevCode did not install it. Try again later.' });
    expect(update.close).toHaveBeenCalledOnce();
    expect(update.download).not.toHaveBeenCalled();
  });

  it('redacts updater details when installation fails', async () => {
    const update = makeUpdate({ install: vi.fn(async () => { throw new Error('private installer path C:/temp/secret.msi'); }) });
    const { port } = createPort({ check: vi.fn(async () => update) });
    const service = createUpdateService({ enabled: () => true, port });
    await service.checkForUpdate();
    await service.downloadAvailableUpdate();

    const result = await service.installDownloadedUpdate();

    expect(result.error).toBe('Update installation failed. Your current version is still installed. Try again later.');
    expect(result.error).not.toContain('secret.msi');
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

  it('does not schedule automatic checks when the saved preference is off', async () => {
    vi.useFakeTimers();
    const { port } = createPort();
    const service = createUpdateService({ enabled: () => true, port, initialCheckDelayMs: 50, checkIntervalMs: 100 });
    service.setPreferences(preferences({ automaticallyCheck: false }));

    const stop = service.startAutomaticChecks();
    await vi.advanceTimersByTimeAsync(500);

    expect(port.check).not.toHaveBeenCalled();
    stop();
  });

  it('downloads an update in the background when automatic download is enabled', async () => {
    const { port, update } = createPort();
    const service = createUpdateService({ enabled: () => true, port });
    service.setPreferences(preferences({ automaticallyDownload: true }));

    const result = await service.checkForUpdate();

    expect(result.phase).toBe('ready');
    expect(update.download).toHaveBeenCalledOnce();
    expect(update.install).not.toHaveBeenCalled();
    expect(port.relaunch).not.toHaveBeenCalled();
  });

  it('does not check or download twice when an update check is already running', async () => {
    let finish!: (update: Update | null) => void;
    const update = makeUpdate();
    const check = vi.fn(() => new Promise<Update | null>(resolve => { finish = resolve; }));
    const { port } = createPort({ check });
    const service = createUpdateService({ enabled: () => true, port });

    const first = service.checkForUpdate();
    const second = await service.checkForUpdate();
    finish(update);
    const result = await first;

    expect(second.phase).toBe('checking');
    expect(result.phase).toBe('available');
    expect(check).toHaveBeenCalledOnce();
  });

  it('waits to install or restart while an agent task is active', async () => {
    const { port, update } = createPort();
    const service = createUpdateService({ enabled: () => true, port });
    let agentActive = true;
    service.setAgentActivityCheck(() => agentActive);
    await service.checkForUpdate();
    await service.downloadAvailableUpdate();

    const blockedInstall = await service.installDownloadedUpdate();

    expect(blockedInstall.phase).toBe('ready');
    expect(update.install).not.toHaveBeenCalled();
    agentActive = false;
    const installed = await service.installDownloadedUpdate();
    expect(installed.phase).toBe('restarting');
    expect(port.relaunch).toHaveBeenCalledOnce();
  });

  it('does not relaunch if an agent becomes active during installation', async () => {
    const { port, update } = createPort();
    let agentActive = false;
    update.install = vi.fn(async () => { agentActive = true; });
    const service = createUpdateService({ enabled: () => true, port });
    service.setAgentActivityCheck(() => agentActive);
    await service.checkForUpdate();
    await service.downloadAvailableUpdate();

    const result = await service.installDownloadedUpdate();

    expect(result.phase).toBe('restart_required');
    expect(port.relaunch).not.toHaveBeenCalled();
    agentActive = false;
    expect((await service.restartApplication()).phase).toBe('restarting');
    expect(port.relaunch).toHaveBeenCalledOnce();
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

    expect(result).toMatchObject({ phase: 'error', error: 'Couldn’t connect to update server. Check your connection and try again.' });
    expect(() => service.getAvailableVersion()).not.toThrow();
    expect(service.getUpdateMetadata()).toBeNull();
  });

  it('persists Remind Me Later and resurfaces the update after 24 hours', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-10-17T12:00:00Z'));
    let saved = defaultUpdatePreferences;
    const persistPreferences = vi.fn(async (next: UpdatePreferences) => {
      saved = next;
      return saved;
    });
    const { port } = createPort();
    const service = createUpdateService({ enabled: () => true, port, persistPreferences });
    await service.checkForUpdate();

    expect(service.shouldShowNotice()).toBe(true);
    expect(await service.remindMeLater()).toBe(true);
    expect(saved).toMatchObject({ deferredVersion: '0.2.0', downloadRequestedVersion: null });
    expect(service.shouldShowNotice()).toBe(false);
    expect(service.hasPendingUpdate()).toBe(false);

    const reopened = createUpdateService({ enabled: () => true, port: createPort().port });
    reopened.setPreferences(saved);
    await reopened.checkForUpdate();
    expect(reopened.shouldShowNotice()).toBe(false);

    await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000);

    expect(service.shouldShowNotice()).toBe(true);
    expect(service.hasPendingUpdate()).toBe(true);
    expect(reopened.shouldShowNotice()).toBe(true);
    expect(saved).toMatchObject({ deferredVersion: null, remindAfter: null });
    expect(persistPreferences).toHaveBeenCalledTimes(2);
  });

  it('restores a skipped version after restart and allows the user to show it again', async () => {
    let saved = defaultUpdatePreferences;
    const persistPreferences = vi.fn(async (next: UpdatePreferences) => {
      saved = next;
      return saved;
    });
    const first = createUpdateService({ enabled: () => true, port: createPort().port, persistPreferences });
    await first.checkForUpdate();
    expect(await first.skipAvailableVersion()).toBe(true);
    expect(saved.skippedVersion).toBe('0.2.0');

    const reopened = createUpdateService({ enabled: () => true, port: createPort().port });
    reopened.setPreferences(saved);
    await reopened.checkForUpdate();
    expect(reopened.shouldShowNotice()).toBe(false);
    expect(reopened.hasPendingUpdate()).toBe(false);

    expect(await reopened.showSkippedVersion()).toBe(true);
    expect(reopened.shouldShowNotice()).toBe(true);
  });

  it('restores a downloaded-update request and downloads it again after restart', async () => {
    vi.useFakeTimers();
    let saved = defaultUpdatePreferences;
    const persistPreferences = vi.fn(async (next: UpdatePreferences) => {
      saved = next;
      return saved;
    });
    const first = createUpdateService({ enabled: () => true, port: createPort().port, persistPreferences });
    await first.checkForUpdate();
    expect((await first.downloadAvailableUpdate()).phase).toBe('ready');
    expect(saved.downloadRequestedVersion).toBe('0.2.0');

    const { port, update } = createPort();
    const reopened = createUpdateService({ enabled: () => true, port, initialCheckDelayMs: 25 });
    reopened.setPreferences(preferences({ ...saved, automaticallyCheck: false }));
    const stop = reopened.startAutomaticChecks();
    await vi.advanceTimersByTimeAsync(25);

    expect(port.check).toHaveBeenCalledOnce();
    expect(update.download).toHaveBeenCalledOnce();
    expect(reopened.getSnapshot().phase).toBe('ready');
    expect(port.relaunch).not.toHaveBeenCalled();
    stop();
  });

  it('clears a skipped version when a newer release is found', async () => {
    const { port } = createPort({ check: vi.fn(async () => makeUpdate({ version: '0.3.0' })) });
    const service = createUpdateService({ enabled: () => true, port });
    service.setPreferences(preferences({ skippedVersion: '0.2.0' }));

    const result = await service.checkForUpdate();

    expect(result).toMatchObject({ phase: 'available', availableVersion: '0.3.0', skippedVersion: null });
    expect(service.shouldShowNotice()).toBe(true);
  });
});
