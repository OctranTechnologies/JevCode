import { useEffect, useState } from 'react';
import { AlertCircle, Check, Download, LoaderCircle, RefreshCw, ShieldCheck } from 'lucide-react';
import { command, desktopAvailable } from '../../lib/ipc';
import { updateService, type UpdateSnapshot } from '../../services/updater';
import type { UpdatePreferences } from '../../types/domain';

export function UpdatesSettings({ preferences, online, agentActive, onSaved }: {
  preferences: UpdatePreferences;
  online: boolean;
  agentActive: boolean;
  onSaved: (preferences: UpdatePreferences) => void;
}) {
  const [update, setUpdate] = useState<UpdateSnapshot>(() => updateService.getSnapshot());
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unsubscribe = updateService.subscribe(setUpdate);
    void updateService.getCurrentVersion();
    return unsubscribe;
  }, []);

  const supported = updateService.isSupported();
  const operationBusy = ['checking', 'downloading', 'ready', 'installing', 'restart_required', 'restarting'].includes(update.phase);
  const latestVersion = update.availableVersion ?? (update.phase === 'up_to_date' ? update.currentVersion : null);
  const progress = update.contentLength && update.contentLength > 0
    ? Math.min(100, Math.round((update.downloadedBytes / update.contentLength) * 100))
    : null;

  async function save(next: UpdatePreferences) {
    setSaving(true);
    setError(null);
    try {
      const persisted = await command('save_update_preferences', { input: next });
      onSaved(persisted);
    } catch {
      setError('Update preferences could not be saved. Try again.');
    } finally {
      setSaving(false);
    }
  }

  const status = statusCopy(update, supported, online);

  return <section className="settings-content updates-settings" aria-labelledby="updates-title">
    <div className="settings-heading-row">
      <div><h1 id="updates-title">Updates</h1><p className="page-description">Choose when JevCode checks and downloads signed releases.</p></div>
    </div>

    <section className="updates-preferences" aria-label="Update preferences">
      <UpdateSwitch
        title="Automatically check for updates"
        description="Check in the background after JevCode opens, then periodically."
        checked={preferences.automaticallyCheck}
        disabled={!desktopAvailable || saving}
        onChange={value => void save({ ...preferences, automaticallyCheck: value })}
      />
      <UpdateSwitch
        title="Automatically download updates"
        description="Download available releases in the background. JevCode waits for you before installing or restarting."
        checked={preferences.automaticallyDownload}
        disabled={!desktopAvailable || saving}
        onChange={value => void save({ ...preferences, automaticallyDownload: value })}
      />
      <div className="update-channel-row"><div><strong>Update channel</strong><span>Receive stable JevCode releases.</span></div><span className="update-channel-value">Stable</span></div>
    </section>

    <section className="update-version-section" aria-labelledby="update-status-heading">
      <div className="permission-section-heading">
        <div><h2 id="update-status-heading">Release status</h2><p>JevCode verifies each signed package before installation.</p></div>
        <ShieldCheck size={16} aria-hidden="true" />
      </div>
      <dl className="about-version-details">
        <div><dt>Current version</dt><dd>{update.currentVersion ? `v${update.currentVersion}` : '—'}</dd></div>
        <div><dt>Latest version</dt><dd>{latestVersion ? `v${latestVersion}` : '—'}</dd></div>
      </dl>

      <div className={`about-update-status is-${update.phase}`} role={update.phase === 'error' ? 'alert' : 'status'}>
        {update.phase === 'checking' || update.phase === 'downloading'
          ? <LoaderCircle size={15} className="spin" />
          : update.phase === 'up_to_date' || update.phase === 'completed' ? <Check size={15} />
            : update.phase === 'error' ? <AlertCircle size={15} /> : <Download size={15} />}
        <div><strong>{status.title}</strong><span>{status.description}</span></div>
      </div>

      {update.notes && update.availableVersion && <div className="about-release-notes">
        <div><strong>JevCode v{update.availableVersion}</strong>{update.publishedAt && <time dateTime={update.publishedAt}>{formatReleaseDate(update.publishedAt)}</time>}</div>
        <p>{update.notes}</p>
      </div>}

      {update.phase === 'downloading' && <div className="about-download-progress" role="status">
        <div><span>{progress === null ? 'Downloading update' : `Downloading update · ${progress}%`}</span><span>{formatBytes(update.downloadedBytes)}{update.contentLength !== null ? ` of ${formatBytes(update.contentLength)}` : ''}</span></div>
        <progress max={100} value={progress ?? undefined} aria-label="Update download progress" />
      </div>}

      {update.phase === 'ready' && agentActive && <p className="updates-restart-hold" role="status">Your task is still active. Finish it before installing and restarting JevCode.</p>}
      {update.phase === 'restart_required' && agentActive && <p className="updates-restart-hold" role="status">Your task is still active. Restart when the task is finished.</p>}

      <div className="about-update-actions">
        <button className="account-button connect" onClick={() => void updateService.checkForUpdate()} disabled={!supported || !online || operationBusy || saving}>
          {update.phase === 'checking' ? <LoaderCircle size={14} className="spin" /> : <RefreshCw size={14} />}
          Check for Updates
        </button>
        {update.phase === 'available' && <button className="account-button quiet" onClick={() => void updateService.downloadAvailableUpdate()} disabled={!online}><Download size={14} />Download</button>}
        {update.phase === 'ready' && <button className="account-button connect" onClick={() => void updateService.installDownloadedUpdate()} disabled={agentActive}><Download size={14} />Install and restart</button>}
        {update.phase === 'restart_required' && <button className="account-button connect" onClick={() => void updateService.restartApplication()} disabled={agentActive}><RefreshCw size={14} />Restart now</button>}
      </div>
      {error && <p className="about-update-error" role="alert">{error}</p>}
      {supported && !online && <p className="about-help">You’re offline. Reconnect to check for or download an update.</p>}
      {!supported && <p className="about-help">Update checks are available in installed release builds. Development builds do not contact the update service.</p>}
    </section>
  </section>;
}

function UpdateSwitch({ title, description, checked, disabled, onChange }: {
  title: string; description: string; checked: boolean; disabled: boolean; onChange: (checked: boolean) => void;
}) {
  return <label className="update-preference-row">
    <span><strong>{title}</strong><small>{description}</small></span>
    <input type="checkbox" role="switch" checked={checked} disabled={disabled} onChange={event => onChange(event.currentTarget.checked)} />
  </label>;
}

function statusCopy(update: UpdateSnapshot, supported: boolean, online: boolean): { title: string; description: string } {
  switch (update.phase) {
    case 'disabled': return { title: 'Updates unavailable in development', description: 'Run an installed release build to check for signed updates.' };
    case 'checking': return { title: 'Checking for updates', description: 'JevCode is checking the stable release channel in the background.' };
    case 'available': return { title: `Version ${update.availableVersion} is available`, description: 'Download the signed release when you’re ready.' };
    case 'downloading': return { title: 'Downloading update', description: 'The download runs in the background. JevCode will wait for your confirmation before installation.' };
    case 'ready': return { title: 'Update ready', description: 'The signed package is downloaded. Installing requires an explicit action and an idle agent.' };
    case 'installing': return { title: 'Installing update', description: 'The verified package is being applied.' };
    case 'restart_required': return { title: 'Restart required', description: 'The update is installed. Restart JevCode when your work is ready.' };
    case 'restarting': return { title: 'Restarting JevCode', description: 'The update is installed and the application is restarting.' };
    case 'completed': return { title: 'Update installed', description: 'The signed update package was installed successfully.' };
    case 'error': return { title: 'Update check failed', description: update.error ?? 'Couldn’t connect to the update server. Try again later.' };
    case 'up_to_date': return { title: 'You’re up to date', description: 'You have the latest version on the stable channel.' };
    default: return { title: supported ? 'Ready to check' : 'Updates unavailable', description: online ? 'Check the stable release channel for a newer version.' : 'Connect to the internet before checking for updates.' };
  }
}

function formatReleaseDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(date);
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return `${value.toFixed(1)} ${units[unit]}`;
}
