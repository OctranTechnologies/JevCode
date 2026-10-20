import { useEffect, useState } from 'react';
import { AlertCircle, Download, LoaderCircle, RefreshCw, RotateCw } from 'lucide-react';
import { updateService, type UpdateSnapshot } from '../../services/updater';
import { ReleaseNotes } from './ReleaseNotes';

export function UpdateNotice({ agentActive }: { agentActive: boolean }) {
  const [update, setUpdate] = useState<UpdateSnapshot>(() => updateService.getSnapshot());
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => updateService.subscribe(snapshot => {
    setUpdate(snapshot);
    if (!snapshot.error) setActionError(null);
  }), []);

  if (!updateService.shouldShowNotice()) return null;
  const version = update.availableVersion;
  const progress = update.contentLength && update.contentLength > 0
    ? Math.min(100, Math.round((update.downloadedBytes / update.contentLength) * 100))
    : null;
  const ready = update.phase === 'ready';
  const needsRestart = update.phase === 'restart_required';
  const downloading = update.phase === 'downloading';
  const actionBlocked = agentActive && (ready || needsRestart);

  async function remindLater() {
    setActionError(null);
    if (!await updateService.remindMeLater()) setActionError(updateService.getSnapshot().error ?? 'The reminder could not be saved. Try again.');
  }

  async function skipVersion() {
    setActionError(null);
    if (!await updateService.skipAvailableVersion()) setActionError(updateService.getSnapshot().error ?? 'The skipped version could not be saved. Try again.');
  }

  async function updateNow() {
    setActionError(null);
    await updateService.downloadAvailableUpdate();
  }

  async function restartAndUpdate() {
    setActionError(null);
    if (ready) await updateService.installDownloadedUpdate();
    else await updateService.restartApplication();
  }

  return <aside className="update-notice" role="status" aria-live="polite">
    <div className="update-notice-icon" aria-hidden="true">
      {downloading ? <LoaderCircle size={15} className="spin" /> : needsRestart ? <RotateCw size={15} /> : <Download size={15} />}
    </div>
    <div className="update-notice-copy">
      <strong>{ready || needsRestart ? `Update v${version} ready` : downloading ? `Updating JevCode to v${version}` : `JevCode v${version} is available`}</strong>
      {downloading ? <>
        <span>{progress === null ? 'Downloading the signed update…' : `${progress}% · ${formatBytes(update.downloadedBytes)}${update.contentLength === null ? ' downloaded' : ` of ${formatBytes(update.contentLength)}`}`}</span>
        <progress max={100} value={progress ?? undefined} aria-label="Update download progress" />
      </> : ready ? <span>{agentActive ? 'Finish the active task before installing and restarting.' : 'The signed download is ready to install.'}</span>
        : needsRestart ? <span>{agentActive ? 'Finish the active task before restarting JevCode.' : 'The update is installed and ready to restart.'}</span>
          : <span>Review the release notes before updating.</span>}
      {version && (update.notes || update.phase === 'available') && <ReleaseNotes version={version} body={update.notes} publishedAt={update.publishedAt} className="update-notice-notes" />}
      {actionBlocked && <span className="update-notice-hold"><AlertCircle size={12} />The agent is working. JevCode will wait before restarting.</span>}
      {actionError && <span className="update-notice-error" role="alert">{actionError}</span>}
    </div>
    <div className="update-notice-actions">
      {update.phase === 'available' && <>
        <button type="button" className="account-button quiet" onClick={() => void remindLater()}>Remind Me Later</button>
        <button type="button" className="account-button connect" onClick={() => void updateNow()}><Download size={13} />Update Now</button>
        <button type="button" className="update-notice-skip" onClick={() => void skipVersion()}>Skip this version</button>
      </>}
      {(ready || needsRestart) && <button type="button" className="account-button connect" disabled={agentActive} onClick={() => void restartAndUpdate()}>
        {ready ? <Download size={13} /> : <RefreshCw size={13} />}Restart and Update
      </button>}
    </div>
  </aside>;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return `${value.toFixed(1)} ${units[unit]}`;
}
