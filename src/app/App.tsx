import { useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { AlertCircle, ChevronDown, ChevronLeft, Folder, LockKeyhole, ShieldCheck, X } from 'lucide-react';
import { useDesktop } from './useDesktop';
import { command, desktopAvailable } from '../lib/ipc';
import { normalizeError } from '../lib/errors';
import type { AgentSession } from '../types/domain';
import { Sidebar, type View } from '../features/workspaces/Sidebar';
import { Conversation } from '../features/agent/Conversation';
import { Composer } from '../features/agent/Composer';
import { ProviderSettings } from '../features/providers/ProviderSettings';
import { UsageView } from '../features/usage/UsageView';

export default function App() {
  const desktop = useDesktop();
  const [view, setView] = useState<View>('agent');
  const [projectChoice, setProjectChoice] = useState('');
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [providerChoice, setProviderChoice] = useState('preview');
  const [modelChoice, setModelChoice] = useState('');
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [opening, setOpening] = useState(false);
  const [permissionBusy, setPermissionBusy] = useState(false);
  const [askReads, setAskReads] = useState(false);
  const data = desktop.data;
  const projects = data?.workspace.projects ?? [];
  const projectId = projectChoice || projects[0]?.id || '';
  const project = projects.find(project => project.id === projectId);
  const session = desktop.sessions.find(session => session.id === sessionId);
  const providerId = session?.providerId ?? providerChoice;
  const provider = data?.providers.find(provider => provider.id === providerId);
  const modelId = session?.modelId ?? (modelChoice || provider?.models[0]?.id || '');
  const busy = session?.status === 'running' || session?.status === 'awaiting_permission';
  const tokens = desktop.usage.reduce((sum, record) => sum + record.inputTokens + record.outputTokens, 0);
  function newSession() { setSessionId(null); setDraft(''); setView('agent'); desktop.setError(null); }
  function selectSession(session: AgentSession) { setProjectChoice(session.projectId); setSessionId(session.id); setDraft(''); setView('agent'); desktop.setError(null); }
  async function openFolder() {
    setOpening(true);
    try {
      if (!desktopAvailable) { await command('open_project', { path: '' }); return; }
      const path = await open({ directory: true, multiple: false, title: 'Open a project folder' });
      if (typeof path === 'string') { const project = await command('open_project', { path }); desktop.dispatch({ type: 'project', project }); setProjectChoice(project.id); newSession(); }
    } catch (error) { desktop.setError(normalizeError(error).message); }
    finally { setOpening(false); }
  }
  async function send() {
    if (!data || !projectId || !draft.trim() || busy || sending) return;
    setSending(true); desktop.setError(null);
    try {
      let active = session;
      if (!active) {
        active = await command('create_session', { input: { projectId, providerId, modelId, permissionPolicy: { ...data.permissionPolicy, readFiles: askReads ? 'ask' : 'allow' } } });
        desktop.dispatch({ type: 'session', session: active }); setSessionId(active.id);
      }
      const updated = await command('send_message', { sessionId: active.id, content: draft });
      desktop.dispatch({ type: 'session', session: updated }); setDraft('');
    } catch (error) { desktop.setError(normalizeError(error).message); }
    finally { setSending(false); }
  }
  async function resolve(approved: boolean) {
    if (!session?.pendingToolCall || permissionBusy) return;
    setPermissionBusy(true);
    try { const updated = await command('resolve_permission', { sessionId: session.id, toolCallId: session.pendingToolCall.id, approved }); desktop.dispatch({ type: 'session', session: updated }); }
    catch (error) { desktop.setError(normalizeError(error).message); }
    finally { setPermissionBusy(false); }
  }
  async function stop() { if (session) { try { await command('cancel_session', { sessionId: session.id }); } catch (error) { desktop.setError(normalizeError(error).message); } } }
  if (desktop.loading) return <main className="fatal-state" role="status"><span className="status-dot pulse" /><p>Opening your workspace…</p></main>;
  if (!data) return <main className="fatal-state"><AlertCircle size={32} /><h1>Could not open JevCode</h1><p>{desktop.error}</p><button className="primary-button" onClick={desktop.retry}>Retry</button></main>;
  return <div className="desktop-shell"><Sidebar projects={projects} projectId={projectId} sessions={desktop.sessions} sessionId={sessionId} view={view} onProject={id => { setProjectChoice(id); newSession(); }} onSession={selectSession} onView={setView} onNew={newSession} onOpen={() => void openFolder()} opening={opening} />
    <main className="main-panel"><header className="toolbar"><div className="breadcrumb">{view === 'agent' ? <><Folder size={19} /><strong>{project?.name ?? 'No project selected'}</strong><span>/</span><span className="session-title">{session?.title ?? 'New session'}</span></> : <><button className="icon-button" onClick={() => setView('agent')} aria-label="Back to conversation"><ChevronLeft size={20} /></button><strong>{view === 'providers' ? 'Providers' : 'Usage'}</strong></>}</div><details className="policy-menu"><summary><LockKeyhole size={15} />Read-only<ChevronDown size={14} /></summary><div><h2>Session permissions</h2><p>Write and shell tools are disabled.<br />Git status asks for approval.</p><label><input type="checkbox" checked={session ? session.permissionPolicy.readFiles === 'ask' : askReads} disabled={!!session} onChange={event => setAskReads(event.target.checked)} />Ask before reading files</label>{session && <p>Start a new session to change its policy.</p>}</div></details></header>
      {desktop.error && <div className="error-banner" role="alert"><AlertCircle size={16} /><span>{desktop.error}</span><button className="icon-button" onClick={() => desktop.setError(null)} aria-label="Dismiss error"><X size={16} /></button></div>}
      {view === 'agent' ? <><div className="conversation-scroll"><Conversation session={session} onSuggestion={prompt => setDraft(prompt)} onPermission={approved => void resolve(approved)} permissionBusy={permissionBusy} /></div><Composer draft={draft} setDraft={setDraft} providers={data.providers} providerId={providerId} modelId={modelId} onProvider={id => { setProviderChoice(id); setModelChoice(''); }} onModel={setModelChoice} onSubmit={() => void send()} onStop={() => void stop()} busy={busy} sending={sending} hasProject={!!project} sessionLocked={!!session} /></> : <div className="settings-scroll">{view === 'providers' ? <ProviderSettings providers={data.providers} onUpdate={providers => desktop.dispatch({ type: 'providers', providers })} /> : <UsageView records={desktop.usage} providers={data.providers} />}</div>}
      <footer className="statusbar"><span><span className={`status-dot ${busy ? 'pulse' : ''}`} />{session?.status === 'awaiting_permission' ? 'Awaiting permission' : busy ? 'Working' : 'Ready'}</span><span className="status-security"><ShieldCheck size={12} />Local data · OS keychain</span><span className="token-count">{tokens.toLocaleString()} tokens</span></footer>
    </main>
  </div>;
}
