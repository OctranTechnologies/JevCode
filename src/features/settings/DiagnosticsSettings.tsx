import { useCallback, useEffect, useState } from 'react';
import { Activity, Check, CircleAlert, Clipboard, FolderOpen, LoaderCircle, RefreshCw, ShieldCheck, Wifi, WifiOff } from 'lucide-react';
import type { AppDiagnostics } from '../../types/domain';
import { command } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';

export function DiagnosticsSettings({ online, onError }: { online: boolean; onError: (message: string | null) => void }) {
  const [details, setDetails] = useState<AppDiagnostics | null>(null);
  const [busy, setBusy] = useState<'diagnostics' | 'logs' | 'copy' | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setBusy('diagnostics');
    onError(null);
    try { setDetails(await command('get_app_diagnostics', undefined)); }
    catch (error) { onError(normalizeError(error).message); }
    finally { setBusy(current => current === 'diagnostics' ? null : current); }
  }, [onError]);

  useEffect(() => { void refresh(); }, [refresh]);
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

  return <section className="settings-content diagnostics-settings">
    <div className="settings-heading-row">
      <div><h1>Diagnostics</h1><p className="page-description">Local health, recovery, logs, and connectivity.</p></div>
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
    </> : <div className="diagnostic-load-state" role="status">{busy === 'diagnostics' ? <><LoaderCircle size={16} className="spin" />Loading local diagnostics…</> : <><CircleAlert size={16} />Local diagnostics are unavailable. Refresh to retry.</>}</div>}
  </section>;
}

function DiagnosticValue({ label, value }: { label: string; value: string }) {
  return <div className="diagnostic-value"><dt>{label}</dt><dd title={value}>{value}</dd></div>;
}
