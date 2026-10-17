import { useCallback, useEffect, useState } from 'react';
import { Activity, Check, CircleAlert, Clipboard, Download, ExternalLink, FolderOpen, LoaderCircle, RefreshCw, ShieldCheck, Wifi, WifiOff } from 'lucide-react';
import type { AppDiagnostics } from '../../types/domain';
import { command } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import { updaterService, type UpdateSnapshot } from '../../services/updater';

export function DiagnosticsSettings({ online, onError }: { online: boolean; onError: (message: string | null) => void }) {
  const [details, setDetails] = useState<AppDiagnostics | null>(null);
  const [update, setUpdate] = useState<UpdateSnapshot>(() => updaterService.getSnapshot());
  const [busy, setBusy] = useState<'diagnostics' | 'update' | 'logs' | 'copy' | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setBusy('diagnostics');
    onError(null);
    try { setDetails(await command('get_app_diagnostics', undefined)); }
    catch (error) { onError(normalizeError(error).message); }
    finally { setBusy(current => current === 'diagnostics' ? null : current); }
  }, [onError]);

  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => updaterService.subscribe(setUpdate), []);

  async function checkUpdates() {
    setBusy('update');
    setNotice(null);
    onError(null);
    try {
      const result = await updaterService.check();
      if (result.phase === 'available') setNotice(`JevCode ${result.availableVersion} is ready to install.`);
      else if (result.phase === 'up_to_date') setNotice(`JevCode ${result.currentVersion ? `v${result.currentVersion}` : ''} is up to date.`);
      else if (result.error) setNotice(result.error);
    } finally { setBusy(null); }
  }

  async function installUpdate() {
    setBusy('update');
    setNotice(null);
    onError(null);
    try { await updaterService.installAvailableUpdate(); }
    finally { setBusy(null); }
  }

  async function revealLogs() {
    setBusy('logs');
    onError(null);
    try { await command('reveal_application_logs', undefined); }
    catch (error) { onError(normalizeError(error).message); }
    finally { setBusy(null); }
  }

  async function copyDiagnostics() {
    if (!details) return;
    setBusy('copy');
    setNotice(null);
    const summary = [
      `JevCode ${details.appVersion}`,
      `Platform: ${details.operatingSystem} ${details.architecture}`,
      `Projects: ${details.projectCount} · Tasks: ${details.taskCount}`,
      `Connected providers: ${details.connectedProviderCount} · Needs attention: ${details.providerAttentionCount}`,
      `Database directory: ${details.dataDirectory}`,
      `Logs directory: ${details.logsDirectory}`,
    ].join('\n');
    try {
      await navigator.clipboard.writeText(summary);
      setNotice('Local diagnostics copied. Review the paths before sharing.');
    } catch { onError('Clipboard access was unavailable. You can still copy the diagnostic values below.'); }
    finally { setBusy(null); }
  }

  async function openRelease() {
    setBusy('update');
    onError(null);
    try { await command('open_latest_release', undefined); }
    catch (error) { onError(normalizeError(error).message); }
    finally { setBusy(null); }
  }

  return <section className="settings-content diagnostics-settings">
    <div className="settings-heading-row">
      <div><h1>Diagnostics</h1><p className="page-description">Local health, recovery, logs, and app updates.</p></div>
      <button className="account-button quiet" onClick={() => void refresh()} disabled={busy !== null} aria-label="Refresh diagnostics">{busy === 'diagnostics' ? <LoaderCircle size={14} className="spin" /> : <RefreshCw size={14} />}Refresh</button>
    </div>

    <div className={`diagnostic-connectivity ${online ? 'is-online' : 'is-offline'}`} role="status">
      {online ? <Wifi size={16} /> : <WifiOff size={16} />}
      <div><strong>{online ? 'Network connection available' : 'You are offline'}</strong><p>{online ? 'Projects and saved tasks remain local. Provider outages and authentication issues are shown on Accounts.' : 'Local projects, history, review, and settings remain available. Provider requests and update checks need a connection.'}</p></div>
    </div>

    {notice && <p className="diagnostic-feedback" role="status"><Check size={14} />{notice}</p>}
    {details ? <>
      <section className="diagnostic-section" aria-labelledby="diagnostic-health-title">
        <div className="diagnostic-section-heading"><div><h2 id="diagnostic-health-title">Application health</h2><p>Recovered interrupted tasks are saved as failed and can be reopened or retried.</p></div><Activity size={16} /></div>
        <dl className="diagnostic-values">
          <DiagnosticValue label="Version" value={details.appVersion} />
          <DiagnosticValue label="Platform" value={`${details.operatingSystem} · ${details.architecture}`} />
          <DiagnosticValue label="Projects" value={details.projectCount.toLocaleString()} />
          <DiagnosticValue label="Saved tasks" value={details.taskCount.toLocaleString()} />
          <DiagnosticValue label="Connected providers" value={`${details.connectedProviderCount} connected${details.providerAttentionCount ? ` · ${details.providerAttentionCount} need attention` : ''}`} />
        </dl>
      </section>

      <section className="diagnostic-section" aria-labelledby="diagnostic-logs-title">
        <div className="diagnostic-section-heading"><div><h2 id="diagnostic-logs-title">Application logs</h2><p>Logs contain event names and service errors, never prompts, API keys, or tool output.</p></div><ShieldCheck size={16} /></div>
        <dl className="diagnostic-paths"><DiagnosticValue label="Logs folder" value={details.logsDirectory} /><DiagnosticValue label="Local data folder" value={details.dataDirectory} /></dl>
        <div className="diagnostic-actions"><button className="account-button quiet" onClick={() => void revealLogs()} disabled={busy !== null}>{busy === 'logs' ? <LoaderCircle size={14} className="spin" /> : <FolderOpen size={14} />}Open logs folder</button><button className="account-button quiet" onClick={() => void copyDiagnostics()} disabled={busy !== null}>{busy === 'copy' ? <LoaderCircle size={14} className="spin" /> : <Clipboard size={14} />}Copy local summary</button></div>
      </section>

      <section className="diagnostic-section" aria-labelledby="diagnostic-updates-title">
        <div className="diagnostic-section-heading"><div><h2 id="diagnostic-updates-title">Updates</h2><p>JevCode checks for signed releases automatically and lets you check or install one here.</p></div><RefreshCw size={16} /></div>
        {update.phase === 'available' && <div className="diagnostic-update-notes"><strong>JevCode {update.availableVersion}</strong><time>{update.publishedAt ? new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(new Date(update.publishedAt)) : ''}</time>{update.notes && <p>{update.notes}</p>}</div>}
        {update.phase === 'checking' && <p className="diagnostic-help" role="status"><LoaderCircle size={13} className="spin" />Checking the signed release manifest…</p>}
        {update.phase === 'up_to_date' && <p className="diagnostic-help" role="status">{update.currentVersion ? `JevCode v${update.currentVersion} is up to date.` : 'JevCode is up to date.'}</p>}
        {update.phase === 'downloading' && <div className="diagnostic-download" role="status"><div><LoaderCircle size={13} className="spin" />Downloading update{update.contentLength ? ` · ${formatBytes(update.downloadedBytes)} of ${formatBytes(update.contentLength)}` : ` · ${formatBytes(update.downloadedBytes)}`}</div>{update.contentLength !== null && <progress max={update.contentLength} value={Math.min(update.downloadedBytes, update.contentLength)} aria-label="Update download progress" />}</div>}
        {update.phase === 'installing' && <p className="diagnostic-help" role="status"><LoaderCircle size={13} className="spin" />Installing update…</p>}
        {update.phase === 'restarting' && <p className="diagnostic-help" role="status">Update installed. Restarting JevCode…</p>}
        {update.phase === 'restart_required' && <p className="diagnostic-update-error" role="alert">Update installed. Restart JevCode to finish applying it.</p>}
        {update.phase === 'error' && <p className="diagnostic-update-error" role="alert">{update.error}</p>}
        <div className="diagnostic-actions"><button className="account-button connect" onClick={() => void checkUpdates()} disabled={!online || busy !== null || !updaterService.isSupported() || ['checking', 'downloading', 'installing', 'restarting'].includes(update.phase)}>{update.phase === 'checking' ? <LoaderCircle size={14} className="spin" /> : <RefreshCw size={14} />}Check for updates</button>{update.phase === 'available' && <><button className="account-button connect" onClick={() => void installUpdate()} disabled={!online || busy !== null}><Download size={14} />Download and install</button><button className="account-button quiet" onClick={() => void openRelease()} disabled={busy !== null}><ExternalLink size={14} />Release page</button></>}</div>
        {!updaterService.isSupported() && <small className="diagnostic-help">Update checks run in installed release builds. Development builds do not contact the update service.</small>}
        {updaterService.isSupported() && !online && <small className="diagnostic-help">Reconnect to check or download releases.</small>}
      </section>
    </> : <div className="diagnostic-load-state" role="status">{busy === 'diagnostics' ? <><LoaderCircle size={16} className="spin" />Loading local diagnostics…</> : <><CircleAlert size={16} />Local diagnostics are unavailable. Refresh to retry.</>}</div>}
  </section>;
}

function DiagnosticValue({ label, value }: { label: string; value: string }) {
  return <div className="diagnostic-value"><dt>{label}</dt><dd title={value}>{value}</dd></div>;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return `${value.toFixed(1)} ${units[unit]}`;
}
