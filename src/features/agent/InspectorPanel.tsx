import { useState } from 'react';
import {
  Check, ChevronDown, CircleHelp, Clock3, Copy, FileCode2, FilePlus2, GitBranch,
  ListChecks, Shield, Terminal, X,
} from 'lucide-react';
import type { ReactNode } from 'react';
import type { AgentSession, Project, Provider } from '../../types/domain';
import { demoChanges, demoDiff, demoTerminal } from '../../app/demo';

export type InspectorTab = 'changes' | 'diff' | 'terminal' | 'context' | 'task';

export function InspectorPanel({
  project, branch, session, provider, demo, open, onClose, tab, onTab,
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
}) {
  const [selectedFile, setSelectedFile] = useState(demoChanges[0].path);
  const tabs: { id: InspectorTab; label: string; icon: ReactNode; count?: string }[] = [
    { id: 'changes', label: 'Files', icon: <FileCode2 size={13} />, count: demo ? '2' : undefined },
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
      {tab === 'changes' && <ChangesPanel demo={demo} onSelect={path => { setSelectedFile(path); onTab('diff'); }} />}
      {tab === 'diff' && <DiffPanel demo={demo} selectedFile={selectedFile} />}
      {tab === 'terminal' && <TerminalPanel demo={demo} />}
      {tab === 'context' && <ContextPanel project={project} branch={branch} session={session} provider={provider} />}
      {tab === 'task' && <TaskPanel session={session} demo={demo} project={project} provider={provider} />}
    </div>
  </aside>;
}

function ChangesPanel({ demo, onSelect }: { demo: boolean; onSelect: (path: string) => void }) {
  if (!demo) return <div className="inspector-empty"><FileCode2 size={18} /><strong>No file changes</strong><p>Changes will appear here when file editing is available for this workspace.</p></div>;
  return <div className="changed-files-panel"><div className="inspector-section-label"><span>2 FILES</span><button title="Collapse file list" aria-label="Collapse file list"><ChevronDown size={14} /></button></div>
    {demoChanges.map(change => <button key={change.path} className="changed-file-row" onClick={() => onSelect(change.path)}><span className={`file-state-mark ${change.state}`} />
      <span className="changed-file-copy"><strong>{change.path.split('/').at(-1)}</strong><small>{change.path.split('/').slice(0, -1).join('/')}</small></span>
      <span className="change-counts"><span>+{change.additions}</span><span>−{change.deletions}</span></span>
    </button>)}
    <div className="changes-summary"><span>Sample diff</span><span><b>+68</b> <i>−8</i></span></div>
  </div>;
}

function DiffPanel({ demo, selectedFile }: { demo: boolean; selectedFile: string }) {
  if (!demo) return <div className="inspector-empty"><ListChecks size={18} /><strong>Nothing to review yet</strong><p>Diffs will be available when the agent can edit project files.</p></div>;
  return <div className="diff-view"><div className="diff-file-heading"><FileCode2 size={13} /><span>{selectedFile}</span><button title="Copy diff" aria-label="Copy diff" onClick={() => void navigator.clipboard?.writeText(demoDiff.map(line => line.text).join('\n'))}><Copy size={13} /></button></div>
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
    <ContextRow label="Location" value={project?.path ?? 'Open a project to get started'} />
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

function TaskPanel({ session, demo, project, provider }: { session?: AgentSession; demo: boolean; project?: Project; provider?: Provider }) {
  if (!session) return <div className="inspector-empty"><Clock3 size={18} /><strong>No active task</strong><p>Start with a prompt to create a task in this project.</p></div>;
  return <div className="task-detail-panel"><div className="task-detail-status"><span className={`task-status-mark status-${session.status}`} />{humanStatus(session.status)}{demo && <span className="task-demo-tag">Example</span>}</div>
    <h3>{session.title || 'New task'}</h3>
    <div className="task-detail-field"><span>Project</span><strong>{project?.name ?? 'Unknown project'}</strong></div>
    <div className="task-detail-field"><span>Model</span><strong>{provider?.models.find(model => model.id === session.modelId)?.displayName ?? `Unavailable · ${session.modelId}`}</strong></div>
    <div className="task-detail-field"><span>Started</span><strong>{new Date(session.createdAt).toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}</strong></div>
    <div className="task-detail-field"><span>Tool rounds</span><strong>{session.toolRounds}</strong></div>
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
