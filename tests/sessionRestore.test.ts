import { describe, expect, it } from 'vitest';
import type { AgentSession, Project } from '../src/types/domain';
import { latestRestorableSession } from '../src/app/restore';

function session(id: string, projectId: string, updatedAt: string, archivedAt: string | null = null): AgentSession {
  return { id, projectId, updatedAt, archivedAt } as AgentSession;
}

function project(id: string): Project { return { id } as Project; }

describe('automatic task restoration', () => {
  it('restores the most recently updated task belonging to an available project', () => {
    const tasks = [session('old', 'project', '2026-10-01T10:00:00Z'), session('latest', 'project', '2026-10-02T10:00:00Z')];
    expect(latestRestorableSession(tasks, [project('project')])?.id).toBe('latest');
  });

  it('skips archived tasks and tasks whose project is unavailable', () => {
    const tasks = [
      session('archived', 'project', '2026-10-03T10:00:00Z', '2026-10-04T10:00:00Z'),
      session('missing-project', 'gone', '2026-10-02T10:00:00Z'),
      session('usable', 'project', '2026-10-01T10:00:00Z'),
    ];
    expect(latestRestorableSession(tasks, [project('project')])?.id).toBe('usable');
  });

  it('returns no selection when the user has no task to resume', () => {
    expect(latestRestorableSession([], [project('project')])).toBeUndefined();
  });
});
