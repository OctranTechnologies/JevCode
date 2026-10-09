import { describe, expect, it } from 'vitest';
import type { UsageRecord } from '../src/types/domain';
import { buildUsageDashboard } from '../src/features/usage/usageStats';

function usage(id: string, date: Date, values: Partial<UsageRecord> = {}): UsageRecord {
  return {
    id,
    sessionId: `session-${id}`,
    projectId: 'project-a',
    providerId: 'openai',
    modelId: 'gpt-5.4-mini',
    inputTokens: 100,
    usageAvailable: true,
    cachedInputTokens: null,
    outputTokens: 40,
    costUsd: 0.001,
    durationMs: 900,
    success: true,
    failureCode: null,
    createdAt: date.toISOString(),
    ...values,
  };
}

describe('usage dashboard aggregation', () => {
  it('separates local today totals from the selected date window', () => {
    const now = new Date(2026, 9, 10, 13, 0, 0);
    const yesterday = new Date(2026, 9, 9, 13, 0, 0);
    const lastWeek = new Date(2026, 9, 4, 13, 0, 0);
    const old = new Date(2026, 8, 30, 13, 0, 0);
    const result = buildUsageDashboard([
      usage('today', now),
      usage('yesterday', yesterday, { success: false, failureCode: 'network_error', costUsd: null }),
      usage('week', lastWeek),
      usage('old', old),
    ], [], [], [], now, 7);

    expect(result.today.requests).toBe(1);
    expect(result.today.tokens).toBe(140);
    expect(result.today.estimatedCostUsd).toBe(0.001);
    expect(result.period.requests).toBe(3);
    expect(result.period.tokens).toBe(420);
    expect(result.daily).toHaveLength(7);
    expect(result.daily.reduce((sum, day) => sum + day.requests, 0)).toBe(3);
  });

  it('keeps failed attempts in provider, model, and project breakdowns', () => {
    const now = new Date(2026, 9, 10, 13, 0, 0);
    const result = buildUsageDashboard([
      usage('failed', now, { success: false, failureCode: 'provider_outage', costUsd: null, projectId: 'project-b', cachedInputTokens: 25 }),
    ], [], [], [], now, 30);

    expect(result.today.failedRequests).toBe(1);
    expect(result.byProvider[0]).toMatchObject({ requests: 1, failedRequests: 1, estimatedCostUsd: null });
    expect(result.byModel[0].key).toBe('openai/gpt-5.4-mini');
    expect(result.byProject[0].key).toBe('project-b');
    expect(result.byProvider[0].cachedInputTokens).toBe(25);
  });

  it('does not treat omitted token metadata as zero usage', () => {
    const now = new Date(2026, 9, 10, 13, 0, 0);
    const result = buildUsageDashboard([
      usage('unknown-usage', now, { usageAvailable: false, inputTokens: 0, outputTokens: 0, costUsd: null }),
    ], [], [], [], now, 7);

    expect(result.today.requests).toBe(1);
    expect(result.today.tokens).toBe(0);
    expect(result.today.reportedRequests).toBe(0);
    expect(result.today.unreportedRequests).toBe(1);
    expect(result.period.estimatedCostUsd).toBeNull();
    expect(result.daily.reduce((sum, day) => sum + day.requests, 0)).toBe(1);
  });
});
