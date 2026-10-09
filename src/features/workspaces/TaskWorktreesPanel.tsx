import { useEffect, useState } from 'react';
import { ChevronDown, CircleAlert, Copy, GitBranch, GitCommitHorizontal, GitMerge, GitPullRequest, LoaderCircle, Trash2 } from 'lucide-react';
import { command } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { AgentSession, TaskWorktree } from '../../types/domain';

type Props = {
  projectId: string;
  desktopAllowed: boolean;
  onSessionUpdated: (session: AgentSession) => void;
  onProjectGitUpdated: () => Promise<void>;
  onError: (message: string | null) => void;
};

export function TaskWorktreesPanel({ projectId, desktopAllowed, onSessionUpdated, onProjectGitUpdated, onError }: Props) {
  const [worktrees, setWorktrees] = useState<TaskWorktree[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [diffs, setDiffs] = useState<Record<string, string>>({});
  const [expanded, setExpanded] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Record<string, { message: string; conflicts: string[] }>>({});
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setLoadError(null);
    if (!desktopAllowed) {
      setWorktrees([]);
      setLoading(false);
      return () => { active = false; };
    }
    void command('project_task_worktrees', { projectId }).then(values => {
      if (active) setWorktrees(values);
    }).catch(error => {
      if (active) setLoadError(normalizeError(error).message);
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [desktopAllowed, projectId]);

  async function showDiff(worktree: TaskWorktree) {
    if (expanded === worktree.sessionId) {
      setExpanded(null);
      return;
    }
    setExpanded(worktree.sessionId);
    if (diffs[worktree.sessionId] !== undefined) return;
    setBusyId(worktree.sessionId);
    try {
      const diff = await command('task_worktree_diff', { sessionId: worktree.sessionId });
      setDiffs(current => ({ ...current, [worktree.sessionId]: diff }));
    } catch (error) { setFeedback(current => ({ ...current, [worktree.sessionId]: { message: normalizeError(error).message, conflicts: [] } })); }
    finally { setBusyId(null); }
  }

  async function runAction(worktree: TaskWorktree, action: 'commit' | 'apply' | 'remove') {
    if (busyId) return;
    setBusyId(worktree.sessionId);
    setFeedback(current => ({ ...current, [worktree.sessionId]: { message: '', conflicts: [] } }));
    onError(null);
    try {
      if (action === 'remove') {
        const updated = await command('remove_task_worktree', { sessionId: worktree.sessionId });
        onSessionUpdated(updated);
        setWorktrees(current => current.filter(item => item.sessionId !== worktree.sessionId));
        setFeedback(current => ({ ...current, [worktree.sessionId]: { message: 'Checkout removed. The task branch remains in Git.', conflicts: [] } }));
        return;
      }
      const result = await command(action === 'commit' ? 'commit_task_worktree' : 'apply_task_worktree', { sessionId: worktree.sessionId });
      if (result.worktree) setWorktrees(current => current.map(item => item.sessionId === worktree.sessionId ? result.worktree! : item));
      setFeedback(current => ({ ...current, [worktree.sessionId]: { message: result.message, conflicts: result.conflicts } }));
      setDiffs(current => { const next = { ...current }; delete next[worktree.sessionId]; return next; });
      if (action === 'apply' && result.conflicts.length === 0) await onProjectGitUpdated();
    } catch (error) {
      setFeedback(current => ({ ...current, [worktree.sessionId]: { message: normalizeError(error).message, conflicts: [] } }));
    } finally { setBusyId(null); }
  }

  return <section className="project-section task-workspaces-section" aria-label="Task workspaces">
    <div className="project-section-heading"><div><h2>Task workspaces</h2><span>Separate copies for parallel tasks</span></div><span className={`project-change-count${worktrees.length ? ' has-changes' : ''}`}>{loading ? '…' : worktrees.length}</span></div>
    {loading ? <div className="task-worktree-skeleton" aria-label="Loading task workspaces"><i /><i /></div>
      : loadError ? <p className="project-section-empty task-worktree-error"><CircleAlert size={13} />{loadError}</p>
        : worktrees.length ? <ul className="task-worktree-list">{worktrees.map(worktree => {
          const isBusy = busyId === worktree.sessionId;
          const running = ['queued', 'planning', 'working', 'waiting_for_permission'].includes(worktree.taskStatus);
          const message = feedback[worktree.sessionId];
          const diff = diffs[worktree.sessionId];
          const canApply = worktree.available && worktree.hasCommittedChanges && !worktree.dirty && !worktree.conflicts.length && !running;
          return <li className="task-worktree-row" key={worktree.sessionId}>
            <div className="task-worktree-topline"><strong title={worktree.title}>{worktree.title || 'New task'}</strong><span className={`task-worktree-state${worktree.dirty ? ' is-dirty' : worktree.hasCommittedChanges ? ' is-ready' : ''}`}><i />{!worktree.available ? 'Missing' : worktree.conflicts.length ? 'Conflicts' : worktree.dirty ? 'Uncommitted' : worktree.hasCommittedChanges ? 'Ready to apply' : running ? 'Working' : 'Clean'}</span></div>
            <div className="task-worktree-branch"><GitBranch size={12} /><code>{worktree.branch}</code><span>from</span><code>{worktree.baseBranch}</code></div>
            <div className="task-worktree-summary">{worktree.changedFiles.length} {worktree.changedFiles.length === 1 ? 'file' : 'files'}{worktree.additions || worktree.deletions ? <span><b>+{worktree.additions}</b> <i>−{worktree.deletions}</i></span> : null}</div>
            {worktree.conflicts.length > 0 && <div className="task-worktree-conflicts"><CircleAlert size={12} /><span>Resolve conflicts in this task workspace before committing.</span></div>}
            {message?.message && <div className={`task-worktree-feedback${message.conflicts.length ? ' has-conflicts' : ''}`} role={message.conflicts.length ? 'alert' : 'status'}>
              <span>{message.message}</span>{message.conflicts.length > 0 && <ul>{message.conflicts.slice(0, 8).map(path => <li key={path}><code>{path}</code></li>)}</ul>}
            </div>}
            <div className="task-worktree-actions">
              <button type="button" onClick={() => void showDiff(worktree)} disabled={!worktree.available || isBusy}><GitPullRequest size={12} />Diff</button>
              <button type="button" onClick={() => void runAction(worktree, 'commit')} disabled={!worktree.available || !worktree.dirty || !!worktree.conflicts.length || running || !!busyId} title={worktree.dirty ? 'Commit all changes in this isolated task checkout' : 'No uncommitted task changes'}><GitCommitHorizontal size={12} />Commit</button>
              <button type="button" className="is-primary" onClick={() => void runAction(worktree, 'apply')} disabled={!canApply || !!busyId} title={worktree.dirty ? 'Commit task changes before applying them' : 'Merge this task branch into the current project branch'}><GitMerge size={12} />Apply</button>
              <button type="button" onClick={() => void runAction(worktree, 'remove')} disabled={!worktree.available || worktree.dirty || running || !!busyId} title={worktree.dirty ? 'Commit or apply the uncommitted changes before removing this workspace' : 'Remove this workspace and keep its task branch'}>{isBusy && busyId === worktree.sessionId ? <LoaderCircle size={12} className="spin" /> : <Trash2 size={12} />}Remove workspace</button>
            </div>
            {worktree.dirty && <p className="task-worktree-protection"><CircleAlert size={11} />Uncommitted changes are protected. Commit or apply them before removing this checkout.</p>}
            {expanded === worktree.sessionId && <div className="task-worktree-diff">
              <div><strong>Changes from {worktree.baseBranch}</strong><button type="button" aria-label="Copy task diff" disabled={!diff} onClick={() => diff && void navigator.clipboard.writeText(diff)}><Copy size={12} />Copy</button><button type="button" aria-label="Close task diff" onClick={() => setExpanded(null)}><ChevronDown size={12} /></button></div>
              {isBusy && !diff ? <p>Loading diff…</p> : <pre>{diff || 'No changes to show yet.'}</pre>}
            </div>}
          </li>;
        })}</ul>
        : <p className="project-section-empty">Tasks use the project folder by default. Choose “Create isolated workspace” in the composer when you want parallel work on a separate checkout.</p>}
  </section>;
}
