import type { AgentSession, Bootstrap, Project, Provider, UsageRecord } from '../types/domain';

export interface DesktopState { data: Bootstrap | null; sessions: AgentSession[]; usage: UsageRecord[] }
export type Action = { type: 'bootstrap'; data: Bootstrap } | { type: 'session'; session: AgentSession } | { type: 'usage'; record: UsageRecord } | { type: 'project'; project: Project } | { type: 'providers'; providers: Provider[] };
export function mergeSession(sessions: AgentSession[], session: AgentSession): AgentSession[] {
  const existing = sessions.find(item => item.id === session.id);
  if (existing && existing.updatedAt > session.updatedAt) return sessions;
  return [...sessions.filter(item => item.id !== session.id), session].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
}
export function desktopReducer(state: DesktopState, action: Action): DesktopState {
  switch (action.type) {
    case 'bootstrap': return { data: action.data, sessions: state.sessions.reduce(mergeSession, action.data.sessions), usage: [...new Map([...action.data.usage, ...state.usage].map(record => [record.id, record])).values()] };
    case 'session': return { ...state, sessions: mergeSession(state.sessions, action.session) };
    case 'usage': return { ...state, usage: [...state.usage.filter(record => record.id !== action.record.id), action.record] };
    case 'project': return state.data ? { ...state, data: { ...state.data, workspace: { ...state.data.workspace, projects: [...state.data.workspace.projects.filter(project => project.id !== action.project.id), action.project] } } } : state;
    case 'providers': return state.data ? { ...state, data: { ...state.data, providers: action.providers } } : state;
  }
}
