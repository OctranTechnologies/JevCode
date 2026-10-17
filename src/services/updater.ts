import { getVersion } from '@tauri-apps/api/app';
import { isTauri } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater';

export type UpdatePhase = 'disabled' | 'idle' | 'checking' | 'up_to_date' | 'available' | 'downloading' | 'ready' | 'installing' | 'completed' | 'restarting' | 'restart_required' | 'error';

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
  private readonly listeners = new Set<(snapshot: UpdateSnapshot) => void>();

  constructor(private readonly options: ServiceOptions) {}

  isSupported(): boolean {
    return this.options.enabled();
  }

  getSnapshot = (): UpdateSnapshot => this.snapshot;

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
    if (!this.isSupported()) return () => undefined;

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
    if (this.snapshot.phase === 'ready') return this.snapshot;

    this.busy = true;
    await this.closePendingUpdate();
    this.set({ ...initialSnapshot(), phase: 'checking' });
    try {
      const update = await this.options.port.check();
      if (update) {
        this.pendingUpdate = update;
        this.set({
          ...initialSnapshot(),
          phase: 'available',
          currentVersion: update.currentVersion,
          availableVersion: update.version,
          notes: update.body ?? null,
          publishedAt: update.date ?? null,
        });
      } else {
        const currentVersion = await this.getCurrentVersion();
        this.set({ ...initialSnapshot(), phase: 'up_to_date', currentVersion });
      }
    } catch (error) {
      this.set({ ...initialSnapshot(), phase: 'error', error: friendlyUpdateError(error) });
    } finally {
      this.busy = false;
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
      this.set({ ...this.snapshot, phase: 'error', error: friendlyUpdateError(error) });
      await update.close().catch(() => undefined);
    } finally {
      this.busy = false;
    }
    return this.snapshot;
  }

  async installDownloadedUpdate(): Promise<UpdateSnapshot> {
    if (!this.isSupported() || this.busy || !this.pendingUpdate || this.snapshot.phase !== 'ready') return this.snapshot;

    this.busy = true;
    const update = this.pendingUpdate;
    this.set({ ...this.snapshot, phase: 'installing', error: null });
    try {
      await update.install();
      this.pendingUpdate = null;
      this.set({ ...this.snapshot, phase: 'completed' });
      try {
        await this.options.port.relaunch();
        this.set({ ...this.snapshot, phase: 'restarting' });
      } catch {
        this.set({ ...this.snapshot, phase: 'restart_required' });
      }
    } catch (error) {
      this.pendingUpdate = null;
      this.set({ ...this.snapshot, phase: 'error', error: friendlyUpdateError(error) });
    } finally {
      this.pendingUpdate = null;
      await update.close().catch(() => undefined);
      this.busy = false;
    }
    return this.snapshot;
  }

  async installAvailableUpdate(): Promise<UpdateSnapshot> {
    const downloaded = await this.downloadAvailableUpdate();
    return downloaded.phase === 'ready' ? this.installDownloadedUpdate() : downloaded;
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

function friendlyUpdateError(error: unknown): string {
  const detail = error instanceof Error ? error.message.toLowerCase() : '';
  if (/(signature|verify)/.test(detail)) {
    return 'JevCode could not verify the update signature. Do not install this update; contact the maintainer.';
  }
  if (/(timeout|network|dns|connect|fetch|request)/.test(detail)) {
    return 'Could not reach the JevCode update service. Check your connection and try again.';
  }
  if (/(endpoint|configuration|config|public key)/.test(detail)) {
    return 'The updater is not configured for this build. Install the latest release from GitHub.';
  }
  return 'The update could not be completed. Retry later or install the latest release from GitHub.';
}

export function createUpdateService(options: ServiceOptions): UpdateService {
  return new UpdateService(options);
}

export const updateService = createUpdateService({
  enabled: () => import.meta.env.PROD && isTauri(),
  port: { check, getVersion, relaunch },
});
