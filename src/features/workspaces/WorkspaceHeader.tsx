import {
  ChevronDown, GitBranch, Moon, PanelLeftClose, PanelLeftOpen, PanelRight,
  Search, ShieldCheck, Sun,
} from 'lucide-react';
import type { AgentSession, Project, Provider } from '../../types/domain';
import { ModelPicker } from '../models/ModelPicker';
import type { ModelPreferences, ModelReference } from '../../types/domain';

export function WorkspaceHeader({
  projects, projectId, project, branch, providers, modelPreferences, providerId, modelId, onProject,
  onModel, onToggleFavorite, onSetDefault, session, modelLocked, sidebarCollapsed, onToggleSidebar, inspectorOpen, onToggleInspector,
  darkMode, onToggleTheme, onSearch, onAskReads, askReads,
}: {
  projects: Project[];
  projectId: string;
  project?: Project;
  branch: string | null;
  providers: Provider[];
  modelPreferences: ModelPreferences;
  providerId: string;
  modelId: string;
  onProject: (id: string) => void;
  onModel: (reference: ModelReference) => void;
  onToggleFavorite: (reference: ModelReference) => void;
  onSetDefault: (reference: ModelReference) => void;
  session?: AgentSession;
  modelLocked: boolean;
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
      <details className="permission-menu"><summary title="Review tool permissions"><ShieldCheck size={14} /><span>Read-only</span><ChevronDown size={12} /></summary><div className="permission-popover"><strong>Session permissions</strong><p>JevCode can read project files. Editing files and running shell commands are disabled.</p><label><input type="checkbox" checked={session ? session.permissionPolicy.readFiles === 'ask' : askReads} disabled={!!session} onChange={event => onAskReads(event.target.checked)} />Ask before reading files</label>{session && <small>Start a new task to change its permissions.</small>}</div></details>
      <ModelPicker providers={providers} preferences={modelPreferences} providerId={providerId} modelId={modelId} locked={modelLocked} compact onSelect={onModel} onToggleFavorite={onToggleFavorite} onSetDefault={onSetDefault} />
      <button className="quiet-icon-button toolbar-search" onClick={onSearch} title="Search · Ctrl K" aria-label="Search"><Search size={15} /><kbd>Ctrl K</kbd></button>
      <button className="quiet-icon-button theme-toggle" onClick={onToggleTheme} title={darkMode ? 'Switch to light appearance' : 'Switch to dark appearance'} aria-label={darkMode ? 'Switch to light appearance' : 'Switch to dark appearance'}>{darkMode ? <Sun size={15} /> : <Moon size={15} />}</button>
      <button className={`quiet-icon-button inspector-toggle${inspectorOpen ? ' is-active' : ''}`} onClick={onToggleInspector} title={inspectorOpen ? 'Hide task details' : 'Show task details'} aria-label={inspectorOpen ? 'Hide task details' : 'Show task details'} aria-pressed={inspectorOpen}><PanelRight size={16} /></button>
    </div>
  </header>;
}
