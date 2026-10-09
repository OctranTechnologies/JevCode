import type { AgentSession, Project, Provider, UsageRecord } from '../../types/domain';

export interface UsageGroup {
  key: string;
  label: string;
  detail: string;
  requests: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reportedRequests: number;
  estimatedCostUsd: number | null;
  pricedRequests: number;
  unreportedRequests: number;
  failedRequests: number;
}

export interface UsageDay {
  key: string;
  label: string;
  inputTokens: number;
  outputTokens: number;
  estimatedCostUsd: number;
  requests: number;
}

export function localDayKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function sumCost(records: UsageRecord[]): number | null {
  const priced = records.filter(record => record.costUsd !== null);
  return priced.length ? priced.reduce((sum, record) => sum + (record.costUsd ?? 0), 0) : null;
}

function groupBy(
  records: UsageRecord[],
  keyFor: (record: UsageRecord) => string,
  labelFor: (key: string) => { label: string; detail: string },
  includeKeys: string[] = [],
): UsageGroup[] {
  const grouped = new Map<string, UsageRecord[]>();
  for (const key of includeKeys) grouped.set(key, []);
  for (const record of records) {
    const key = keyFor(record);
    const items = grouped.get(key);
    if (items) items.push(record);
    else grouped.set(key, [record]);
  }
  return [...grouped.entries()].map(([key, items]) => {
    const { label, detail } = labelFor(key);
    return {
      key,
      label,
      detail,
      requests: items.length,
      inputTokens: items.reduce((sum, item) => sum + (item.usageAvailable ? item.inputTokens : 0), 0),
      cachedInputTokens: items.reduce((sum, item) => sum + (item.usageAvailable ? item.cachedInputTokens ?? 0 : 0), 0),
      outputTokens: items.reduce((sum, item) => sum + (item.usageAvailable ? item.outputTokens : 0), 0),
      estimatedCostUsd: sumCost(items),
      pricedRequests: items.filter(item => item.costUsd !== null).length,
      unreportedRequests: items.filter(item => !item.usageAvailable).length,
      reportedRequests: items.filter(item => item.usageAvailable).length,
      failedRequests: items.filter(item => !item.success).length,
    };
  }).sort((a, b) => b.requests - a.requests || a.label.localeCompare(b.label));
}

export function buildUsageDashboard(
  records: UsageRecord[],
  providers: Provider[],
  projects: Project[],
  sessions: AgentSession[],
  now = new Date(),
  days = 7,
) {
  const todayKey = localDayKey(now);
  const today = records.filter(record => localDayKey(new Date(record.createdAt)) === todayKey);
  const firstDay = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  firstDay.setDate(firstDay.getDate() - (days - 1));
  const startKey = localDayKey(firstDay);
  const period = records.filter(record => localDayKey(new Date(record.createdAt)) >= startKey && localDayKey(new Date(record.createdAt)) <= todayKey);
  const providerNames = new Map(providers.map(provider => [provider.id, provider.name]));
  const models = new Map<string, string>(providers.flatMap(provider => provider.models.map(model => [`${provider.id}/${model.id}`, model.displayName] as [string, string])));
  const projectsById = new Map(projects.map(project => [project.id, project.name]));
  const sessionsById = new Map(sessions.map(session => [session.id, session]));
  const effectiveProject = (record: UsageRecord) => record.projectId || sessionsById.get(record.sessionId)?.projectId || '';
  const keys: string[] = [];
  for (let offset = days - 1; offset >= 0; offset -= 1) {
    const date = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    date.setDate(date.getDate() - offset);
    keys.push(localDayKey(date));
  }
  const periodKeys = new Set(keys);
  const byDay = new Map<string, UsageRecord[]>();
  for (const record of period) {
    const key = localDayKey(new Date(record.createdAt));
    if (periodKeys.has(key)) {
      const items = byDay.get(key);
      if (items) items.push(record);
      else byDay.set(key, [record]);
    }
  }
  const daily: UsageDay[] = keys.map(key => {
    const items = byDay.get(key) ?? [];
    const date = new Date(`${key}T12:00:00`);
    return {
      key,
      label: date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }),
      inputTokens: items.reduce((sum, item) => sum + (item.usageAvailable ? item.inputTokens : 0), 0),
      outputTokens: items.reduce((sum, item) => sum + (item.usageAvailable ? item.outputTokens : 0), 0),
      estimatedCostUsd: items.reduce((sum, item) => sum + (item.costUsd ?? 0), 0),
      requests: items.length,
    };
  });
  const providersWithUsage = [...new Set([...providers.map(provider => provider.id), ...period.map(record => record.providerId)])];
  const byProvider = groupBy(period, record => record.providerId, key => ({ label: providerNames.get(key) ?? key, detail: 'Provider' }), providersWithUsage);
  const byModel = groupBy(
    period,
    record => `${record.providerId}/${record.modelId}`,
    key => ({ label: models.get(key) ?? key.split('/').slice(1).join('/'), detail: key.split('/')[0] }),
  );
  const byProject = groupBy(
    period,
    record => effectiveProject(record) || 'unknown',
    key => ({ label: projectsById.get(key) ?? (key === 'unknown' ? 'Project unavailable' : key), detail: key === 'unknown' ? 'Project' : 'Workspace' }),
  );
  return {
    today: {
      requests: today.length,
      tokens: today.reduce((sum, item) => sum + (item.usageAvailable ? item.inputTokens + item.outputTokens : 0), 0),
      reportedRequests: today.filter(item => item.usageAvailable).length,
      unreportedRequests: today.filter(item => !item.usageAvailable).length,
      estimatedCostUsd: sumCost(today),
      pricedRequests: today.filter(item => item.costUsd !== null).length,
      failedRequests: today.filter(item => !item.success).length,
    },
    period: {
      requests: period.length,
      tokens: period.reduce((sum, item) => sum + (item.usageAvailable ? item.inputTokens + item.outputTokens : 0), 0),
      reportedRequests: period.filter(item => item.usageAvailable).length,
      estimatedCostUsd: sumCost(period),
      pricedRequests: period.filter(item => item.costUsd !== null).length,
    },
    daily,
    byProvider,
    byModel,
    byProject,
    records: [...period].sort((a, b) => b.createdAt.localeCompare(a.createdAt)),
  };
}
