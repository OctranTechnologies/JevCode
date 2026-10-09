import { BarChart3, ChevronRight, Folder, FolderPlus, Layers, MessageSquare, Monitor, Plus } from 'lucide-react';
import { Brand } from '../../components/Brand';
import type { AgentSession, Project } from '../../types/domain';

export type View = 'agent' | 'overview' | 'providers' | 'permissions' | 'usage';
export function Sidebar({ projects, projectId, sessions, sessionId, view, onProject, onSession, onView, onNew, onOpen, opening }: {
  projects: Project[]; projectId: string; sessions: AgentSession[]; sessionId: string | null; view: View;
  onProject: (id: string) => void; onSession: (session: AgentSession) => void; onView: (view: View) => void; onNew: () => void; onOpen: () => void; opening: boolean;
}) {
  return <aside className="sidebar"><div className="app-brand"><Brand /><span>JevCode</span><span className="version">0.1</span></div><button className="new-session" onClick={onNew}><Plus size={17} />New session</button>
    <nav className="project-navigation" aria-label="Projects"><h2>Projects</h2>{projects.map(project => <button className={projectId === project.id ? 'nav-row active' : 'nav-row'} key={project.id} title={project.path} onClick={() => onProject(project.id)} aria-current={projectId === project.id ? 'page' : undefined}><Folder size={17} /><span>{project.name}</span></button>)}<button className="nav-row" onClick={onOpen} disabled={opening}><FolderPlus size={17} /><span>{opening ? 'Opening…' : 'Open folder'}</span></button></nav>
    <nav className="session-navigation" aria-label="Sessions"> <h2>Sessions</h2>{sessions.filter(session => session.projectId === projectId).length ? sessions.filter(session => session.projectId === projectId).map(session => <button key={session.id} className={sessionId === session.id && view === 'agent' ? 'nav-row active' : 'nav-row'} onClick={() => onSession(session)} title={session.title}><MessageSquare size={15} /><span>{session.title}</span>{['queued', 'planning', 'working'].includes(session.status) && <span className="status-dot pulse" />}</button>) : <p className="empty-sessions">Your sessions will appear here.</p>}</nav>
    <nav className="utility-navigation" aria-label="Application"><button className={view === 'providers' ? 'nav-row active' : 'nav-row'} onClick={() => onView('providers')}><Layers size={18} /><span>Accounts</span><ChevronRight size={15} /></button><button className={view === 'usage' ? 'nav-row active' : 'nav-row'} onClick={() => onView('usage')}><BarChart3 size={18} /><span>Usage</span><ChevronRight size={15} /></button></nav><div className="workspace-footer"><Monitor size={17} /><div>Local workspace<small>{projects.length} {projects.length === 1 ? 'project' : 'projects'}</small></div></div>
  </aside>;
}
