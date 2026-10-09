import { ArrowUp, AtSign, ChevronDown, FilePlus2, Paperclip, ShieldCheck, Sparkles, Square, X } from 'lucide-react';
import type { ModelPreferences, ModelReference, Provider } from '../../types/domain';
import { ModelPicker } from '../models/ModelPicker';

export function WorkspaceComposer({
  draft, setDraft, providers, modelPreferences, providerId, modelId, onModel, onToggleFavorite, onSetDefault, modelLocked, mode, onMode,
  onSubmit, onStop, onAttach, attachments, onRemoveAttachment, busy, sending, hasProject,
  sessionLocked, desktopAllowed, providerConnected, contextOpen, setContextOpen, includeProject, setIncludeProject,
}: {
  draft: string;
  setDraft: (value: string) => void;
  providers: Provider[];
  modelPreferences: ModelPreferences;
  providerId: string;
  modelId: string;
  onModel: (reference: ModelReference) => void;
  onToggleFavorite: (reference: ModelReference) => void;
  onSetDefault: (reference: ModelReference) => void;
  modelLocked: boolean;
  mode: 'agent' | 'plan';
  onMode: (mode: 'agent' | 'plan') => void;
  onSubmit: () => void;
  onStop: () => void;
  onAttach: () => void;
  attachments: string[];
  onRemoveAttachment: (path: string) => void;
  busy: boolean;
  sending: boolean;
  hasProject: boolean;
  sessionLocked: boolean;
  desktopAllowed: boolean;
  providerConnected: boolean;
  contextOpen: boolean;
  setContextOpen: (open: boolean) => void;
  includeProject: boolean;
  setIncludeProject: (include: boolean) => void;
}) {
  const provider = providers.find(item => item.id === providerId);
  const unavailable = !desktopAllowed || !hasProject || !providerConnected;
  return <section className="composer-dock" aria-label="Task composer">
    {contextOpen && <button className="popover-dismiss-area" aria-label="Close context controls" onClick={() => setContextOpen(false)} />}
    <form className="workspace-composer" onSubmit={event => { event.preventDefault(); if (!unavailable && !busy && !sending && draft.trim()) onSubmit(); }}>
      {attachments.length > 0 && <div className="attached-files" aria-label="Files selected as task context">{attachments.map(path => <span className="attached-file" key={path}><FilePlus2 size={12} /><span>{path}</span><button type="button" onClick={() => onRemoveAttachment(path)} aria-label={`Remove ${path}`}><X size={12} /></button></span>)}</div>}
      <label className="sr-only" htmlFor="workspace-prompt">Describe a task for JevCode</label>
      <textarea id="workspace-prompt" value={draft} onChange={event => setDraft(event.target.value)} placeholder={hasProject ? 'Ask JevCode to explore, explain, or change something…' : 'Open a project to start a task…'} rows={3} disabled={busy || sending} onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (!unavailable && !busy && !sending && draft.trim()) onSubmit(); } }} />
      <div className="composer-toolbar">
        <div className="composer-tool-actions">
          <button type="button" className="composer-tool-button" onClick={onAttach} disabled={!desktopAllowed || !hasProject || busy} title={desktopAllowed ? 'Attach files from this project' : 'Use the desktop app to attach local files'}><Paperclip size={14} /><span>Attach</span></button>
          <div className="context-control-wrap">
            <button type="button" className={`composer-tool-button${contextOpen ? ' is-active' : ''}`} aria-expanded={contextOpen} onClick={() => setContextOpen(!contextOpen)}><AtSign size={14} /><span>Context</span><ChevronDown size={12} /></button>
            {contextOpen && <div className="context-popover" role="group" aria-label="Task context controls">
              <div className="context-popover-heading"><strong>Task context</strong><span>Choose what this task can use</span></div>
              <label className="context-option"><input type="checkbox" checked={includeProject} disabled={sessionLocked} onChange={event => setIncludeProject(event.target.checked)} /><span><strong>Project files</strong><small>{sessionLocked ? 'Permission is fixed for this task' : 'Allow read-only access to the selected project'}</small></span><ShieldCheck size={14} /></label>
              <div className="context-disabled"><span className="context-square" /><span><strong>Terminal output</strong><small>Shell tools are disabled in this foundation</small></span></div>
            </div>}
          </div>
          <span className="context-scope">{includeProject && hasProject ? 'Project context on' : 'No project context'}</span>
        </div>
        <div className="composer-controls-right">
          <label className="sr-only" htmlFor="task-mode">Task mode</label>
          <span className="composer-select-wrap mode-select-wrap"><span className="mode-select-icon"><SparkleMode /></span><select id="task-mode" value={mode} onChange={event => onMode(event.target.value as 'agent' | 'plan')} aria-label="Task mode"><option value="agent">Agent</option><option value="plan">Plan first</option></select><ChevronDown size={12} /></span>
          <ModelPicker providers={providers} preferences={modelPreferences} providerId={providerId} modelId={modelId} locked={modelLocked || sending} compact onSelect={onModel} onToggleFavorite={onToggleFavorite} onSetDefault={onSetDefault} />
          {busy ? <button type="button" className="composer-stop-button" onClick={onStop} aria-label="Stop agent task"><Square size={13} fill="currentColor" /><span>Stop</span></button> : <button type="submit" className="composer-send-button" disabled={unavailable || sending || !draft.trim()} title={!desktopAllowed ? 'Open JevCode desktop to send a task' : undefined} aria-label="Send task"><span>{sending ? 'Sending' : 'Send'}</span><ArrowUp size={14} /></button>}
        </div>
      </div>
    </form>
    <div className="composer-footnote"><span className="privacy-dot" />{!desktopAllowed ? 'Sample workspace · tasks and activity are illustrative' : !hasProject ? 'Choose a project to enable tasks' : !providerConnected ? 'Connect a provider in Settings to start a task' : provider?.protocol === 'preview' ? 'Local preview · project files stay on this device' : `Connected to ${provider?.name} · project files stay on this device`}</div>
  </section>;
}

function SparkleMode() {
  return <Sparkles size={13} />;
}
