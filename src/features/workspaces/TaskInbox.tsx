import { useEffect, useState, type FormEvent } from 'react';
import { ArrowRight, CalendarClock, Clock3, LoaderCircle, Pause, Pin, Play, Plus, Trash2, X } from 'lucide-react';
import type { AgentSession, Project, Provider } from '../../types/domain';
import type { PermissionPolicy, ScheduledTask, ScheduledTaskFrequency, ScheduledTaskRun, WorkspaceMode } from '../../types/domain';
import { command, desktopAvailable } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
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

function defaultScheduleDate() {
  const date = new Date(Date.now() + 5 * 60_000);
  date.setMinutes(date.getMinutes() - date.getTimezoneOffset());
  return date.toISOString().slice(0, 16);
}

function scheduleTimeLabel(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? 'Time unavailable' : new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric', hour: 'numeric', minute: '2-digit' }).format(date);
}

function scheduleFrequencyLabel(value: ScheduledTaskFrequency) {
  return value === 'once' ? 'One time' : value === 'daily' ? 'Every day' : 'Every week';
}

function scheduleRunLabel(value: ScheduledTaskRun['status']) {
  switch (value) {
    case 'starting': return 'Starting';
    case 'running': return 'Running';
    case 'waiting_for_permission': return 'Approval needed';
    case 'waiting_for_user': return 'Your answer needed';
    case 'completed': return 'Completed';
    case 'failed': return 'Failed';
    case 'cancelled': return 'Cancelled';
  }
}

export function TaskInbox({ sessions, projects, providers, permissionPolicy, onSelectSession, onTogglePin, onNewTask }: {
  sessions: AgentSession[];
  projects: Project[];
  providers: Provider[];
  permissionPolicy: PermissionPolicy;
  onSelectSession: (session: AgentSession) => void;
  onTogglePin: (session: AgentSession) => void;
  onNewTask: () => void;
}) {
  const [scheduledTasks, setScheduledTasks] = useState<ScheduledTask[]>([]);
  const [scheduledRuns, setScheduledRuns] = useState<ScheduledTaskRun[]>([]);
  const [scheduleOpen, setScheduleOpen] = useState(false);
  const [scheduleLoading, setScheduleLoading] = useState(false);
  const [schedulePending, setSchedulePending] = useState(false);
  const [scheduleError, setScheduleError] = useState<string | null>(null);
  const [scheduleName, setScheduleName] = useState('');
  const [schedulePrompt, setSchedulePrompt] = useState('');
  const [scheduleProjectId, setScheduleProjectId] = useState(projects[0]?.id ?? '');
  const [scheduleProviderId, setScheduleProviderId] = useState(providers.find(provider => provider.connected && provider.protocol !== 'preview')?.id ?? '');
  const [scheduleModelId, setScheduleModelId] = useState('');
  const [scheduleFrequency, setScheduleFrequency] = useState<ScheduledTaskFrequency>('once');
  const [scheduleWorkspaceMode, setScheduleWorkspaceMode] = useState<WorkspaceMode>(projects[0]?.repositoryRoot ? 'isolated' : 'direct');
  const [scheduleAt, setScheduleAt] = useState(defaultScheduleDate);
  const availableProviders = providers.filter(provider => provider.connected && provider.protocol !== 'preview');
  const selectedProject = projects.find(project => project.id === scheduleProjectId);
  const selectedProvider = availableProviders.find(provider => provider.id === scheduleProviderId);
  const availableModels = selectedProvider?.models.filter(model => model.status !== 'unavailable') ?? [];
  const selectedModelId = availableModels.some(model => model.id === scheduleModelId) ? scheduleModelId : availableModels[0]?.id ?? '';

  useEffect(() => {
    if (!projects.some(project => project.id === scheduleProjectId)) setScheduleProjectId(projects[0]?.id ?? '');
  }, [projects, scheduleProjectId]);
  useEffect(() => {
    if (!availableProviders.some(provider => provider.id === scheduleProviderId)) setScheduleProviderId(availableProviders[0]?.id ?? '');
  }, [availableProviders, scheduleProviderId]);
  useEffect(() => {
    if (!selectedProvider?.models.some(model => model.id === scheduleModelId && model.status !== 'unavailable')) setScheduleModelId(availableModels[0]?.id ?? '');
  }, [availableModels, scheduleModelId, selectedProvider]);
  useEffect(() => {
    if (!selectedProject?.repositoryRoot && scheduleWorkspaceMode === 'isolated') setScheduleWorkspaceMode('direct');
  }, [scheduleWorkspaceMode, selectedProject]);
  useEffect(() => {
    if (!desktopAvailable) return;
    let active = true;
    const refresh = async () => {
      try {
        const [tasks, runs] = await Promise.all([
          command('list_scheduled_tasks', undefined),
          command('list_scheduled_task_runs', undefined),
        ]);
        if (active) { setScheduledTasks(tasks); setScheduledRuns(runs); }
      } catch (reason) {
        if (active) setScheduleError(normalizeError(reason).message);
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);

  async function createSchedule(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!desktopAvailable || !selectedProject || !selectedProvider || !selectedModelId || !schedulePrompt.trim() || schedulePending) return;
    const parsedTime = new Date(scheduleAt);
    if (Number.isNaN(parsedTime.getTime())) { setScheduleError('Choose a valid date and time.'); return; }
    setSchedulePending(true);
    setScheduleError(null);
    try {
      const projectPermissions = selectedProject.permissions;
      await command('create_scheduled_task', { input: {
        name: scheduleName.trim() || null,
        projectId: selectedProject.id,
        providerId: selectedProvider.id,
        modelId: selectedModelId,
        prompt: schedulePrompt.trim(),
        permissionPolicy: {
          ...permissionPolicy,
          ...projectPermissions,
          mode: permissionPolicy.mode,
          readFiles: permissionPolicy.readFiles === 'deny' || projectPermissions.readFiles === 'deny'
            ? 'deny'
            : permissionPolicy.readFiles === 'ask' || projectPermissions.readFiles === 'ask' ? 'ask' : 'allow',
          maxToolRounds: Math.min(permissionPolicy.maxToolRounds, projectPermissions.maxToolRounds),
        },
        workspaceMode: scheduleWorkspaceMode,
        frequency: scheduleFrequency,
        scheduledAt: parsedTime.toISOString(),
      } });
      setScheduledTasks(await command('list_scheduled_tasks', undefined));
      setScheduleOpen(false);
      setScheduleName('');
      setSchedulePrompt('');
      setScheduleAt(defaultScheduleDate());
    } catch (reason) { setScheduleError(normalizeError(reason).message); }
    finally { setSchedulePending(false); }
  }

  async function toggleSchedule(task: ScheduledTask) {
    setScheduleLoading(true);
    setScheduleError(null);
    try {
      await command('set_scheduled_task_enabled', { scheduleId: task.id, enabled: !task.enabled });
      setScheduledTasks(await command('list_scheduled_tasks', undefined));
    } catch (reason) { setScheduleError(normalizeError(reason).message); }
    finally { setScheduleLoading(false); }
  }

  async function removeSchedule(task: ScheduledTask) {
    setScheduleLoading(true);
    setScheduleError(null);
    try {
      await command('delete_scheduled_task', { scheduleId: task.id });
      setScheduledTasks(await command('list_scheduled_tasks', undefined));
      setScheduledRuns(await command('list_scheduled_task_runs', undefined));
    } catch (reason) { setScheduleError(normalizeError(reason).message); }
    finally { setScheduleLoading(false); }
  }

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
        <div className="task-inbox-heading-actions">
          {desktopAvailable && <button type="button" className="task-inbox-schedule-new" onClick={() => { setScheduleOpen(value => !value); setScheduleError(null); }}><CalendarClock size={14} />Schedule task</button>}
          <button type="button" className="task-inbox-new" onClick={onNewTask}><Plus size={15} />New task</button>
        </div>
      </header>

      <dl className="task-inbox-summary" aria-label="Task counts">
        <div><dt>Needs attention</dt><dd>{attentionCount}</dd></div>
        <div><dt>In progress</dt><dd>{runningCount}</dd></div>
        <div><dt>Pinned</dt><dd>{pinnedCount}</dd></div>
      </dl>

      {desktopAvailable && <section className="scheduled-task-manager" aria-labelledby="scheduled-tasks-title">
        <div className="scheduled-task-manager-heading">
          <div><h2 id="scheduled-tasks-title">Scheduled tasks</h2><p>Runs locally. If Octran Code is closed at the scheduled time, it starts when the app opens again; missed repeat intervals are skipped.</p></div>
          {scheduledTasks.length > 0 && <span>{scheduledTasks.filter(task => task.enabled).length} active</span>}
        </div>
        {scheduleError && <p className="scheduled-task-error" role="alert">{scheduleError}</p>}
        {scheduleOpen && <form className="scheduled-task-form" onSubmit={event => void createSchedule(event)}>
          <div className="scheduled-task-form-heading"><strong>Schedule a task</strong><button type="button" onClick={() => setScheduleOpen(false)} aria-label="Close schedule form"><X size={14} /></button></div>
          <label>Task name <input value={scheduleName} maxLength={100} onChange={event => setScheduleName(event.target.value)} placeholder="Optional · uses the prompt if blank" /></label>
          <label>Prompt <textarea value={schedulePrompt} maxLength={32768} onChange={event => setSchedulePrompt(event.target.value)} placeholder="Describe what Octran Code should do when this runs" required rows={3} /></label>
          <div className="scheduled-task-form-grid">
            <label>Project <select value={scheduleProjectId} onChange={event => {
              const next = projects.find(project => project.id === event.target.value);
              setScheduleProjectId(event.target.value);
              setScheduleWorkspaceMode(next?.repositoryRoot ? 'isolated' : 'direct');
              if (!next?.repositoryRoot && scheduleFrequency !== 'once') setScheduleFrequency('once');
            }} required>
              {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
            </select></label>
            <label>Model <select value={selectedProvider ? `${selectedProvider.id}|${selectedModelId}` : ''} onChange={event => {
              const [nextProvider, nextModel] = event.target.value.split('|');
              setScheduleProviderId(nextProvider);
              setScheduleModelId(nextModel);
            }} required disabled={availableProviders.length === 0}>
              {availableProviders.flatMap(provider => provider.models.filter(model => model.status !== 'unavailable').map(model => <option key={`${provider.id}|${model.id}`} value={`${provider.id}|${model.id}`}>{provider.name} · {model.displayName}</option>))}
            </select></label>
            <label>Workspace <select value={scheduleWorkspaceMode} onChange={event => {
              const next = event.target.value as WorkspaceMode;
              setScheduleWorkspaceMode(next);
              if (next === 'direct' && scheduleFrequency !== 'once') setScheduleFrequency('once');
            }}>
              <option value="direct">Work in project</option>
              <option value="isolated" disabled={!selectedProject?.repositoryRoot}>Isolated Git worktree</option>
            </select></label>
            <label>Repeat <select value={scheduleFrequency} onChange={event => {
              const next = event.target.value as ScheduledTaskFrequency;
              setScheduleFrequency(next);
              if (next !== 'once' && selectedProject?.repositoryRoot) setScheduleWorkspaceMode('isolated');
            }}>
              <option value="once">One time</option>
              <option value="daily" disabled={!selectedProject?.repositoryRoot}>Every day</option>
              <option value="weekly" disabled={!selectedProject?.repositoryRoot}>Every week</option>
            </select></label>
            <label className="scheduled-task-time">First run <input type="datetime-local" min={defaultScheduleDate()} value={scheduleAt} onChange={event => setScheduleAt(event.target.value)} required /></label>
          </div>
          <p className="scheduled-task-policy-note">Runs with this project’s saved permissions. Any approval request will wait for you.</p>
          {!selectedProvider && <p className="scheduled-task-error">Connect a provider before scheduling a run.</p>}
          <div className="scheduled-task-form-actions"><button type="button" onClick={() => setScheduleOpen(false)}>Cancel</button><button type="submit" className="task-inbox-new" disabled={schedulePending || !selectedProject || !selectedProvider || !selectedModelId || !schedulePrompt.trim() || scheduleFrequency !== 'once' && scheduleWorkspaceMode !== 'isolated'}>{schedulePending ? <LoaderCircle size={13} className="spin" /> : <Clock3 size={13} />}{schedulePending ? 'Saving' : 'Schedule'}</button></div>
        </form>}
        {scheduledTasks.length ? <ul className="scheduled-task-list">{scheduledTasks.map(task => {
          const project = projects.find(item => item.id === task.projectId);
          const lastRun = scheduledRuns.find(run => run.scheduledTaskId === task.id);
          const finishedOnce = task.frequency === 'once' && !!lastRun;
          return <li className="scheduled-task-row" key={task.id}>
            <span className={`scheduled-task-state${task.enabled ? ' is-enabled' : ''}`} aria-hidden="true" />
            <div className="scheduled-task-main"><strong>{task.name}</strong><span>{project?.name ?? 'Project removed'} · {scheduleFrequencyLabel(task.frequency)} · {task.workspaceMode === 'isolated' ? 'isolated worktree' : 'project workspace'}</span></div>
            <div className="scheduled-task-next"><span>{task.enabled ? `Next · ${scheduleTimeLabel(task.nextRunAt)}` : finishedOnce ? 'One-time task finished' : 'Paused'}</span>{lastRun && <small>Last run · {scheduleRunLabel(lastRun.status)}</small>}</div>
            <div className="scheduled-task-actions">
              {(!finishedOnce || task.enabled) && <button type="button" disabled={scheduleLoading} onClick={() => void toggleSchedule(task)} aria-label={`${task.enabled ? 'Pause' : 'Resume'} ${task.name}`} title={task.enabled ? 'Pause schedule' : 'Resume schedule'}>{task.enabled ? <Pause size={13} /> : <Play size={13} />}</button>}
              <button type="button" disabled={scheduleLoading} onClick={() => void removeSchedule(task)} aria-label={`Delete ${task.name}`} title="Delete schedule"><Trash2 size={13} /></button>
            </div>
          </li>;
        })}</ul> : !scheduleOpen && <p className="scheduled-task-empty">No scheduled tasks yet. Schedule a one-time, daily, or weekly task to run later.</p>}
      </section>}

      {desktopAvailable && scheduledRuns.length > 0 && <section className="scheduled-run-history" aria-labelledby="scheduled-run-history-title">
        <div className="task-inbox-group-heading"><h2 id="scheduled-run-history-title">Scheduled run history</h2><span>{scheduledRuns.length}</span></div>
        <ul className="scheduled-run-list">{scheduledRuns.slice(0, 12).map(run => {
          const session = run.sessionId ? sessionById.get(run.sessionId) : undefined;
          return <li key={run.id}>
            <div><strong>{run.scheduledTaskName}</strong><span>{scheduleTimeLabel(run.scheduledFor)}</span></div>
            <span className={`scheduled-run-status is-${run.status}`}>{scheduleRunLabel(run.status)}</span>
            {run.error && <small>{run.error}</small>}
            {session && <button type="button" onClick={() => onSelectSession(session)}>Open task</button>}
          </li>;
        })}</ul>
      </section>}

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
