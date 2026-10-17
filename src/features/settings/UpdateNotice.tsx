import { useEffect, useState } from 'react';
import { AlertCircle, Download, LoaderCircle, RefreshCw, RotateCw, X } from 'lucide-react';
import { updateService, type UpdateSnapshot } from '../../services/updater';

export function UpdateNotice({ agentActive }: { agentActive: boolean }) {
  const [update, setUpdate] = useState<UpdateSnapshot>(() => updateService.getSnapshot());
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);

  useEffect(() => updateService.subscribe(setUpdate), []);

  if (!['available', 'downloading', 'ready', 'restart_required'].includes(update.phase)) return null;
  const version = update.availableVersion;
  if (update.phase === 'available' && version && dismissedVersion === version) return null;

  const progress = update.contentLength && update.contentLength > 0
    ? Math.min(100, Math.round((update.downloadedBytes / update.contentLength) * 100))
    : null;
  const ready = update.phase === 'ready';
  const needsRestart = update.phase === 'restart_required';
  const downloading = update.phase === 'downloading';

  return <aside className="update-notice" role="status" aria-live="polite">
    <div className="update-notice-icon" aria-hidden="true">
      {downloading ? <LoaderCircle size={15} className="spin" /> : needsRestart ? <RotateCw size={15} /> : <Download size={15} />}
    </div>
    <div className="update-notice-copy">
      <strong>{ready || needsRestart ? 'Update ready' : downloading ? `Downloading JevCode v${version}` : `JevCode v${version} is available`}</strong>
      {downloading ? <>
        <span>{progress === null ? 'Downloading in the background…' : `${progress}% · ${formatBytes(update.downloadedBytes)}${update.contentLength === null ? ' downloaded' : ` of ${formatBytes(update.contentLength)}`}`}</span>
        <progress max={100} value={progress ?? undefined} aria-label="Update download progress" />
      </> : ready ? <span>{agentActive ? 'Finish the active task before installing and restarting.' : 'The signed download is ready to install.'}</span>
        : needsRestart ? <span>{agentActive ? 'Finish the active task before restarting JevCode.' : 'Restart JevCode to finish installing the update.'}</span>
          : <span>{update.notes?.split(/\r?\n/)[0]?.slice(0, 130) || 'A signed release is ready to download.'}</span>}
      {(ready || needsRestart) && agentActive && <span className="update-notice-hold"><AlertCircle size={12} />Restart waits until the agent is idle.</span>}
    </div>
    <div className="update-notice-actions">
      {update.phase === 'available' && <>
        <button type="button" className="update-notice-dismiss" onClick={() => { if (version) setDismissedVersion(version); }} aria-label="Later" title="Later"><X size={14} /></button>
        <button type="button" className="account-button connect" onClick={() => void updateService.downloadAvailableUpdate()}><Download size={13} />Download</button>
      </>}
      {ready && <button type="button" className="account-button connect" disabled={agentActive} onClick={() => void updateService.installDownloadedUpdate()}><Download size={13} />Install and restart</button>}
      {needsRestart && <button type="button" className="account-button connect" disabled={agentActive} onClick={() => void updateService.restartApplication()}><RefreshCw size={13} />Restart now</button>}
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
