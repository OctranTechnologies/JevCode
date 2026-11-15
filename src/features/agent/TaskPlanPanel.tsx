import { useCallback, useEffect, useState } from 'react';
import { save } from '@tauri-apps/plugin-dialog';
import { command, desktopAvailable } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { AgentSession, TaskPlan } from '../../types/domain';

/**
 * A plan is the answer a Plan task gave, kept as its own artifact. Starting an
 * Agent task from it creates a new task with fresh permissions; the plan itself
 * grants nothing.
 */
export function TaskPlanPanel({ session, projectId, onStart }: { session?: AgentSession; projectId?: string; onStart: (plan: TaskPlan) => void }) {
  const [plans, setPlans] = useState<TaskPlan[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const sessionId = session?.id;

  const load = useCallback(async () => {
    if (!desktopAvailable || !projectId) return;
    try {
      setPlans(await command('list_task_plans', { projectId }));
    } catch (cause) {
      setError(normalizeError(cause).message);
    }
  }, [projectId]);

  useEffect(() => { void load(); }, [load, sessionId]);

  if (!session || !desktopAvailable || !projectId) return null;
  const plan = plans.find(item => item.sourceSessionId === session.id);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setBusy(false);
    }
  }

  async function savePlan() {
    await run(async () => {
      await command('create_task_plan', { sessionId: session!.id });
      await load();
    });
  }

  async function saveRevision(markdown: string) {
    if (!plan) return;
    await run(async () => {
      await command('update_task_plan', { input: { projectId: projectId!, planId: plan.id, markdown } });
      setDraft(null);
      await load();
    });
  }

  async function exportPlan(current: TaskPlan) {
    await run(async () => {
      const path = await save({ defaultPath: `${current.title}.md`, filters: [{ name: 'Markdown', extensions: ['md'] }] });
      if (!path) return;
      await command('export_task_plan', { input: { projectId: projectId!, planId: current.id, path } });
    });
  }

  return <section className="task-plan" aria-label="Task plan">
    <p className="inspector-section-label">PLAN</p>
    {!plan && session.taskMode === 'plan' && <div className="task-plan-empty">
      <p>Save this answer as a plan to review it, edit it, and start an Agent task from it.</p>
      <button type="button" onClick={() => void savePlan()} disabled={busy || session.status !== 'completed'}>Save as plan</button>
      {session.status !== 'completed' && <small>Available when this Plan task finishes.</small>}
    </div>}
    {!plan && session.taskMode !== 'plan' && <p className="task-plan-empty">No plan from this task.</p>}
    {plan && draft === null && <>
      <p className="task-plan-meta">{plan.title} · version {plan.version}</p>
      <pre className="task-plan-text">{plan.markdown}</pre>
      <div className="task-plan-actions">
        <button type="button" onClick={() => setDraft(plan.markdown)} disabled={busy}>Edit</button>
        <button type="button" onClick={() => void exportPlan(plan)} disabled={busy}>Export Markdown</button>
        <button type="button" className="button-primary" onClick={() => void run(async () => onStart(plan))} disabled={busy} title="Creates a new Agent task. The plan grants no permissions; the new task asks for each one as usual.">Start Agent task from plan</button>
      </div>
      {plan.history.length > 0 && <details className="task-plan-history">
        <summary>Earlier versions ({plan.history.length})</summary>
        {plan.history.slice().reverse().map(version => <div key={version.version}>
          <small>Version {version.version} · saved {new Date(version.savedAt).toLocaleString()}</small>
          <pre className="task-plan-text">{version.markdown}</pre>
        </div>)}
      </details>}
    </>}
    {plan && draft !== null && <form className="task-plan-editor" onSubmit={event => { event.preventDefault(); void saveRevision(draft); }}>
      <label htmlFor="task-plan-draft">Edit the plan</label>
      <textarea id="task-plan-draft" value={draft} onChange={event => setDraft(event.target.value)} rows={12} maxLength={60_000} />
      <div className="task-plan-actions">
        <button type="button" onClick={() => setDraft(null)} disabled={busy}>Cancel</button>
        <button type="submit" className="button-primary" disabled={busy || !draft.trim() || draft.trim() === plan.markdown}>Save revision</button>
      </div>
    </form>}
    {error && <p className="task-plan-error" role="alert">{error}</p>}
  </section>;
}
