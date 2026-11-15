import { ArrowUp, ChevronDown, FilePlus2, Paperclip, ShieldCheck, SlidersHorizontal, Square, X } from 'lucide-react';
import type { ReactNode } from 'react';
import type { ModelPreferences, ModelReference, Provider, WorkspaceMode } from '../../types/domain';
import { ModelPicker } from '../models/ModelPicker';
import { DisclosureMenu } from '../../components/DisclosureMenu';

export function WorkspaceComposer({
  draft, setDraft, providers, modelPreferences, providerId, modelId, onModel, onToggleFavorite, onSetDefault, modelLocked, mode, onMode,
  workspaceMode, baseBranch, branches, canIsolate, modelPickerOpenRequest,
  onSubmit, onStop, onAttach, attachments, onRemoveAttachment, busy, sending, hasProject,
  sessionLocked, desktopAllowed, providerConnected, authRouteLabel, includeProject, setIncludeProject, approval, lastPrompt, dropActive = false,
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
  modelPickerOpenRequest: number;
  modelLocked: boolean;
  mode: 'agent' | 'plan';
  onMode: (mode: 'agent' | 'plan') => void;
  workspaceMode: WorkspaceMode;
  baseBranch: string;
  branches: string[];
  canIsolate: boolean;
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
  authRouteLabel?: string | null;
  includeProject: boolean;
  setIncludeProject: (include: boolean) => void;
  approval?: ReactNode;
  lastPrompt: string;
  dropActive?: boolean;
}) {
  const provider = providers.find(item => item.id === providerId);
  const workspaceUnavailable = workspaceMode === 'isolated' && (!canIsolate || !baseBranch || !branches.includes(baseBranch));
  const unavailable = !desktopAllowed || !hasProject || !providerConnected || workspaceUnavailable;
  return <section className={`composer-dock${approval ? ' has-approval' : ''}${dropActive ? ' is-drop-target' : ''}`} aria-label="Task composer">
    {mode === 'plan' && <p role="status" className="composer-footnote">Plan mode is read-only. Edits, commands, and MCP execution are blocked. Start a new Agent task to implement this plan.</p>}
    {approval}
    <form className="workspace-composer" onSubmit={event => { event.preventDefault(); if (!unavailable && !busy && !sending && draft.trim()) onSubmit(); }}>
      {attachments.length > 0 && <div className="attached-files" aria-label="Files selected as task context">{attachments.map(path => <span className="attached-file" key={path}><FilePlus2 size={12} /><span>{path}</span><button type="button" onClick={() => onRemoveAttachment(path)} aria-label={`Remove ${path}`}><X size={12} /></button></span>)}</div>}
      <label className="sr-only" htmlFor="workspace-prompt">Describe a task for JevCode</label>
      <textarea id="workspace-prompt" value={draft} onChange={event => setDraft(event.target.value)} placeholder={!hasProject ? 'Open a project to start a task…' : busy ? 'JevCode is working. Draft your next message here…' : 'Ask JevCode to explore, explain, or change something…'} rows={3} onKeyDown={event => { if (event.key === 'ArrowUp' && !draft && lastPrompt) { event.preventDefault(); setDraft(lastPrompt); } else if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (!unavailable && !busy && !sending && draft.trim()) onSubmit(); } }} />
      <div className="composer-toolbar">
        <div className="composer-tool-actions">
          <button type="button" className="composer-tool-button" onClick={onAttach} disabled={!desktopAllowed || !hasProject || busy} title={desktopAllowed ? 'Attach files from this project' : 'Use the desktop app to attach local files'}><Paperclip size={14} /><span>Attach</span></button>
          <DisclosureMenu className="composer-options-menu" summary={<><SlidersHorizontal size={14} /><span>Options</span>{mode === 'plan' && <span className="task-option-badge">Plan · read-only</span>}{!includeProject && <span className="task-option-badge">No project context</span>}<ChevronDown size={12} /></>}>
            <div className="composer-options-popover">
              <label className="composer-option-row"><span><strong>Task approach</strong><small>Plan tasks stay read-only. Implementation requires a new Agent task.</small></span><select value={mode} disabled={sessionLocked || sending || busy} onChange={event => onMode(event.target.value as 'agent' | 'plan')} aria-label="Task approach"><option value="agent">Agent</option><option value="plan">Plan · read-only</option></select></label>
              <label className="composer-option-row composer-context-toggle"><span><strong>Project context</strong><small>Allow this task to inspect project files.</small></span><input type="checkbox" checked={includeProject} disabled={sessionLocked} onChange={event => setIncludeProject(event.target.checked)} /></label>
            </div>
          </DisclosureMenu>
        </div>
        <div className="composer-controls-right">
          <ModelPicker providers={providers} preferences={modelPreferences} providerId={providerId} modelId={modelId} locked={modelLocked || sending} compact openRequest={modelPickerOpenRequest} onSelect={onModel} onToggleFavorite={onToggleFavorite} onSetDefault={onSetDefault} />
          {busy ? <button type="button" className="composer-stop-button swap-in" onClick={onStop} aria-label="Stop agent task"><Square size={13} fill="currentColor" /><span>Stop</span></button> : <button type="submit" className="composer-send-button swap-in"disabled={unavailable || sending || !draft.trim()} title={!desktopAllowed ? 'Open JevCode desktop to send a task' : undefined} aria-label="Send task"><span>{sending ? 'Sending' : 'Send'}</span><ArrowUp size={14} /></button>}
        </div>
      </div>
    </form>
    <div className="composer-footnote"><span className="privacy-dot" />{dropActive ? 'Drop files to attach them to this task' : workspaceMode === 'isolated' && workspaceUnavailable ? 'Choose a Git repository and a local starting branch for this isolated task' : !desktopAllowed ? 'Sample workspace · tasks and activity are illustrative' : !hasProject ? 'Choose a project to enable tasks' : !providerConnected ? 'Connect a provider in Settings to start a task' : provider?.protocol === 'preview' ? 'Local preview · project files stay on this device' : `Connected to ${provider?.name} · project files stay on this device`}{authRouteLabel && <span className="auth-route-indicator"><ShieldCheck size={10} />{authRouteLabel}</span>}</div>
  </section>;
}
