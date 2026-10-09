import {
  Archive, BarChart3, ChevronDown, ChevronsLeft, Copy, ExternalLink, Folder, FolderPlus, GitBranch,
  GitFork, Layers3, MessageSquare, MoreHorizontal, Pencil, Plus, Puzzle, RotateCcw, Search, Settings2, ShieldCheck, Trash2, X,
} from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { Brand } from '../../components/Brand';
import { displayPath } from '../../lib/paths';
import type { AgentSession, Project, Provider } from '../../types/domain';
import { primaryShortcutModifier } from '../../lib/shortcuts';
import type { View } from './Sidebar';

export function WorkspaceSidebar({
  projects, sessions, projectId, sessionId, view, provider, collapsed, opening, showArchived, onShowArchived, onCollapse,
  onProject, onSession, onRenameSession, onArchiveSession, onDeleteSession, onDuplicateSession, onForkSession,
  onView, onNew, onOpen, onCreate, onRemoveRecent, onReveal, onSearch,
}: {
  projects: Project[];
  sessions: AgentSession[];
  projectId: string;
  sessionId: string | null;
  view: View;
  provider?: Provider;
  collapsed: boolean;
  opening: boolean;
  showArchived: boolean;
  onShowArchived: () => void;
  onCollapse: () => void;
  onProject: (id: string) => void;
  onSession: (session: AgentSession) => void;
  onRenameSession: (session: AgentSession) => void;
  onArchiveSession: (session: AgentSession) => void;
  onDeleteSession: (session: AgentSession) => void;
  onDuplicateSession: (session: AgentSession) => void;
  onForkSession: (session: AgentSession) => void;
  onView: (view: View) => void;
  onNew: () => void;
  onOpen: () => void;
  onCreate: () => void;
  onRemoveRecent: (project: Project) => void;
  onReveal: (project: Project) => void;
  onSearch: () => void;
}) {
  const [projectMenu, setProjectMenu] = useState<string | null>(null);
  const modifier = primaryShortcutModifier;
  const [taskMenu, setTaskMenu] = useState<string | null>(null);
  const recentProjects = projects.filter(project => project.isRecent).slice(0, 5);
  const visibleTasks = sessions.filter(session => showArchived ? !!session.archivedAt : !session.archivedAt);
  const projectRow = (project: Project, prefix: string) => {
    const active = projectId === project.id && view === 'overview';
    const menuOpen = projectMenu === `${prefix}-${project.id}`;
    return <div className="sidebar-project-wrap" key={`${prefix}-${project.id}`}>
      <div className="sidebar-project-row">
        <button className={`sidebar-row${active ? ' is-active' : ''}`} onClick={() => onProject(project.id)} title={displayPath(project.path)} aria-current={active ? 'page' : undefined}>
          <Folder size={15} /><span>{project.name}</span>{active && <span className="row-presence" />}
        </button>
        <button className="project-row-menu-toggle" onClick={() => setProjectMenu(menuOpen ? null : `${prefix}-${project.id}`)} title={`${project.name} actions`} aria-label={`${project.name} project actions`} aria-expanded={menuOpen}><MoreHorizontal size={15} /></button>
      </div>
      {menuOpen && <div className="sidebar-project-actions" role="group" aria-label={`${project.name} actions`}>
        <button onClick={() => { setProjectMenu(null); onReveal(project); }}><ExternalLink size={13} />Reveal in file manager</button>
        {project.isRecent && <button onClick={() => { setProjectMenu(null); onRemoveRecent(project); }}><X size={13} />Remove from recents</button>}
      </div>}
    </div>;
  };
  return <aside className={`workbench-sidebar${collapsed ? ' is-collapsed' : ''}`} aria-label="Workspace navigation">
    <div className="sidebar-brand-row">
      <div className="sidebar-brand"><Brand /><span>JevCode</span></div>
      <button className="quiet-icon-button collapse-sidebar" onClick={onCollapse} title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'} aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
        <ChevronsLeft size={17} />
      </button>
    </div>

    <button className="new-task-button" onClick={onNew} title={`New task · ${modifier} N`}>
      <Plus size={16} strokeWidth={2.2} /><span>New task</span><kbd>{modifier} N</kbd>
    </button>

    <button className="sidebar-search" onClick={onSearch} title={`Search projects and tasks · ${modifier} K`}>
      <Search size={15} /><span>Search</span><kbd>{modifier} K</kbd>
    </button>

    <div className="sidebar-scroll">
      {recentProjects.length > 0 && <SidebarSection title="Recent projects" icon={<ChevronDown size={12} />}>
        {recentProjects.map(project => projectRow(project, 'recent'))}
      </SidebarSection>}

      <SidebarSection title={showArchived ? 'Archived tasks' : 'Recent tasks'} icon={<ChevronDown size={12} />} action={<button className="task-filter-toggle" onClick={onShowArchived}>{showArchived ? 'Recent' : 'Archived'}</button>}>
        {visibleTasks.length > 0 ? visibleTasks.map(session => {
          const active = sessionId === session.id && view === 'agent';
          const menuOpen = taskMenu === session.id;
          return <div className="sidebar-task-wrap" key={session.id}>
            <div className="sidebar-task-row">
              <button className={`sidebar-row task-row${active ? ' is-active' : ''}`} onClick={() => onSession(session)} title={session.title} aria-current={active ? 'page' : undefined}>
                <TaskState status={session.status} /><span>{session.title || 'New task'}</span>
              </button>
              <button className="task-row-menu-toggle" onClick={() => setTaskMenu(menuOpen ? null : session.id)} title={`${session.title} actions`} aria-label={`${session.title} task actions`} aria-expanded={menuOpen}><MoreHorizontal size={15} /></button>
            </div>
            {menuOpen && <div className="sidebar-project-actions task-row-actions" role="group" aria-label={`${session.title} task actions`}>
              <button onClick={() => { setTaskMenu(null); onRenameSession(session); }}><Pencil size={13} />Rename</button>
              {session.archivedAt
                ? <button onClick={() => { setTaskMenu(null); onArchiveSession(session); }}><RotateCcw size={13} />Resume task</button>
                : <button onClick={() => { setTaskMenu(null); onArchiveSession(session); }}><Archive size={13} />Archive</button>}
              <button onClick={() => { setTaskMenu(null); onDuplicateSession(session); }}><Copy size={13} />Duplicate</button>
              <button onClick={() => { setTaskMenu(null); onForkSession(session); }}><GitFork size={13} />Fork</button>
              <button className="task-delete-action" onClick={() => { setTaskMenu(null); onDeleteSession(session); }}><Trash2 size={13} />Delete</button>
            </div>}
          </div>;
        }) : <p className="sidebar-empty">{showArchived ? 'No archived tasks.' : 'Your tasks will show up here.'}</p>}
      </SidebarSection>

      <SidebarSection title="Projects" icon={<ChevronDown size={12} />} action={<button className="section-add" onClick={onCreate} title="Create project" aria-label="Create project"><Plus size={14} /></button>}>
        {projects.length > 0 ? projects.map(project => projectRow(project, 'all')) : <button className="sidebar-row add-project-row" onClick={onOpen} disabled={opening} title="Open a local folder">
          <FolderPlus size={15} /><span>{opening ? 'Opening project…' : 'Open a folder or repository'}</span>
        </button>}
      </SidebarSection>
    </div>

    <div className="sidebar-bottom">
      <nav className="sidebar-utilities" aria-label="Settings and reports">
        <SidebarAction icon={<Settings2 size={16} />} label="Settings" active={view === 'providers'} onClick={() => onView('providers')} />
        <SidebarAction icon={<ShieldCheck size={16} />} label="Permissions" active={view === 'permissions'} onClick={() => onView('permissions')} />
        <SidebarAction icon={<Puzzle size={16} />} label="MCP" active={view === 'mcp'} onClick={() => onView('mcp')} />
        <SidebarAction icon={<BarChart3 size={16} />} label="Usage" active={view === 'usage'} onClick={() => onView('usage')} />
      </nav>
      <button className="account-provider" onClick={() => onView('providers')} title={provider?.connected ? `${provider.name} · Connected` : 'Set up a provider'}>
        <span className={`provider-avatar${provider?.protocol === 'preview' ? ' is-local' : ''}`}>{provider?.protocol === 'preview' ? <GitBranch size={14} /> : <Layers3 size={14} />}</span>
        <span className="account-copy"><strong>{provider?.name ?? 'Choose a provider'}</strong><small>{provider?.connected ? 'Ready for a new task' : 'Set up a provider'}</small></span>
        <span className={`provider-connection${provider?.connected ? ' is-connected' : ''}`} aria-hidden="true" />
      </button>
    </div>
  </aside>;
}

function SidebarSection({ title, icon, action, children }: { title: string; icon: ReactNode; action?: ReactNode; children: ReactNode }) {
  return <section className="sidebar-section">
    <div className="sidebar-section-heading"><h2>{title}</h2><span className="section-chevron">{icon}</span>{action}</div>
    <div className="sidebar-section-items">{children}</div>
  </section>;
}

function TaskState({ status }: { status: AgentSession['status'] }) {
  if (['queued', 'planning', 'working'].includes(status)) return <span className="task-state is-running" aria-label={status} />;
  if (status === 'waiting_for_permission' || status === 'waiting_for_user') return <span className="task-state is-waiting" aria-label={status} />;
  if (status === 'failed') return <span className="task-state is-error" aria-label="Failed" />;
  return <MessageSquare size={14} />;
}

function SidebarAction({ icon, label, active, onClick }: { icon: ReactNode; label: string; active: boolean; onClick: () => void }) {
  return <button className={`sidebar-row utility-row${active ? ' is-active' : ''}`} onClick={onClick} title={label} aria-current={active ? 'page' : undefined}>
    {icon}<span>{label}</span>
  </button>;
}
