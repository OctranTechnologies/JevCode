import type { AgentSession, Project } from '../types/domain';

/** Select the last usable task so the desktop can return to where the user left off. */
export function latestRestorableSession(sessions: AgentSession[], projects: Project[]): AgentSession | undefined {
  const projectIds = new Set(projects.map(project => project.id));
  return sessions
    .filter(session => !session.archivedAt && projectIds.has(session.projectId))
    .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))[0];
}
