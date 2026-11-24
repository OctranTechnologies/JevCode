import { ArrowRight, Pin, Plus } from 'lucide-react';
import type { AgentSession, Project, Provider } from '../../types/domain';
import { groupInboxSessions } from './taskInboxData';
import './taskInbox.css';

function statusLabel(status: AgentSession['status']) {
  switch (status) {
    case 'queued': return 'Queued';
    case 'planning': return 'Planning';
    case 'working': return 'Working';
    case 'waiting_for_permission': return 'Approval needed';
    case 'waiting_for_user': return 'Your answer needed';
    case 'completed': return 'Completed';
    case 'failed': return 'Failed';
    case 'cancelled': return 'Cancelled';
  }
}

function updateLabel(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? 'Update time unavailable' : `Updated ${new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }).format(date)}`;
}

export function TaskInbox({ sessions, projects, providers, onSelectSession, onTogglePin, onNewTask }: {
  sessions: AgentSession[];
  projects: Project[];
  providers: Provider[];
  onSelectSession: (session: AgentSession) => void;
  onTogglePin: (session: AgentSession) => void;
  onNewTask: () => void;
}) {
  const visibleSessions = sessions.filter(session => !session.archivedAt);
  const categorized = groupInboxSessions(visibleSessions);
  const attentionCount = categorized.find(group => group.id === 'attention')?.sessions.length ?? 0;
  const runningCount = categorized.find(group => group.id === 'working')?.sessions.length ?? 0;
  const pinnedCount = visibleSessions.filter(session => session.pinned === true).length;
  const sessionById = new Map(visibleSessions.map(session => [session.id, session]));
  const childCounts = new Map<string, number>();
  for (const item of visibleSessions) {
    if (item.parentSessionId) childCounts.set(item.parentSessionId, (childCounts.get(item.parentSessionId) ?? 0) + 1);
  }

  return <section className="task-inbox-view" aria-label="Task inbox">
    <div className="task-inbox-content">
      <header className="task-inbox-heading">
        <div>
          <h1>Task inbox</h1>
          <p>Tasks across all projects, ordered by what needs your attention.</p>
        </div>
        <button type="button" className="task-inbox-new" onClick={onNewTask}><Plus size={15} />New task</button>
      </header>

      <dl className="task-inbox-summary" aria-label="Task counts">
        <div><dt>Needs attention</dt><dd>{attentionCount}</dd></div>
        <div><dt>In progress</dt><dd>{runningCount}</dd></div>
        <div><dt>Pinned</dt><dd>{pinnedCount}</dd></div>
      </dl>

      {visibleSessions.length === 0 ? <div className="task-inbox-empty">
        <h2>No tasks yet</h2>
        <p>Start a task in any project. It will appear here with its status and recent activity.</p>
        <button type="button" className="task-inbox-new" onClick={onNewTask}><Plus size={15} />Start a task</button>
      </div> : <div className="task-inbox-groups">
        {categorized.map(group => <section className="task-inbox-group" key={group.id} aria-labelledby={`task-inbox-${group.id}`}>
          <div className="task-inbox-group-heading"><h2 id={`task-inbox-${group.id}`}>{group.title}</h2><span>{group.sessions.length}</span></div>
          {group.sessions.length ? <ul className="task-inbox-list">
            {group.sessions.map(session => {
              const project = projects.find(item => item.id === session.projectId);
              const provider = providers.find(item => item.id === session.providerId);
              const model = provider?.models.find(item => item.id === session.modelId);
              const parent = session.parentSessionId ? sessionById.get(session.parentSessionId) : undefined;
              const childCount = childCounts.get(session.id) ?? 0;
              const followUpCount = session.queuedFollowUps?.length ?? 0;
              const branch = session.gitBranch ?? session.baseBranch;
              return <li className={`task-inbox-row${session.pinned ? ' is-pinned' : ''}`} key={session.id}>
                <button type="button" className="task-inbox-open" onClick={() => onSelectSession(session)} title={`Open ${session.title}`}>
                  <span className={`task-inbox-state is-${session.status}`} aria-hidden="true" />
                  <span className="task-inbox-row-main">
                    <strong>{session.title || 'New task'}</strong>
                    <span className="task-inbox-meta">
                      <span>{project?.name ?? 'Unknown project'}</span>
                      <span>{provider?.name ?? session.providerId}{model ? ` · ${model.displayName}` : ''}</span>
                      {parent && <span>Child of {parent.title || 'New task'}</span>}
                      <span>{branch ? `Branch ${branch}` : 'No branch'}</span>
                    </span>
                  </span>
                  <span className="task-inbox-row-state">
                    <span className={`task-inbox-status is-${session.status}`}>{statusLabel(session.status)}</span>
                    {followUpCount > 0 && <span className="task-inbox-queue-count">{followUpCount} queued {followUpCount === 1 ? 'follow-up' : 'follow-ups'}</span>}
                    {childCount > 0 && <span className="task-inbox-queue-count">{childCount} delegated {childCount === 1 ? 'task' : 'tasks'}</span>}
                    <time dateTime={session.updatedAt}>{updateLabel(session.updatedAt)}</time>
                  </span>
                  <ArrowRight size={15} className="task-inbox-arrow" aria-hidden="true" />
                </button>
                <button type="button" className={`task-inbox-pin${session.pinned ? ' is-pinned' : ''}`} onClick={() => onTogglePin(session)} aria-label={`${session.pinned ? 'Unpin' : 'Pin'} ${session.title || 'task'}`} title={session.pinned ? 'Unpin task' : 'Pin task'}>
                  <Pin size={14} fill={session.pinned ? 'currentColor' : 'none'} />
                </button>
              </li>;
            })}
          </ul> : <p className="task-inbox-group-empty">{group.empty}</p>}
        </section>)}
      </div>}
    </div>
  </section>;
}
