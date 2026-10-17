import { getVersion } from '@tauri-apps/api/app';
import { isTauri } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater';
import { command } from '../lib/ipc';
import { defaultUpdatePreferences, type UpdatePreferences } from '../types/domain';

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
  deferredVersion: string | null;
  skippedVersion: string | null;
  remindAfter: string | null;
  downloadRequestedVersion: string | null;
}

export interface UpdaterPort {
  check(): Promise<Update | null>;
  getVersion(): Promise<string>;
  relaunch(): Promise<void>;
}

interface ServiceOptions {
  enabled: () => boolean;
  port: UpdaterPort;
  persistPreferences?: (preferences: UpdatePreferences) => Promise<UpdatePreferences>;
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
  deferredVersion: null,
  skippedVersion: null,
  remindAfter: null,
  downloadRequestedVersion: null,
});

export class UpdateService {
  private snapshot = initialSnapshot();
  private pendingUpdate: Update | null = null;
  private busy = false;
  private preferences: UpdatePreferences = defaultUpdatePreferences;
  private reminderTimeout: ReturnType<typeof setTimeout> | null = null;
  private agentActivityCheck: () => boolean = () => false;
  private readonly listeners = new Set<(snapshot: UpdateSnapshot) => void>();

  constructor(private readonly options: ServiceOptions) {}

  isSupported(): boolean {
    return this.options.enabled();
  }

  getSnapshot = (): UpdateSnapshot => this.snapshot;

  getPreferences(): UpdatePreferences {
    return this.preferences;
  }

  setPreferences(preferences: UpdatePreferences): void {
    this.preferences = { ...defaultUpdatePreferences, ...preferences };
    this.scheduleReminderNotification();
    this.set({ ...this.snapshot });
  }

  shouldShowNotice(): boolean {
    if (!['available', 'downloading', 'ready', 'restart_required'].includes(this.snapshot.phase)) return false;
    return this.snapshot.phase !== 'available' || !this.isSuppressed(this.snapshot.availableVersion);
  }

  hasPendingUpdate(): boolean {
    if (!['available', 'downloading', 'ready', 'restart_required'].includes(this.snapshot.phase)) return false;
    return this.snapshot.phase !== 'available' || !this.isSuppressed(this.snapshot.availableVersion);
  }

  async remindMeLater(): Promise<boolean> {
    const version = this.snapshot.availableVersion;
    if (!version || this.snapshot.phase !== 'available') return false;
    return this.saveDisposition({
      ...this.preferences,
      deferredVersion: version,
      remindAfter: new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString(),
      downloadRequestedVersion: null,
    });
  }

  async skipAvailableVersion(): Promise<boolean> {
    const version = this.snapshot.availableVersion;
    if (!version || this.snapshot.phase !== 'available') return false;
    return this.saveDisposition({
      ...this.preferences,
      deferredVersion: null,
      remindAfter: null,
      skippedVersion: version,
      downloadRequestedVersion: null,
    });
  }

  async showSkippedVersion(): Promise<boolean> {
    if (!this.preferences.skippedVersion && !this.preferences.deferredVersion) return false;
    return this.saveDisposition({
      ...this.preferences,
      skippedVersion: null,
      deferredVersion: null,
      remindAfter: null,
    });
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
    if (!this.isSupported() || (!this.preferences.automaticallyCheck && !this.preferences.downloadRequestedVersion)) return () => undefined;

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
          const availableVersion = normalizeVersion(update.version);
          const staleDownload = this.preferences.downloadRequestedVersion && this.preferences.downloadRequestedVersion !== availableVersion;
          const staleReminder = this.preferences.deferredVersion && this.preferences.deferredVersion !== availableVersion;
          const staleSkip = this.preferences.skippedVersion && this.preferences.skippedVersion !== availableVersion;
          if (staleDownload || staleReminder || staleSkip) {
            await this.saveDisposition({
              ...this.preferences,
              deferredVersion: staleReminder ? null : this.preferences.deferredVersion,
              remindAfter: staleReminder ? null : this.preferences.remindAfter,
              skippedVersion: staleSkip ? null : this.preferences.skippedVersion,
              downloadRequestedVersion: staleDownload ? null : this.preferences.downloadRequestedVersion,
            });
          }
          this.set({
            ...initialSnapshot(),
            phase: 'available',
            currentVersion: normalizeVersion(update.currentVersion),
            availableVersion,
            notes: typeof update.body === 'string' ? update.body.slice(0, 20_000) : null,
            publishedAt: typeof update.date === 'string' ? update.date : null,
          });
        }
      } else {
        const currentVersion = await this.getCurrentVersion();
        if (this.preferences.downloadRequestedVersion) {
          await this.saveDisposition({ ...this.preferences, downloadRequestedVersion: null });
        }
        this.set({ ...initialSnapshot(), phase: 'up_to_date', currentVersion });
      }
    } catch (error) {
      this.set({ ...initialSnapshot(), ...retainedMetadata, phase: 'error', error: friendlyUpdateError(error, 'check') });
    } finally {
      this.busy = false;
    }
    if (this.snapshot.phase === 'available'
      && (this.preferences.downloadRequestedVersion === this.snapshot.availableVersion
        || (this.preferences.automaticallyDownload && !this.isSuppressed(this.snapshot.availableVersion)))) {
      return this.downloadAvailableUpdate();
    }
    return this.snapshot;
  }

  async downloadAvailableUpdate(): Promise<UpdateSnapshot> {
    if (!this.isSupported() || this.busy || !this.pendingUpdate) return this.snapshot;

    this.busy = true;
    const update = this.pendingUpdate;
    const version = this.snapshot.availableVersion;
    if (version && (this.preferences.deferredVersion === version || this.preferences.skippedVersion === version)) {
      const saved = await this.saveDisposition({
        ...this.preferences,
        deferredVersion: null,
        remindAfter: null,
        skippedVersion: this.preferences.skippedVersion === version ? null : this.preferences.skippedVersion,
        downloadRequestedVersion: version,
      });
      if (!saved) {
        this.busy = false;
        return this.snapshot;
      }
    } else if (version && this.preferences.downloadRequestedVersion !== version) {
      const saved = await this.saveDisposition({ ...this.preferences, downloadRequestedVersion: version });
      if (!saved) {
        this.busy = false;
        return this.snapshot;
      }
    }
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
      await this.saveDisposition({ ...this.preferences, downloadRequestedVersion: null });
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
      await this.saveDisposition({ ...this.preferences, downloadRequestedVersion: null });
      this.set({ ...this.snapshot, phase: 'completed' });
      if (this.agentActivityCheck()) this.set({ ...this.snapshot, phase: 'restart_required' });
      else await this.relaunchOrRequireRestart();
    } catch (error) {
      this.pendingUpdate = null;
      await this.saveDisposition({ ...this.preferences, downloadRequestedVersion: null });
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

  private isSuppressed(version: string | null): boolean {
    if (!version) return false;
    if (this.preferences.skippedVersion === version) return true;
    if (this.preferences.deferredVersion !== version || !this.preferences.remindAfter) return false;
    const remindAfter = Date.parse(this.preferences.remindAfter);
    return Number.isFinite(remindAfter) && remindAfter > Date.now();
  }

  private async saveDisposition(preferences: UpdatePreferences): Promise<boolean> {
    try {
      const saved = this.options.persistPreferences
        ? await this.options.persistPreferences(preferences)
        : preferences;
      this.preferences = { ...defaultUpdatePreferences, ...saved };
      this.scheduleReminderNotification();
      this.set({ ...this.snapshot });
      return true;
    } catch {
      this.set({ ...this.snapshot, error: 'Update preferences could not be saved. Try again.' });
      return false;
    }
  }

  private scheduleReminderNotification(): void {
    if (this.reminderTimeout) clearTimeout(this.reminderTimeout);
    this.reminderTimeout = null;
    if (!this.preferences.deferredVersion || !this.preferences.remindAfter) return;

    const remindAt = Date.parse(this.preferences.remindAfter);
    if (!Number.isFinite(remindAt)) return;
    const delay = Math.max(0, Math.min(remindAt - Date.now(), 2_147_000_000));
    this.reminderTimeout = setTimeout(() => {
      this.reminderTimeout = null;
      if (remindAt > Date.now()) {
        this.scheduleReminderNotification();
      } else {
        void this.saveDisposition({ ...this.preferences, deferredVersion: null, remindAfter: null });
      }
    }, delay);
  }

  private set(snapshot: UpdateSnapshot): void {
    this.snapshot = {
      ...snapshot,
      deferredVersion: this.preferences.deferredVersion,
      skippedVersion: this.preferences.skippedVersion,
      remindAfter: this.preferences.remindAfter,
      downloadRequestedVersion: this.preferences.downloadRequestedVersion,
    };
    for (const listener of this.listeners) listener(this.snapshot);
  }
}

function validateUpdateMetadata(update: Update): string | null {
  if (!isSemver(update.currentVersion) || !isSemver(update.version)) {
    return 'The update information is invalid, so JevCode did not install it. Try again later.';
  }
  if (compareSemver(update.version, update.currentVersion) <= 0) {
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
  return parseSemver(value) !== null;
}

function parseSemver(value: unknown): { major: bigint; minor: bigint; patch: bigint; prerelease: string[] } | null {
  if (typeof value !== 'string' || value.length > 128) return null;
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(value);
  if (!match) return null;
  const prerelease = match[4]?.split('.') ?? [];
  if (prerelease.some(identifier => /^\d+$/.test(identifier) && identifier.length > 1 && identifier.startsWith('0'))) return null;
  return { major: BigInt(match[1]), minor: BigInt(match[2]), patch: BigInt(match[3]), prerelease };
}

function compareSemver(left: string, right: string): number {
  const a = parseSemver(left);
  const b = parseSemver(right);
  if (!a || !b) return Number.NEGATIVE_INFINITY;
  for (const key of ['major', 'minor', 'patch'] as const) {
    if (a[key] < b[key]) return -1;
    if (a[key] > b[key]) return 1;
  }
  if (a.prerelease.length === 0 || b.prerelease.length === 0) {
    if (a.prerelease.length === b.prerelease.length) return 0;
    return a.prerelease.length === 0 ? 1 : -1;
  }
  const sharedLength = Math.min(a.prerelease.length, b.prerelease.length);
  for (let index = 0; index < sharedLength; index += 1) {
    const leftIdentifier = a.prerelease[index];
    const rightIdentifier = b.prerelease[index];
    const leftNumeric = /^\d+$/.test(leftIdentifier);
    const rightNumeric = /^\d+$/.test(rightIdentifier);
    if (leftNumeric && rightNumeric) {
      const leftNumber = BigInt(leftIdentifier);
      const rightNumber = BigInt(rightIdentifier);
      if (leftNumber < rightNumber) return -1;
      if (leftNumber > rightNumber) return 1;
    } else if (leftNumeric !== rightNumeric) {
      return leftNumeric ? -1 : 1;
    } else if (leftIdentifier !== rightIdentifier) {
      return leftIdentifier < rightIdentifier ? -1 : 1;
    }
  }
  return Math.sign(a.prerelease.length - b.prerelease.length);
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
  persistPreferences: preferences => command('save_update_preferences', { input: preferences }),
});
