import {
  Check, ChevronDown, CircleHelp, Clock3, Copy, ExternalLink, FileCode2, FilePlus2, GitBranch,
  ListChecks, RotateCcw, Shield, Terminal, X, GitCommitHorizontal,
} from 'lucide-react';
import type { ReactNode } from 'react';
import type { AgentSession, Project, Provider, ReviewAllAction, ReviewFileAction, SessionChanges, SessionFileDiff, UsageRecord } from '../../types/domain';
import { demoChanges, demoDiff, demoTerminal } from '../../app/demo';

export type InspectorTab = 'changes' | 'diff' | 'terminal' | 'context' | 'task';

export function InspectorPanel({
  project, branch, session, provider, demo, open, onClose, tab, onTab, review, reviewDiff, selectedPath, reviewBusy, usage, onSelectFile, onFileAction, onAllAction,
}: {
  project?: Project;
  branch: string | null;
  session?: AgentSession;
  provider?: Provider;
  demo: boolean;
  open: boolean;
  onClose: () => void;
  tab: InspectorTab;
  onTab: (tab: InspectorTab) => void;
  review: SessionChanges | null;
  reviewDiff: SessionFileDiff | null;
  selectedPath: string | null;
  reviewBusy: boolean;
  usage: UsageRecord[];
  onSelectFile: (path: string) => void;
  onFileAction: (path: string, action: ReviewFileAction) => void;
  onAllAction: (action: ReviewAllAction) => void;
}) {
  const tabs: { id: InspectorTab; label: string; icon: ReactNode; count?: string }[] = [
    { id: 'changes', label: 'Files', icon: <FileCode2 size={13} />, count: demo ? '2' : review?.files.length ? String(review.files.length) : undefined },
    { id: 'diff', label: 'Diff', icon: <ListChecks size={13} /> },
    { id: 'terminal', label: 'Terminal', icon: <Terminal size={13} /> },
    { id: 'context', label: 'Context', icon: <CircleHelp size={13} /> },
    { id: 'task', label: 'Task', icon: <Clock3 size={13} /> },
  ];
  return <aside className={`workbench-inspector${open ? ' is-open' : ''}`} aria-label="Task details">
    <header className="inspector-heading"><span>Task details</span><button className="quiet-icon-button inspector-close" onClick={onClose} aria-label="Close task details"><X size={15} /></button></header>
    <div className="inspector-tabs" role="tablist" aria-label="Task detail panels">{tabs.map(item => <button key={item.id} id={`inspector-tab-${item.id}`} className={`inspector-tab${tab === item.id ? ' is-active' : ''}`} role="tab" aria-selected={tab === item.id} aria-controls="inspector-panel" onClick={() => onTab(item.id)} title={item.label}>{item.icon}<span>{item.label}</span>{item.count && <small>{item.count}</small>}</button>)}</div>
    <div className="inspector-panel" role="tabpanel" id="inspector-panel" aria-labelledby={`inspector-tab-${tab}`}>
      {demo && <div className="inspector-demo-note"><span className="demo-note-mark">i</span><span>Example data · not applied to files</span></div>}
      {tab === 'changes' && <ChangesPanel demo={demo} session={session} review={review} selectedPath={selectedPath} busy={reviewBusy} onSelect={path => { onSelectFile(path); onTab('diff'); }} onFileAction={onFileAction} onAllAction={onAllAction} />}
      {tab === 'diff' && <DiffPanel demo={demo} selectedFile={selectedPath} selectedChange={review?.files.find(file => file.path === selectedPath)} reviewDiff={reviewDiff} reviewBusy={reviewBusy} onFileAction={onFileAction} />}
      {tab === 'terminal' && <TerminalPanel demo={demo} />}
      {tab === 'context' && <ContextPanel project={project} branch={branch} session={session} provider={provider} />}
      {tab === 'task' && <TaskPanel session={session} demo={demo} project={project} provider={provider} review={review} usage={usage} />}
    </div>
  </aside>;
}

function ChangesPanel({ demo, session, review, selectedPath, busy, onSelect, onFileAction, onAllAction }: { demo: boolean; session?: AgentSession; review: SessionChanges | null; selectedPath: string | null; busy: boolean; onSelect: (path: string) => void; onFileAction: (path: string, action: ReviewFileAction) => void; onAllAction: (action: ReviewAllAction) => void }) {
  if (!demo && !review) return <div className="inspector-empty"><FileCode2 size={18} /><strong>Changes load with the task</strong><p>Start or select a task to see its reviewable files.</p></div>;
  const grouped = demo ? null : (['added', 'modified', 'deleted'] as const).map(kind => ({ kind, label: kind === 'added' ? 'Added' : kind === 'modified' ? 'Modified' : 'Deleted', files: review!.files.filter(file => file.kind === kind) })).filter(group => group.files.length);
  if (!demo && review?.files.length === 0) return <div className="review-empty"><span className="review-empty-mark"><Check size={15} /></span><strong>{session?.status === 'completed' ? 'No file changes' : 'No file changes yet'}</strong><p>{session?.status === 'completed' ? 'This task finished without any files left changed.' : 'File edits from this task will appear here with a reviewable diff.'}</p></div>;
  return <div className="changed-files-panel"><div className="inspector-section-label"><span>{demo ? '2 FILES' : `${review?.files.length ?? 0} FILES`}</span><button title="Collapse file list" aria-label="Collapse file list"><ChevronDown size={14} /></button></div>
    {demo ? <>{demoChanges.map(change => <button key={change.path} className="changed-file-row" onClick={() => onSelect(change.path)}><span className={`file-state-mark ${change.state}`} />
      <span className="changed-file-copy"><strong>{change.path.split('/').at(-1)}</strong><small>{change.path.split('/').slice(0, -1).join('/')}</small></span>
      <span className="change-counts"><span>+{change.additions}</span><span>−{change.deletions}</span></span>
    </button>)}</> : <>
      <div className="review-actions"><span>{review?.files.length} {review?.files.length === 1 ? 'file' : 'files'} · +{review?.additions} −{review?.deletions}</span><div><button disabled={busy || !review?.files.length} onClick={() => onAllAction('accept_all')} title="Keep all task changes"><Check size={12} />Accept all</button><button disabled={busy || !review?.files.length} onClick={() => onAllAction('revert_all')} title="Revert task changes, preserving the pre-task working tree"><RotateCcw size={12} />Revert all</button></div></div>
      {grouped?.map(group => <section key={group.kind} className="review-file-group"><h3>{group.label}<span>{group.files.length}</span></h3>{group.files.map(file => <div key={file.path} className={`review-file-row${selectedPath === file.path ? ' is-selected' : ''}`}>
        <button className="review-file-select" onClick={() => onSelect(file.path)} aria-label={`Review ${file.path}`}><span className={`file-state-mark ${file.kind === 'added' ? 'added' : file.kind === 'deleted' ? 'deleted' : 'modified'}`} /><span className="changed-file-copy"><strong>{file.path.split('/').at(-1)}</strong><small>{file.path.split('/').slice(0, -1).join('/') || '.'}</small><span className="review-file-flags">{file.preexistingStatus && <i>Pre-existing edits</i>}{file.staged && <i className="is-staged">Staged</i>}{file.reviewed && <i className="is-reviewed">Accepted</i>}{file.conflicted && <i className="is-conflicted">Changed since task</i>}</span></span><span className="change-counts"><span>+{file.additions}</span><span>−{file.deletions}</span></span></button>
        <div className="review-row-actions"><button title={file.reviewed ? 'Already accepted' : 'Accept file changes'} aria-label={`Accept ${file.path}`} disabled={busy || file.reviewed} onClick={() => onFileAction(file.path, 'accept')}><Check size={12} /></button><button title="Revert file to its pre-task state" aria-label={`Revert ${file.path}`} disabled={busy || file.conflicted} onClick={() => onFileAction(file.path, 'revert')}><RotateCcw size={12} /></button></div>
      </div>)}</section>)}
      <div className="changes-summary"><span>{review?.workingTree === 'clean' ? 'Working tree clean' : 'Working tree modified'}</span><span>{review?.baselineHead ? `from ${review.baselineHead.slice(0, 7)}` : 'Local workspace'}</span></div>
    </>}
  </div>;
}

function DiffPanel({ demo, selectedFile, selectedChange, reviewDiff, reviewBusy, onFileAction }: { demo: boolean; selectedFile: string | null; selectedChange?: SessionChanges['files'][number]; reviewDiff: SessionFileDiff | null; reviewBusy: boolean; onFileAction: (path: string, action: ReviewFileAction) => void }) {
  if (!demo && (!selectedFile || !reviewDiff)) return <div className="inspector-empty"><ListChecks size={18} /><strong>Select a changed file</strong><p>Choose a file from Files to inspect its task-scoped diff.</p></div>;
  if (!demo && reviewDiff) {
    const file = reviewDiff;
    const lines = file.diff.split('\n');
    return <div className="diff-view"><div className="diff-file-heading"><FileCode2 size={13} /><span title={file.path}>{file.path}</span><button title="Copy diff" aria-label="Copy diff" onClick={() => void navigator.clipboard?.writeText(file.diff)}><Copy size={13} /></button></div>
      {file.conflicted && <div className="diff-conflict-note"><CircleHelp size={13} />This file changed after the agent edit. Revert and stage are disabled until you inspect the current file.</div>}
      <div className="diff-code" role="region" aria-label={`Diff for ${file.path}`}>{file.binary ? <div className="diff-binary-note">Binary file changed; text diff is unavailable.</div> : lines.map((line, index) => <div key={`${index}-${line}`} className={`diff-text-line${line.startsWith('+') ? ' is-add' : line.startsWith('-') ? ' is-delete' : line.startsWith('@@') ? ' is-hunk' : ''}`}><span className="diff-gutter">{line.startsWith('+') ? '+' : line.startsWith('-') ? '−' : ' '}</span><code>{line || ' '}</code></div>)}</div>
      <div className="review-diff-actions"><button disabled={reviewBusy} onClick={() => onFileAction(file.path, 'open')}><ExternalLink size={13} />Open file</button><button disabled={reviewBusy || file.conflicted || !selectedChange?.canStage} title={selectedChange?.preexistingStatus ? 'Staging is disabled because this file had user changes before the task.' : 'Stage the complete file'} onClick={() => onFileAction(file.path, 'stage')}><GitCommitHorizontal size={13} />{selectedChange?.staged ? 'Staged' : 'Stage file'}</button><span /><button disabled={reviewBusy || selectedChange?.reviewed} onClick={() => onFileAction(file.path, 'accept')}><Check size={13} />{selectedChange?.reviewed ? 'Accepted' : 'Accept'}</button><button disabled={reviewBusy || file.conflicted} onClick={() => onFileAction(file.path, 'revert')}><RotateCcw size={13} />Revert</button></div>
    </div>;
  }
  return <div className="diff-view"><div className="diff-file-heading"><FileCode2 size={13} /><span>{selectedFile ?? demoChanges[0].path}</span><button title="Copy diff" aria-label="Copy diff" onClick={() => void navigator.clipboard?.writeText(demoDiff.map(line => line.text).join('\n'))}><Copy size={13} /></button></div>
    <div className="diff-code" role="region" aria-label={`Illustrative diff for ${selectedFile}`}>{demoDiff.map((line, index) => <div key={`${line.kind}-${index}`} className={`diff-line is-${line.kind}`}><span className="diff-number">{line.old}</span><span className="diff-number">{line.next}</span><span className="diff-sign">{line.kind === 'add' ? '+' : line.kind === 'delete' ? '−' : line.kind === 'hunk' ? '@' : ' '}</span><code>{line.text}</code></div>)}</div>
    <div className="diff-explainer">This patch is a visual example. It has not been applied.</div>
  </div>;
}

function TerminalPanel({ demo }: { demo: boolean }) {
  if (!demo) return <div className="inspector-empty"><Terminal size={18} /><strong>Agent commands run with approval</strong><p>When project command access is set to Ask or Allow, executed commands and bounded output appear in the task timeline.</p></div>;
  return <div className="terminal-panel"><div className="terminal-panel-heading"><span className="terminal-live-dot" /> SAMPLE OUTPUT <button title="Copy output" aria-label="Copy terminal output" onClick={() => void navigator.clipboard?.writeText(demoTerminal)}><Copy size={13} /></button></div><pre>{demoTerminal}</pre><div className="terminal-footnote">Example command output · no command was run</div></div>;
}

function ContextPanel({ project, branch, session, provider }: { project?: Project; branch: string | null; session?: AgentSession; provider?: Provider }) {
  return <div className="context-detail-panel"><p className="inspector-section-label">PROJECT</p>
    <ContextRow label="Project" value={project?.name ?? 'No project selected'} icon={<FileCode2 size={13} />} />
    <ContextRow label="Branch" value={branch ?? 'Not detected'} icon={<GitBranch size={13} />} />
    {session?.gitBranch && <ContextRow label="Task branch" value={session.gitBranch} icon={<GitBranch size={13} />} />}
    <ContextRow label="Location" value={project?.path ?? 'Open a project to get started'} />
    <p className="inspector-section-label context-section-spaced">PROJECT GUIDANCE</p>
    <ContextRow label="Loaded" value={session?.projectInstructionFiles.length ? session.projectInstructionFiles.join(' · ') : 'No instruction files loaded'} />
    <p className="inspector-section-label context-section-spaced">WORKING CONTEXT</p>
    <ContextRow label="Compacted turns" value={String(session?.workingContext.compactedTurns ?? 0)} />
    {session?.workingContext.objective && <ContextBlock label="Objective" value={session.workingContext.objective} />}
    {!!session?.workingContext.decisions.length && <ContextList label="Decisions and progress" items={session.workingContext.decisions} />}
    {!!session?.workingContext.repositoryFacts.length && <ContextList label="Repository facts" items={session.workingContext.repositoryFacts} />}
    {!!session?.workingContext.outstandingTasks.length && <ContextList label="Outstanding" items={session.workingContext.outstandingTasks} />}
    <p className="inspector-section-label context-section-spaced">MODEL</p>
    <ContextRow label="Provider" value={provider?.name ?? 'No provider'} />
    <ContextRow label="Model" value={session?.modelId ?? provider?.models[0]?.displayName ?? 'Choose in the composer'} />
    <p className="inspector-section-label context-section-spaced">PERMISSIONS</p>
    <ContextRow label="Mode" value={session ? permissionModeLabel(session.permissionPolicy.mode) : 'Ask'} icon={<Shield size={13} />} />
    <div className="permission-summary"><Check size={13} /><span>Read project files</span><small>{categoryLabel(session, 'read_files')}</small></div>
    <div className="permission-summary"><CircleHelp size={13} /><span>Git status</span><small>{categoryLabel(session, 'git')}</small></div>
    <div className="permission-summary"><CircleHelp size={13} /><span>File edits</span><small>{categoryLabel(session, 'write_files')}</small></div>
    <div className="permission-summary"><CircleHelp size={13} /><span>Agent commands</span><small>{categoryLabel(session, 'shell')}</small></div>
    <div className="permission-summary"><Shield size={13} /><span>Outside project</span><small>{permissionLabel(session?.permissionPolicy.externalFiles)}</small></div>
  </div>;
}

function ContextRow({ label, value, icon }: { label: string; value: string; icon?: ReactNode }) {
  return <div className="context-detail-row"><span>{icon}{label}</span><strong title={value}>{value}</strong></div>;
}

function ContextBlock({ label, value }: { label: string; value: string }) {
  return <div className="context-summary-block"><span>{label}</span><p>{value}</p></div>;
}

function ContextList({ label, items }: { label: string; items: string[] }) {
  return <div className="context-summary-block"><span>{label}</span><ul>{items.map((item, index) => <li key={`${label}-${index}`}>{item}</li>)}</ul></div>;
}

function TaskPanel({ session, demo, project, provider, review, usage }: { session?: AgentSession; demo: boolean; project?: Project; provider?: Provider; review: SessionChanges | null; usage: UsageRecord[] }) {
  if (!session) return <div className="inspector-empty"><Clock3 size={18} /><strong>No active task</strong><p>Start with a prompt to create a task in this project.</p></div>;
  return <div className="task-detail-panel"><div className="task-detail-status"><span className={`task-status-mark status-${session.status}`} />{humanStatus(session.status)}{demo && <span className="task-demo-tag">Example</span>}</div>
    <h3>{session.title || 'New task'}</h3>
    <div className="task-detail-field"><span>Project</span><strong>{project?.name ?? 'Unknown project'}</strong></div>
    <div className="task-detail-field"><span>Model</span><strong>{provider?.models.find(model => model.id === session.modelId)?.displayName ?? `Unavailable · ${session.modelId}`}</strong></div>
    {session.gitBranch && <div className="task-detail-field"><span>Git branch</span><strong>{session.gitBranch}</strong></div>}
    {session.worktreePath && <div className="task-detail-field"><span>Worktree</span><strong title={session.worktreePath}>{session.worktreePath}</strong></div>}
    <div className="task-detail-field"><span>Started</span><strong>{new Date(session.createdAt).toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}</strong></div>
    <div className="task-detail-field"><span>Changed files</span><strong>{review?.files.length ?? 0}</strong></div>
    <div className="task-detail-field"><span>Usage</span><strong>{usage.filter(record => record.sessionId === session.id).reduce((total, record) => total + record.inputTokens + record.outputTokens, 0).toLocaleString()} tokens</strong></div>
    <div className="task-detail-field"><span>Tool rounds</span><strong>{session.toolRounds}</strong></div>
    <div className="task-detail-field"><span>Compacted turns</span><strong>{session.workingContext.compactedTurns}</strong></div>
    {session.projectInstructionFiles.length > 0 && <div className="task-instruction-status"><Check size={13} /><span>Project guidance loaded · {session.projectInstructionFiles.join(' · ')}</span></div>}
    <div className="task-summary-block"><span>REQUEST</span><p>{session.messages.find(message => message.role === 'user')?.content ?? 'No prompt yet.'}</p></div>
    <div className="task-detail-foot"><FilePlus2 size={13} /><span>Task history is saved on this device</span></div>
  </div>;
}

function humanStatus(status: AgentSession['status']) {
  if (status === 'completed') return 'Completed';
  if (status === 'queued') return 'Queued';
  if (status === 'planning') return 'Planning';
  if (status === 'working') return 'Working';
  if (status === 'waiting_for_permission') return 'Waiting for approval';
  if (status === 'waiting_for_user') return 'Waiting for your answer';
  if (status === 'failed') return 'Failed';
  if (status === 'cancelled') return 'Stopped';
  return 'Ready';
}

function permissionLabel(value: AgentSession['permissionPolicy']['shell'] | undefined) {
  return value === 'allow' ? 'Allowed' : value === 'ask' ? 'Ask first' : 'Denied';
}

function categoryLabel(session: AgentSession | undefined, category: 'read_files' | 'git' | 'write_files' | 'shell') {
  const value = !session ? undefined : category === 'read_files' ? session.permissionPolicy.readFiles : category === 'write_files' ? session.permissionPolicy.writeFiles : session.permissionPolicy[category];
  if (value === 'deny') return 'Denied';
  if (value === 'ask') return 'Ask first';
  if (category === 'shell' && session?.permissionPolicy.mode !== 'full_access') return 'Ask first';
  if (category === 'write_files' && session?.permissionPolicy.mode === 'ask') return 'Ask first';
  return value === 'allow' ? 'Allowed' : 'Ask first';
}

function permissionModeLabel(mode: AgentSession['permissionPolicy']['mode']) {
  return mode === 'ask' ? 'Ask' : mode === 'workspace_write' ? 'Workspace Write' : 'Full Access';
}
