import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { sessionSchema } from '../src/lib/schemas';
import type { AgentSession } from '../src/types/domain';
import { TaskInbox } from '../src/features/workspaces/TaskInbox';
import { groupInboxSessions } from '../src/features/workspaces/taskInboxData';

const fixture = sessionSchema.parse(JSON.parse(readFileSync(new URL('./fixtures/session.json', import.meta.url), 'utf8')) as unknown);

function task(overrides: Partial<AgentSession>): AgentSession {
  return { ...fixture, queuedFollowUps: [], ...overrides };
}

describe('task inbox', () => {
  it('groups active sessions across projects and sorts pinned tasks first', () => {
    const groups = groupInboxSessions([
      task({ id: 'waiting', projectId: 'one', status: 'waiting_for_user', updatedAt: '2026-01-02T00:00:00Z' }),
      task({ id: 'working', projectId: 'two', status: 'working', updatedAt: '2026-01-03T00:00:00Z' }),
      task({ id: 'pinned', projectId: 'three', status: 'working', pinned: true, updatedAt: '2026-01-01T00:00:00Z' }),
      task({ id: 'archived', status: 'failed', archivedAt: '2026-01-01T00:00:00Z' }),
    ]);

    expect(groups.find(group => group.id === 'attention')?.sessions.map(item => item.id)).toEqual(['waiting']);
    expect(groups.find(group => group.id === 'working')?.sessions.map(item => item.id)).toEqual(['pinned', 'working']);
    expect(groups.flatMap(group => group.sessions).some(item => item.id === 'archived')).toBe(false);
  });

  it('renders task counts, project labels, pin actions, and the empty state', () => {
    const onNewTask = vi.fn();
    const taskView = renderToStaticMarkup(<TaskInbox sessions={[task({ id: 'one', status: 'working', pinned: true })]} projects={[]} providers={[]} onSelectSession={vi.fn()} onTogglePin={vi.fn()} onNewTask={onNewTask} />);
    const emptyView = renderToStaticMarkup(<TaskInbox sessions={[]} projects={[]} providers={[]} onSelectSession={vi.fn()} onTogglePin={vi.fn()} onNewTask={onNewTask} />);

    expect(taskView).toContain('Task inbox');
    expect(taskView).toContain('In progress');
    expect(taskView).toContain('Pin');
    expect(taskView).toContain('Unknown project');
    expect(emptyView).toContain('No tasks yet');
    expect(emptyView).toContain('Start a task');
  });

  it('shows the parent relationship and delegated task count in the inbox', () => {
    const parent = task({ id: 'parent', title: 'Build account settings', status: 'completed' });
    const child = task({ id: 'child', title: 'Add the settings form', status: 'working', parentSessionId: 'parent' });
    const html = renderToStaticMarkup(<TaskInbox sessions={[parent, child]} projects={[]} providers={[]} onSelectSession={vi.fn()} onTogglePin={vi.fn()} onNewTask={vi.fn()} />);

    expect(html).toContain('Child of Build account settings');
    expect(html).toContain('1 delegated task');
  });
});
