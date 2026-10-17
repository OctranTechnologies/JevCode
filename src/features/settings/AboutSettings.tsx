import { useEffect, useState } from 'react';
import { Check, Download, ExternalLink, LoaderCircle, RefreshCw, Sparkles } from 'lucide-react';
import { Brand } from '../../components/Brand';
import { command } from '../../lib/ipc';
import { updateService, type UpdateSnapshot } from '../../services/updater';

export function AboutSettings({ online }: { online: boolean }) {
  const [update, setUpdate] = useState<UpdateSnapshot>(() => updateService.getSnapshot());
  const [releaseError, setReleaseError] = useState<string | null>(null);

  useEffect(() => {
    const unsubscribe = updateService.subscribe(setUpdate);
    void updateService.getCurrentVersion();
    return unsubscribe;
  }, []);

  async function openRelease() {
    setReleaseError(null);
    try { await command('open_latest_release', undefined); }
    catch { setReleaseError('The release page could not be opened.'); }
  }

  const supported = updateService.isSupported();
  const metadata = updateService.getUpdateMetadata();
  const latestVersion = metadata?.version ?? (update.phase === 'up_to_date' ? update.currentVersion : null);
  const busy = ['checking', 'downloading', 'ready', 'installing', 'restarting'].includes(update.phase);

  return <section className="settings-content about-settings" aria-labelledby="about-title">
    <div className="settings-heading-row">
      <div><h1 id="about-title">About</h1><p className="page-description">JevCode desktop application and signed release updates.</p></div>
    </div>

    <div className="about-identity">
      <Brand />
      <div><strong>JevCode</strong><span>AI coding workspace</span></div>
      <span className="about-current-version">{update.currentVersion ? `v${update.currentVersion}` : 'Version unavailable'}</span>
    </div>

    <section className="about-update-section" aria-labelledby="about-updates-title">
      <div className="about-section-heading"><div><h2 id="about-updates-title">Software updates</h2><p>Release packages are verified with JevCode’s updater signing key before installation.</p></div></div>
      <dl className="about-version-details">
        <div><dt>Current version</dt><dd>{update.currentVersion ? `v${update.currentVersion}` : '—'}</dd></div>
        <div><dt>Latest version</dt><dd>{latestVersion ? `v${latestVersion}` : '—'}</dd></div>
      </dl>

      <div className={`about-update-status is-${update.phase}`} role={update.phase === 'error' ? 'alert' : 'status'}>
        {['checking', 'downloading', 'installing', 'restarting'].includes(update.phase)
          ? <LoaderCircle size={15} className="spin" />
          : update.phase === 'up_to_date' || update.phase === 'completed' ? <Check size={15} /> : <Sparkles size={15} />}
        <div>
          <strong>{statusTitle(update)}</strong>
          <span>{statusDescription(update, online, supported)}</span>
        </div>
      </div>

      {metadata && <div className="about-release-notes">
        <div><strong>JevCode v{metadata.version} available</strong>{metadata.publishedAt && <time dateTime={metadata.publishedAt}>{formatReleaseDate(metadata.publishedAt)}</time>}</div>
        {metadata.notes && <p>{metadata.notes}</p>}
      </div>}

      {update.phase === 'downloading' && <div className="about-download-progress" role="status">
        <div><span>Downloading update</span><span>{formatBytes(update.downloadedBytes)}{update.contentLength !== null ? ` of ${formatBytes(update.contentLength)}` : ''}</span></div>
        <progress max={update.contentLength ?? undefined} value={update.contentLength === null ? undefined : Math.min(update.downloadedBytes, update.contentLength)} aria-label="Update download progress" />
      </div>}

      <div className="about-update-actions">
        <button className="account-button connect" onClick={() => void updateService.checkForUpdate()} disabled={!online || !supported || busy}>
          {update.phase === 'checking' ? <LoaderCircle size={14} className="spin" /> : <RefreshCw size={14} />}
          Check for Updates
        </button>
        {update.phase === 'available' && <>
          <button className="account-button connect" onClick={() => void updateService.installAvailableUpdate()} disabled={!online || busy}><Download size={14} />Update Now</button>
          <button className="account-button quiet" onClick={() => void openRelease()}><ExternalLink size={14} />Release page</button>
        </>}
        {update.phase === 'ready' && <button className="account-button connect" onClick={() => void updateService.installDownloadedUpdate()}><Download size={14} />Install and restart</button>}
      </div>
      {releaseError && <p className="about-update-error" role="alert">{releaseError}</p>}
      {supported && !online && <p className="about-help">Reconnect to check for or download an update.</p>}
      {!supported && <p className="about-help">Update checks are available in installed release builds. Development builds do not contact the update service.</p>}
    </section>
  </section>;
}

function statusTitle(update: UpdateSnapshot): string {
  switch (update.phase) {
    case 'disabled': return 'Updates unavailable in development';
    case 'checking': return 'Checking for updates';
    case 'available': return `Version ${update.availableVersion} available`;
    case 'downloading': return 'Downloading update';
    case 'ready': return 'Update ready to install';
    case 'installing': return 'Installing update';
    case 'completed': return 'Update installed';
    case 'restarting': return 'Restarting JevCode';
    case 'restart_required': return 'Restart required';
    case 'error': return 'Update couldn’t be completed';
    case 'up_to_date': return 'You’re up to date';
    default: return 'Update status';
  }
}

function statusDescription(update: UpdateSnapshot, online: boolean, supported: boolean): string {
  if (update.phase === 'error') return update.error ?? 'The update service could not be reached. Try again later.';
  if (update.phase === 'available') return 'An update is ready. Choose Update Now to download and install it.';
  if (update.phase === 'ready') return 'The signed update has been downloaded and can now be installed.';
  if (update.phase === 'restart_required') return 'The update is installed. Close and reopen JevCode to finish.';
  if (update.phase === 'restarting') return 'The update is installed. The application is restarting.';
  if (update.phase === 'completed') return 'The update package was installed successfully.';
  if (update.phase === 'checking') return 'Contacting the signed release manifest…';
  if (update.phase === 'downloading') return 'The update is being downloaded and verified.';
  if (update.phase === 'installing') return 'Applying the verified update package…';
  if (!supported) return 'Run a release build to enable update checks.';
  if (!online) return 'You’re offline. Connect to the internet to check for updates.';
  if (update.phase === 'up_to_date') return 'You have the latest version available for this release channel.';
  return 'Check the release service for the latest version.';
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
