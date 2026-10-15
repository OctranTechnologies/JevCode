import { describe, expect, it } from 'vitest';
import { desktopReducer } from '../src/app/store';
import type { UsageRecord } from '../src/types/domain';

const record: UsageRecord = {
  id: 'usage-1',
  sessionId: 'task-1',
  sessionTitle: 'Earlier coding task',
  projectId: 'project-1',
  providerId: 'openai',
  modelId: 'gpt-5.4-mini',
  inputTokens: 40,
  usageAvailable: true,
  cachedInputTokens: null,
  outputTokens: 18,
  costUsd: 0.001,
  durationMs: 120,
  success: true,
  failureCode: null,
  createdAt: '2026-08-01T12:00:00.000Z',
};

describe('usage retention', () => {
  it('keeps all-time request history when a task is removed from the sidebar', () => {
    const state = { data: null, sessions: [], usage: [record] };

    const next = desktopReducer(state, { type: 'delete-session', sessionId: 'task-1' });

    expect(next.usage).toEqual([record]);
  });
});
