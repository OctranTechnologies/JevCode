import {
  ChevronDown, GitBranch, Moon, PanelLeftClose, PanelLeftOpen, PanelRight,
  Search, ShieldCheck, Sun,
} from 'lucide-react';
import type { AgentSession, Project, WorkspaceMode } from '../../types/domain';
import { primaryShortcutModifier } from '../../lib/shortcuts';

export function WorkspaceHeader({
  projects, projectId, project, branch, onProject, session, sidebarCollapsed, onToggleSidebar, inspectorOpen, onToggleInspector,
  darkMode, onToggleTheme, onSearch, onAskReads, askReads,
  workspaceMode, onWorkspaceMode, baseBranch, onBaseBranch, branches, canIsolate, workspaceModeLocked,
}: {
  projects: Project[];
  projectId: string;
  project?: Project;
  branch: string | null;
  onProject: (id: string) => void;
  session?: AgentSession;
  workspaceMode: WorkspaceMode;
  onWorkspaceMode: (mode: WorkspaceMode) => void;
  baseBranch: string;
  onBaseBranch: (branch: string) => void;
  branches: string[];
  canIsolate: boolean;
  workspaceModeLocked: boolean;
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  inspectorOpen: boolean;
  onToggleInspector: () => void;
  darkMode: boolean;
  onToggleTheme: () => void;
  onSearch: () => void;
  askReads: boolean;
  onAskReads: (value: boolean) => void;
}) {
  return <header className="workspace-toolbar">
    <div className="toolbar-project-group">
      <button className="quiet-icon-button toolbar-sidebar-toggle" onClick={onToggleSidebar} title={sidebarCollapsed ? 'Show sidebar' : 'Hide sidebar'} aria-label={sidebarCollapsed ? 'Show sidebar' : 'Hide sidebar'}>{sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}</button>
      <div className="project-title-wrap"><label className="sr-only" htmlFor="project-switcher">Current project</label>
        {projects.length > 1 ? <span className="project-switcher-wrap"><select id="project-switcher" value={projectId} onChange={event => onProject(event.target.value)} aria-label="Current project">{projects.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select><ChevronDown size={12} /></span> : <strong className="current-project-title" title={project?.path}>{project?.name ?? 'No project selected'}</strong>}
      </div>
      <span className="toolbar-divider" />
      <span className="branch-indicator" title={branch ? `Git branch ${branch}` : 'Git branch not detected'}><GitBranch size={13} /><span>{branch ?? 'No branch'}</span></span>
    </div>
    <div className="toolbar-actions">
      <details className="permission-menu"><summary title="Review tool permissions"><ShieldCheck size={14} /><span>Tool access</span><ChevronDown size={12} /></summary><div className="permission-popover"><strong>Session permissions</strong><p>Reads, edits, Git, commands, and outside-project access follow the project policy.</p><label><input type="checkbox" checked={session ? session.permissionPolicy.readFiles === 'ask' : askReads} disabled={!!session} onChange={event => onAskReads(event.target.checked)} />Ask before reading files</label><small>Change edit, command, and outside-project access in the project overview.</small>{session && <small>Start a new task to change this session's permissions.</small>}</div></details>
      <label className="toolbar-workspace-wrap" title="Choose where this task works">
        <GitBranch size={13} />
        <span className="sr-only">Task workspace</span>
        <select value={workspaceMode} onChange={event => onWorkspaceMode(event.target.value as WorkspaceMode)} disabled={workspaceModeLocked} aria-label="Task workspace">
          <option value="direct">Work directly in project</option>
          <option value="isolated" disabled={!canIsolate}>Create isolated workspace</option>
        </select>
        <ChevronDown size={12} />
      </label>
      {workspaceMode === 'isolated' && <>
        <span className="toolbar-workspace-base-caption">from</span>
        <label className="toolbar-workspace-base-wrap" title="Choose the branch this task starts from">
          <span className="sr-only">Starting branch</span>
          <select value={baseBranch} onChange={event => onBaseBranch(event.target.value)} disabled={workspaceModeLocked || !branches.length} aria-label="Starting branch">
            {branches.length ? branches.map(item => <option key={item} value={item}>{item}</option>) : <option value="">No local branches</option>}
          </select>
          <ChevronDown size={12} />
        </label>
      </>}
      <button className="quiet-icon-button toolbar-search" onClick={onSearch} aria-label={`Search projects and tasks, ${primaryShortcutModifier} K`}><Search size={15} /><kbd aria-hidden="true">{primaryShortcutModifier} K</kbd></button>
      <button className="quiet-icon-button theme-toggle" onClick={onToggleTheme} title={darkMode ? 'Switch to light appearance' : 'Switch to dark appearance'} aria-label={darkMode ? 'Switch to light appearance' : 'Switch to dark appearance'}>{darkMode ? <Sun size={15} /> : <Moon size={15} />}</button>
      <button className={`quiet-icon-button inspector-toggle${inspectorOpen ? ' is-active' : ''}`} onClick={onToggleInspector} title={inspectorOpen ? 'Hide task details' : 'Show task details'} aria-label={inspectorOpen ? 'Hide task details' : 'Show task details'} aria-pressed={inspectorOpen}><PanelRight size={16} /></button>
    </div>
  </header>;
}
