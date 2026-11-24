import type { AgentSession } from '../../types/domain';

type InboxGroup = {
  id: 'attention' | 'working' | 'completed' | 'cancelled';
  title: string;
  empty: string;
  statuses: AgentSession['status'][];
};

const groups: InboxGroup[] = [
  { id: 'attention', title: 'Needs attention', empty: 'Nothing needs your attention.', statuses: ['waiting_for_permission', 'waiting_for_user', 'failed'] },
  { id: 'working', title: 'In progress', empty: 'No tasks are running.', statuses: ['queued', 'planning', 'working'] },
  { id: 'completed', title: 'Completed', empty: 'Completed tasks will appear here.', statuses: ['completed'] },
  { id: 'cancelled', title: 'Cancelled', empty: 'No cancelled tasks.', statuses: ['cancelled'] },
];

export function groupInboxSessions(sessions: AgentSession[]) {
  const active = sessions.filter(session => !session.archivedAt);
  return groups.map(group => ({
    ...group,
    sessions: active
      .filter(session => group.statuses.includes(session.status))
      .sort((left, right) => Number(right.pinned === true) - Number(left.pinned === true) || right.updatedAt.localeCompare(left.updatedAt)),
  }));
}
