import { useEffect, useRef, useState } from 'react';
import { AlertCircle, Check, Download, LoaderCircle, RotateCw } from 'lucide-react';
import { updateService, type UpdatePhase, type UpdateSnapshot } from '../../services/updater';

const activePhases: UpdatePhase[] = ['downloading', 'ready', 'installing', 'restarting'];

export function UpdateDialog() {
  const dialog = useRef<HTMLDialogElement>(null);
  const [update, setUpdate] = useState<UpdateSnapshot>(() => updateService.getSnapshot());
  const [visible, setVisible] = useState(false);
  const [deferredVersion, setDeferredVersion] = useState<string | null>(null);

  useEffect(() => updateService.subscribe(setUpdate), []);

  useEffect(() => {
    const version = update.availableVersion;
    if (!version) {
      if (visible) setVisible(false);
      return;
    }
    if (version === deferredVersion) return;
    if (update.phase === 'available' || update.phase === 'restart_required' || update.phase === 'error') {
      setVisible(true);
    }
  }, [deferredVersion, update.availableVersion, update.phase, visible]);

  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    if (visible && !element.open) element.showModal();
    else if (!visible && element.open) element.close();
  }, [visible]);

  function dismissOrHide() {
    if (update.availableVersion && !activePhases.includes(update.phase)) {
      setDeferredVersion(update.availableVersion);
    }
    setVisible(false);
  }

  const version = update.availableVersion;
  if (!version) return <dialog ref={dialog} className="update-dialog" aria-labelledby="update-dialog-title" onClose={() => setVisible(false)} />;

  const percent = update.contentLength && update.contentLength > 0
    ? Math.min(100, Math.round((update.downloadedBytes / update.contentLength) * 100))
    : null;
  const downloading = update.phase === 'downloading';
  const failed = update.phase === 'error';
  const restartRequired = update.phase === 'restart_required';
  const active = ['checking', 'downloading', 'ready', 'installing', 'restarting'].includes(update.phase);
  const title = failed
    ? 'Update couldn’t be completed'
    : restartRequired
      ? 'Restart required'
      : update.phase === 'checking'
        ? 'Checking for updates'
        : downloading || update.phase === 'ready' || update.phase === 'installing' || update.phase === 'restarting'
        ? `Updating to v${version}`
        : update.phase === 'completed'
          ? 'Update installed'
          : 'Update Available';
  const subtitle = failed
    ? 'JevCode did not install this update.'
    : update.phase === 'checking'
      ? 'Checking whether this update is still available…'
      : update.phase === 'available'
        ? `Version ${version} is available.`
        : `Version ${version} is being prepared.`;

  return <dialog
    ref={dialog}
    className="update-dialog"
    aria-labelledby="update-dialog-title"
    onClose={() => setVisible(false)}
    onCancel={event => { event.preventDefault(); dismissOrHide(); }}
    onClick={event => { if (event.target === dialog.current) dismissOrHide(); }}
  >
    <div className="update-dialog-content">
      <header className="update-dialog-header">
        <span className={`update-dialog-mark${failed ? ' is-error' : ''}`}>
          {failed ? <AlertCircle size={16} /> : active ? <LoaderCircle size={16} className="spin" /> : <Download size={16} />}
        </span>
        <div><h2 id="update-dialog-title">{title}</h2><p>{subtitle}</p></div>
      </header>

      {update.phase === 'available' && update.notes && <div className="update-dialog-notes" aria-label="Release notes">{update.notes}</div>}

      {downloading && <section className="update-dialog-progress" aria-label="Update download progress">
        <div className="update-dialog-progress-copy">
          <strong>{percent === null ? 'Downloading update…' : `${percent}%`}</strong>
          <span>{formatBytes(update.downloadedBytes)}{update.contentLength === null ? ' downloaded' : ` of ${formatBytes(update.contentLength)}`}</span>
        </div>
        <progress max={100} value={percent ?? undefined} aria-label="Download progress" />
        <small>The download continues if you return to work.</small>
      </section>}

      {update.phase === 'checking' && <p className="update-dialog-status" role="status">Checking the signed release information…</p>}
      {(update.phase === 'ready' || update.phase === 'installing') && <p className="update-dialog-status" role="status">{update.phase === 'ready' ? 'Download complete. Preparing installation…' : 'Installing the verified update…'}</p>}
      {update.phase === 'restarting' && <p className="update-dialog-status" role="status">JevCode is restarting with the new version.</p>}
      {update.phase === 'completed' && <p className="update-dialog-status" role="status"><Check size={14} /> Update installed successfully.</p>}
      {restartRequired && <p className="update-dialog-error" role="alert">The update is installed. Restart JevCode to finish.</p>}
      {failed && <p className="update-dialog-error" role="alert">{update.error ?? 'Update installation failed. Try again later.'}</p>}

      <footer className="update-dialog-actions">
        {update.phase === 'available' && <>
          <button className="account-button quiet" onClick={dismissOrHide}>Later</button>
          <button className="account-button connect" onClick={() => void updateService.installAvailableUpdate()}><Download size={14} />Update Now</button>
        </>}
        {downloading && <button className="account-button quiet" onClick={() => setVisible(false)}>Keep working</button>}
        {restartRequired && <>
          <button className="account-button quiet" onClick={dismissOrHide}>Later</button>
          <button className="account-button connect" onClick={() => void updateService.restartApplication()}><RotateCw size={14} />Restart now</button>
        </>}
        {failed && <>
          <button className="account-button quiet" onClick={dismissOrHide}>Later</button>
          <button className="account-button connect" onClick={() => void updateService.checkForUpdate()}><RotateCw size={14} />Try again</button>
        </>}
        {update.phase === 'completed' && <button className="account-button quiet" onClick={dismissOrHide}>Done</button>}
      </footer>
    </div>
  </dialog>;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return `${value.toFixed(1)} ${units[unit]}`;
}
