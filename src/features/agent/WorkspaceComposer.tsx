import { ArrowUp, ChevronDown, FilePlus2, GitBranch, ListPlus, Paperclip, Play, Search, ShieldCheck, SlidersHorizontal, Sparkles, Square, X } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import type { HarnessStatus, ModelPreferences, ModelReference, Provider, QueuedFollowUp, SkillSummary, WorkspaceMode } from '../../types/domain';
import { ModelPicker } from '../models/ModelPicker';
import { DisclosureMenu } from '../../components/DisclosureMenu';
import { filterSkills } from '../../lib/skills';
import './childTask.css';
import './harnesses.css';

export function WorkspaceComposer({
  draft, setDraft, providers, modelPreferences, providerId, modelId, onModel, onToggleFavorite, onSetDefault, modelLocked, mode, onMode,
  workspaceMode, baseBranch, branches, canIsolate, modelPickerOpenRequest,
  onSubmit, onQueueFollowUp, canQueueFollowUp, queuedFollowUps, onRemoveQueuedFollowUp, onStop, onAttach, attachments, onRemoveAttachment, busy, sending, hasProject,
  sessionLocked, desktopAllowed, providerConnected, authRouteLabel, includeProject, setIncludeProject, approval, lastPrompt, dropActive = false, skills, selectedSkills, onSkillSelect, onSkillRemove,
  delegationParentTitle, canDelegate = false, onDelegate, onCancelDelegation,
  harnesses, harnessId, setHarnessId, harnessModel, setHarnessModel, canRunHarness, onRunHarness,
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
  onQueueFollowUp: (content: string) => void;
  canQueueFollowUp: boolean;
  queuedFollowUps: QueuedFollowUp[];
  onRemoveQueuedFollowUp: (id: string) => void;
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
  skills: SkillSummary[];
  selectedSkills: string[];
  onSkillSelect: (id: string) => void;
  onSkillRemove: (id: string) => void;
  delegationParentTitle?: string | null;
  canDelegate?: boolean;
  onDelegate?: () => void;
  onCancelDelegation?: () => void;
  harnesses: HarnessStatus[];
  harnessId: string;
  setHarnessId: (id: string) => void;
  harnessModel: string;
  setHarnessModel: (model: string) => void;
  canRunHarness: boolean;
  onRunHarness: () => void;
}) {
  const provider = providers.find(item => item.id === providerId);
  const workspaceUnavailable = workspaceMode === 'isolated' && (!canIsolate || !baseBranch || !branches.includes(baseBranch));
  const unavailable = !desktopAllowed || !hasProject || !providerConnected || workspaceUnavailable;
  const availableSkills = skills.filter(skill => skill.valid && !selectedSkills.includes(skill.id));
  const [skillQuery, setSkillQuery] = useState('');
  const searchableSkills = availableSkills.length > 5;
  const shownSkills = searchableSkills ? filterSkills(availableSkills, skillQuery) : availableSkills;
  const supportedHarnesses = harnesses.filter(item => item.taskExecutionSupported);
  const selectedHarness = harnesses.find(item => item.id === harnessId);
  return <section className={`composer-dock${approval ? ' has-approval' : ''}${dropActive ? ' is-drop-target' : ''}`} aria-label="Task composer">
    {mode === 'plan' && <p role="status" className="composer-footnote">Plan mode is read-only. Edits, commands, and MCP execution are blocked. Start a new Agent task to implement this plan.</p>}
    {delegationParentTitle && <div className="child-task-banner" role="status"><GitBranch size={14} /><span>New child task from <strong>{delegationParentTitle}</strong>. It will start on the parent branch in a separate worktree.</span><button type="button" onClick={onCancelDelegation}>Cancel</button></div>}
    {approval}
    {queuedFollowUps.length > 0 && <div className="queued-follow-ups" aria-label="Queued follow-ups">
      <div className="queued-follow-ups-heading"><strong>Queued follow-ups</strong><span>{queuedFollowUps.length}</span></div>
      <ol>{queuedFollowUps.map((item, index) => <li key={item.id}>
        <span className="queued-follow-up-index">{index + 1}</span>
        <span className="queued-follow-up-content">{item.content}</span>
        <button type="button" onClick={() => onRemoveQueuedFollowUp(item.id)} disabled={sending} aria-label={`Remove queued follow-up ${index + 1}`} title="Remove follow-up"><X size={13} /></button>
      </li>)}</ol>
    </div>}
    <form className="workspace-composer" onSubmit={event => { event.preventDefault(); if (busy) { if (canQueueFollowUp && !sending && draft.trim()) onQueueFollowUp(draft); } else if (!unavailable && !sending && draft.trim()) onSubmit(); }}>
      {selectedSkills.length > 0 && <div className="attached-files" aria-label="Skills for this task">{selectedSkills.map(id => { const skill = skills.find(item => item.id === id); const label = skill?.name ?? id; return <span className="attached-file" key={id}><Sparkles size={12} /><span>Skill: {label}</span><button type="button" onClick={() => onSkillRemove(id)} aria-label={`Remove skill ${label}`}><X size={12} /></button></span>; })}</div>}
      {attachments.length > 0 && <div className="attached-files" aria-label="Files selected as task context">{attachments.map(path => <span className="attached-file" key={path}><FilePlus2 size={12} /><span>{path}</span><button type="button" onClick={() => onRemoveAttachment(path)} aria-label={`Remove ${path}`}><X size={12} /></button></span>)}</div>}
      <label className="sr-only" htmlFor="workspace-prompt">Describe a task for Octran Code</label>
      <textarea id="workspace-prompt" value={draft} onChange={event => setDraft(event.target.value)} placeholder={!hasProject ? 'Open a project to start a task…' : busy ? 'Octran Code is working. Draft your next message here…' : 'Ask Octran Code to explore, explain, or change something…'} rows={3} onKeyDown={event => { if (event.key === 'ArrowUp' && !draft && lastPrompt) { event.preventDefault(); setDraft(lastPrompt); } else if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (busy) { if (canQueueFollowUp && !sending && draft.trim()) onQueueFollowUp(draft); } else if (!unavailable && !sending && draft.trim()) onSubmit(); } }} />
      <div className="composer-toolbar">
        <div className="composer-tool-actions">
          <button type="button" className="composer-tool-button" onClick={onAttach} disabled={!desktopAllowed || !hasProject || busy} title={desktopAllowed ? 'Attach files from this project' : 'Use the desktop app to attach local files'}><Paperclip size={14} /><span>Attach</span></button>
          {skills.some(skill => skill.valid) && <DisclosureMenu className="composer-skill-menu" closeOnChoice summary={<><Sparkles size={14} /><span>Skill</span><ChevronDown size={12} /></>} summaryTitle="Invoke a project or user skill for this task">
            <div className="composer-skill-popover" role="group" aria-label="Skills you can add to this task">
              {searchableSkills && <div className="composer-skill-search"><Search size={13} /><input type="search" value={skillQuery} onChange={event => setSkillQuery(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') event.preventDefault(); }} placeholder="Search skills" aria-label="Search skills" /></div>}
              {shownSkills.map(skill => <button type="button" className="composer-skill-choice" key={skill.id} disabled={busy} onClick={() => { setSkillQuery(''); onSkillSelect(skill.id); }}><span><strong>{skill.name}</strong><em>{skill.scope === 'project' ? 'Project' : 'User'}</em></span>{skill.description && <small>{skill.description}</small>}</button>)}
              {availableSkills.length === 0 && <p className="composer-skill-empty">Every available skill is already attached.</p>}
              {availableSkills.length > 0 && shownSkills.length === 0 && <p className="composer-skill-empty">No skills match “{skillQuery.trim()}”.</p>}
            </div>
          </DisclosureMenu>}
          <DisclosureMenu className="composer-options-menu" summary={<><SlidersHorizontal size={14} /><span>Options</span>{mode === 'plan' && <span className="task-option-badge">Plan · read-only</span>}{!includeProject && <span className="task-option-badge">No project context</span>}<ChevronDown size={12} /></>}>
            <div className="composer-options-popover">
              <label className="composer-option-row"><span><strong>Task approach</strong><small>Plan tasks stay read-only. Implementation requires a new Agent task.</small></span><select value={mode} disabled={sessionLocked || sending || busy} onChange={event => onMode(event.target.value as 'agent' | 'plan')} aria-label="Task approach"><option value="agent">Agent</option><option value="plan">Plan · read-only</option></select></label>
              <label className="composer-option-row composer-context-toggle"><span><strong>Project context</strong><small>Allow this task to inspect project files.</small></span><input type="checkbox" checked={includeProject} disabled={sessionLocked} onChange={event => setIncludeProject(event.target.checked)} /></label>
              {canDelegate && <button type="button" className="composer-delegate-choice" onClick={onDelegate}><GitBranch size={13} /><span>Create a child task from this branch</span></button>}
              {supportedHarnesses.length > 0 && <div className="composer-harness-options">
                <label className="composer-option-row"><span><strong>External coding tool</strong><small>Continue this isolated task with a local CLI.</small></span><select value={harnessId} disabled={busy || sending} onChange={event => setHarnessId(event.target.value)} aria-label="External coding tool">
                  <option value="">Choose a tool</option>
                  {supportedHarnesses.map(item => <option key={item.id} value={item.id} disabled={!item.installed}>{item.name}{item.installed ? '' : ' · not installed'}</option>)}
                </select></label>
                <label className="composer-harness-model"><span>Model override</span><input value={harnessModel} disabled={busy || sending || !selectedHarness?.installed} onChange={event => setHarnessModel(event.target.value)} placeholder="Use CLI default" maxLength={120} aria-label="External coding tool model override" /></label>
                <p className="composer-options-note">{selectedHarness?.taskPermissionSummary ?? 'Only supported Codex and Claude adapters can run tasks. Other detected tools are not launched.'}</p>
                {selectedHarness?.installed && <p className="composer-options-note">A task with read and write permissions is required; Codex also requires Full Access command and Git permissions.</p>}
              </div>}
            </div>
          </DisclosureMenu>
        </div>
        <div className="composer-controls-right">
          <ModelPicker providers={providers} preferences={modelPreferences} providerId={providerId} modelId={modelId} locked={modelLocked || sending} compact openRequest={modelPickerOpenRequest} onSelect={onModel} onToggleFavorite={onToggleFavorite} onSetDefault={onSetDefault} />
          {busy ? <>{canQueueFollowUp && <button type="button" className="composer-queue-button" onClick={() => onQueueFollowUp(draft)} disabled={!desktopAllowed || sending || !draft.trim()} title="Queue this follow-up to run after the current task"><ListPlus size={13} /><span>Queue</span></button>}<button type="button" className="composer-stop-button swap-in" onClick={onStop} aria-label="Stop agent task"><Square size={13} fill="currentColor" /><span>Stop</span></button></> : <>{canRunHarness && <button type="button" className="composer-harness-button swap-in" onClick={onRunHarness} disabled={sending || !draft.trim()} title={`Run this follow-up with ${selectedHarness?.name ?? 'the selected CLI'}`}><Play size={12} fill="currentColor" /><span>{sending ? 'Starting' : `Run ${selectedHarness?.name ?? 'CLI'}`}</span></button>}<button type="submit" className="composer-send-button swap-in" disabled={unavailable || sending || !draft.trim()} title={!desktopAllowed ? 'Open Octran Code desktop to send a task' : undefined} aria-label="Send task"><span>{sending ? 'Sending' : 'Send'}</span><ArrowUp size={14} /></button></>}
        </div>
      </div>
    </form>
    <div className="composer-footnote"><span className="privacy-dot" />{dropActive ? 'Drop files to attach them to this task' : workspaceMode === 'isolated' && workspaceUnavailable ? 'Choose a Git repository and a local starting branch for this isolated task' : !desktopAllowed ? 'Sample workspace · tasks and activity are illustrative' : !hasProject ? 'Choose a project to enable tasks' : !providerConnected ? 'Connect a provider in Settings to start a task' : provider?.protocol === 'preview' ? 'Local preview · project files stay on this device' : `Connected to ${provider?.name} · project files stay on this device`}{authRouteLabel && <span className="auth-route-indicator"><ShieldCheck size={10} />{authRouteLabel}</span>}</div>
  </section>;
}
