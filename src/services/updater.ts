import { getVersion } from '@tauri-apps/api/app';
import { isTauri } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater';
import type { UpdatePreferences } from '../types/domain';

export type UpdatePhase = 'disabled' | 'idle' | 'checking' | 'up_to_date' | 'available' | 'downloading' | 'ready' | 'installing' | 'completed' | 'restarting' | 'restart_required' | 'error';

type UpdateOperation = 'check' | 'download' | 'install';

export interface UpdateMetadata {
  version: string;
  publishedAt: string | null;
  notes: string | null;
}

export interface UpdateSnapshot {
  phase: UpdatePhase;
  currentVersion: string | null;
  availableVersion: string | null;
  notes: string | null;
  publishedAt: string | null;
  downloadedBytes: number;
  contentLength: number | null;
  error: string | null;
}

export interface UpdaterPort {
  check(): Promise<Update | null>;
  getVersion(): Promise<string>;
  relaunch(): Promise<void>;
}

interface ServiceOptions {
  enabled: () => boolean;
  port: UpdaterPort;
  initialCheckDelayMs?: number;
  checkIntervalMs?: number;
}

const initialSnapshot = (): UpdateSnapshot => ({
  phase: 'idle',
  currentVersion: null,
  availableVersion: null,
  notes: null,
  publishedAt: null,
  downloadedBytes: 0,
  contentLength: null,
  error: null,
});

export class UpdateService {
  private snapshot = initialSnapshot();
  private pendingUpdate: Update | null = null;
  private busy = false;
  private preferences: UpdatePreferences = { automaticallyCheck: true, automaticallyDownload: false, channel: 'stable' };
  private agentActivityCheck: () => boolean = () => false;
  private readonly listeners = new Set<(snapshot: UpdateSnapshot) => void>();

  constructor(private readonly options: ServiceOptions) {}

  isSupported(): boolean {
    return this.options.enabled();
  }

  getSnapshot = (): UpdateSnapshot => this.snapshot;

  setPreferences(preferences: UpdatePreferences): void {
    this.preferences = preferences;
  }

  setAgentActivityCheck(check: () => boolean): void {
    this.agentActivityCheck = check;
  }

  async getCurrentVersion(): Promise<string | null> {
    try {
      const currentVersion = await this.options.port.getVersion();
      this.set({ ...this.snapshot, currentVersion });
      return currentVersion;
    } catch {
      return this.snapshot.currentVersion;
    }
  }

  getAvailableVersion(): string | null {
    return this.snapshot.availableVersion;
  }

  getUpdateMetadata(): UpdateMetadata | null {
    const version = this.getAvailableVersion();
    return version ? { version, publishedAt: this.snapshot.publishedAt, notes: this.snapshot.notes } : null;
  }

  subscribe = (listener: (snapshot: UpdateSnapshot) => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  startAutomaticChecks(): () => void {
    if (!this.isSupported() || !this.preferences.automaticallyCheck) return () => undefined;

    let active = true;
    const run = () => {
      if (active) void this.checkForUpdate();
    };
    const initial = setTimeout(run, this.options.initialCheckDelayMs ?? 10_000);
    const interval = setInterval(run, this.options.checkIntervalMs ?? 12 * 60 * 60 * 1000);
    return () => {
      active = false;
      clearTimeout(initial);
      clearInterval(interval);
    };
  }

  async checkForUpdate(): Promise<UpdateSnapshot> {
    if (!this.isSupported()) {
      this.set({ ...initialSnapshot(), phase: 'disabled' });
      return this.snapshot;
    }
    if (this.busy) return this.snapshot;
    if (['ready', 'restart_required'].includes(this.snapshot.phase)) return this.snapshot;

    this.busy = true;
    const retainedMetadata = this.snapshot.availableVersion ? {
      currentVersion: this.snapshot.currentVersion,
      availableVersion: this.snapshot.availableVersion,
      notes: this.snapshot.notes,
      publishedAt: this.snapshot.publishedAt,
    } : {};
    this.set({ ...initialSnapshot(), ...retainedMetadata, phase: 'checking' });
    await this.closePendingUpdate();
    try {
      const update = await this.options.port.check();
      if (update) {
        const invalidMetadata = validateUpdateMetadata(update);
        if (invalidMetadata) {
          await update.close().catch(() => undefined);
          this.set({ ...initialSnapshot(), ...retainedMetadata, phase: 'error', error: invalidMetadata });
        } else {
          this.pendingUpdate = update;
          this.set({
            ...initialSnapshot(),
            phase: 'available',
            currentVersion: normalizeVersion(update.currentVersion),
            availableVersion: normalizeVersion(update.version),
            notes: typeof update.body === 'string' ? update.body.slice(0, 20_000) : null,
            publishedAt: typeof update.date === 'string' ? update.date : null,
          });
        }
      } else {
        const currentVersion = await this.getCurrentVersion();
        this.set({ ...initialSnapshot(), phase: 'up_to_date', currentVersion });
      }
    } catch (error) {
      this.set({ ...initialSnapshot(), ...retainedMetadata, phase: 'error', error: friendlyUpdateError(error, 'check') });
    } finally {
      this.busy = false;
    }
    if (this.snapshot.phase === 'available' && this.preferences.automaticallyDownload) {
      return this.downloadAvailableUpdate();
    }
    return this.snapshot;
  }

  async downloadAvailableUpdate(): Promise<UpdateSnapshot> {
    if (!this.isSupported() || this.busy || !this.pendingUpdate) return this.snapshot;

    this.busy = true;
    const update = this.pendingUpdate;
    this.set({
      ...this.snapshot,
      phase: 'downloading',
      downloadedBytes: 0,
      contentLength: null,
      error: null,
    });

    try {
      await update.download((event) => this.onDownloadEvent(event));
      this.set({ ...this.snapshot, phase: 'ready' });
    } catch (error) {
      this.pendingUpdate = null;
      this.set({ ...this.snapshot, phase: 'error', error: friendlyUpdateError(error, 'download') });
      await update.close().catch(() => undefined);
    } finally {
      this.busy = false;
    }
    return this.snapshot;
  }

  async installDownloadedUpdate(): Promise<UpdateSnapshot> {
    if (!this.isSupported() || this.busy || !this.pendingUpdate || this.snapshot.phase !== 'ready') return this.snapshot;
    if (this.agentActivityCheck()) return this.snapshot;

    this.busy = true;
    const update = this.pendingUpdate;
    this.set({ ...this.snapshot, phase: 'installing', error: null });
    try {
      await update.install({ restartAfterInstall: false });
      this.pendingUpdate = null;
      this.set({ ...this.snapshot, phase: 'completed' });
      if (this.agentActivityCheck()) this.set({ ...this.snapshot, phase: 'restart_required' });
      else await this.relaunchOrRequireRestart();
    } catch (error) {
      this.pendingUpdate = null;
      this.set({ ...this.snapshot, phase: 'error', error: friendlyUpdateError(error, 'install') });
    } finally {
      this.pendingUpdate = null;
      await update.close().catch(() => undefined);
      this.busy = false;
    }
    return this.snapshot;
  }

  async restartApplication(): Promise<UpdateSnapshot> {
    if (!this.isSupported() || this.busy || this.snapshot.phase !== 'restart_required') return this.snapshot;
    if (this.agentActivityCheck()) return this.snapshot;
    this.busy = true;
    try {
      await this.options.port.relaunch();
      this.set({ ...this.snapshot, phase: 'restarting', error: null });
    } catch {
      this.set({ ...this.snapshot, phase: 'restart_required' });
    } finally {
      this.busy = false;
    }
    return this.snapshot;
  }

  private async relaunchOrRequireRestart(): Promise<void> {
    if (this.agentActivityCheck()) {
      this.set({ ...this.snapshot, phase: 'restart_required' });
      return;
    }
    try {
      await this.options.port.relaunch();
      this.set({ ...this.snapshot, phase: 'restarting' });
    } catch {
      this.set({ ...this.snapshot, phase: 'restart_required' });
    }
  }

  private onDownloadEvent(event: DownloadEvent): void {
    if (event.event === 'Started') {
      this.set({ ...this.snapshot, contentLength: event.data.contentLength ?? null });
    } else if (event.event === 'Progress') {
      this.set({ ...this.snapshot, downloadedBytes: this.snapshot.downloadedBytes + event.data.chunkLength });
    } else if (event.event === 'Finished') {
      this.set({ ...this.snapshot, downloadedBytes: this.snapshot.contentLength ?? this.snapshot.downloadedBytes });
    }
  }

  private async closePendingUpdate(): Promise<void> {
    const update = this.pendingUpdate;
    this.pendingUpdate = null;
    if (update) await update.close().catch(() => undefined);
  }

  private set(snapshot: UpdateSnapshot): void {
    this.snapshot = snapshot;
    for (const listener of this.listeners) listener(snapshot);
  }
}

function validateUpdateMetadata(update: Update): string | null {
  if (!isSemver(update.currentVersion) || !isSemver(update.version)) {
    return 'The update information is invalid, so JevCode did not install it. Try again later.';
  }
  if (update.body != null && typeof update.body !== 'string') {
    return 'The update information is invalid, so JevCode did not install it. Try again later.';
  }
  if (update.date != null && !isRfc3339Date(update.date)) {
    return 'The update information is invalid, so JevCode did not install it. Try again later.';
  }
  return null;
}

function isSemver(value: unknown): value is string {
  return typeof value === 'string' && /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(value);
}

function normalizeVersion(version: string): string {
  return version.startsWith('v') ? version.slice(1) : version;
}

function isRfc3339Date(value: unknown): value is string {
  return typeof value === 'string'
    && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/i.test(value)
    && !Number.isNaN(Date.parse(value));
}

function friendlyUpdateError(error: unknown, operation: UpdateOperation): string {
  const detail = error instanceof Error ? error.message.toLowerCase() : '';
  if (/(signature|verify|verification)/.test(detail)) {
    return 'The update signature was invalid. JevCode did not install it.';
  }
  if (/(manifest|metadata|invalid version|semver|parse update)/.test(detail)) {
    return 'The update information is invalid, so JevCode did not install it. Try again later.';
  }
  if (/(endpoint|configuration|config|public key)/.test(detail)) {
    return 'The updater is not configured for this build. Install the latest release from GitHub.';
  }
  if (/(timeout|network|dns|connect|fetch|request)/.test(detail)) {
    return 'Couldn’t connect to update server. Check your connection and try again.';
  }
  if (operation === 'download') return 'Update download failed. JevCode did not install it. Try again later.';
  if (operation === 'install') return 'Update installation failed. Your current version is still installed. Try again later.';
  return 'Couldn’t connect to update server. Try again later.';
}

export function createUpdateService(options: ServiceOptions): UpdateService {
  return new UpdateService(options);
}

export const updateService = createUpdateService({
  enabled: () => import.meta.env.PROD && isTauri(),
  port: { check, getVersion, relaunch },
});
