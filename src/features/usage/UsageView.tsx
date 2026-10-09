import { useMemo, useState } from 'react';
import { Activity, AlertCircle, Check, CircleDollarSign, Clock3, Download, Layers3, X } from 'lucide-react';
import type { AgentSession, Project, Provider, UsageRecord } from '../../types/domain';
import { buildUsageDashboard } from './usageStats';

type ExportFormat = 'csv' | 'json';

function formatCost(value: number | null, precise = false) {
  if (value === null) return '—';
  return `$${value.toFixed(precise || value < 1 ? 4 : 2)}`;
}

function formatTokens(value: number) {
  return new Intl.NumberFormat(undefined, { notation: value >= 10_000 ? 'compact' : 'standard', maximumFractionDigits: 1 }).format(value);
}

function csvCell(value: string | number | boolean | null) {
  const text = value === null ? '' : String(value);
  return `"${text.replaceAll('"', '""')}"`;
}

function exportUsage(records: UsageRecord[], format: ExportFormat) {
  const notice = 'Local Agent Usage only. These records cover requests made by JevCode and do not represent complete provider-account usage or provider invoices.';
  const orderedRecords = [...records].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
  const body = format === 'json'
    ? JSON.stringify({ source: 'local_agent_usage', exportedAt: new Date().toISOString(), notice, records: orderedRecords }, null, 2)
    : [
      ['timestamp', 'provider', 'model', 'project_id', 'session_id', 'usage_available', 'input_tokens', 'cached_input_tokens', 'output_tokens', 'estimated_cost_usd', 'latency_ms', 'success', 'failure_code'].map(csvCell).join(','),
      ...orderedRecords.map(record => [record.createdAt, record.providerId, record.modelId, record.projectId, record.sessionId, record.usageAvailable, record.usageAvailable ? record.inputTokens : null, record.usageAvailable ? record.cachedInputTokens : null, record.usageAvailable ? record.outputTokens : null, record.costUsd, record.durationMs, record.success, record.failureCode].map(csvCell).join(',')),
    ].join('\r\n');
  const blob = new Blob([body], { type: format === 'json' ? 'application/json' : 'text/csv;charset=utf-8' });
  const href = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = href;
  anchor.download = `jevcode-local-usage-${new Date().toISOString().slice(0, 10)}.${format}`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(href), 1000);
}

export function UsageView({ records, providers, projects, sessions }: {
  records: UsageRecord[];
  providers: Provider[];
  projects: Project[];
  sessions: AgentSession[];
}) {
  const [days, setDays] = useState<7 | 30>(7);
  const [exportError, setExportError] = useState('');
  const data = useMemo(() => buildUsageDashboard(records, providers, projects, sessions, new Date(), days), [days, projects, providers, records, sessions]);
  const providersById = useMemo(() => new Map(providers.map(provider => [provider.id, provider.name])), [providers]);
  const projectsById = useMemo(() => new Map(projects.map(project => [project.id, project.name])), [projects]);
  const sessionsById = useMemo(() => new Map(sessions.map(session => [session.id, session])), [sessions]);
  const maxTokens = Math.max(1, ...data.daily.map(day => day.inputTokens + day.outputTokens));
  const maxRequests = Math.max(1, ...data.byProvider.map(item => item.requests), ...data.byModel.map(item => item.requests), ...data.byProject.map(item => item.requests));
  const shownRecords = data.records.slice(0, 100);

  function handleExport(format: ExportFormat) {
    try {
      setExportError('');
      exportUsage(records, format);
    } catch {
      setExportError('The usage export could not be created. Try again.');
    }
  }

  return <section className="settings-content usage-dashboard" aria-labelledby="usage-title">
    <header className="usage-heading">
      <div>
        <div className="usage-kicker"><Activity size={12} /> APPLICATION TELEMETRY</div>
        <h1 id="usage-title">Usage</h1>
        <p className="page-description">Local Agent Usage measures requests made by JevCode. Estimated spend is based on token counts and the local model-pricing registry.</p>
      </div>
      <div className="usage-export-actions" aria-label="Export usage">
        <button type="button" onClick={() => handleExport('csv')}><Download size={13} /> CSV</button>
        <button type="button" onClick={() => handleExport('json')}><Download size={13} /> JSON</button>
      </div>
    </header>
    {exportError && <p className="usage-export-error" role="alert"><AlertCircle size={13} />{exportError}</p>}

    <section className="usage-local-banner" aria-label="Local Agent Usage">
      <div className="usage-local-mark"><Activity size={15} /></div>
      <div><strong>Local Agent Usage</strong><p>Tracks this application’s model requests only. It is not a complete OpenAI, Anthropic, Google, or OpenCode account total.</p></div>
      <span className="usage-local-badge"><Check size={11} /> Stored locally</span>
    </section>

    <div className="usage-section-heading">
      <div><span className="usage-overline">TODAY</span><h2>At a glance</h2></div>
      <span className="usage-updated">{data.today.failedRequests ? `${data.today.failedRequests} failed` : 'All requests included'}</span>
    </div>
    <div className="usage-stat-grid">
      <Stat icon={<Activity size={14} />} label="Requests" value={data.today.requests.toLocaleString()} note={`${data.today.failedRequests.toLocaleString()} failed`} />
      <Stat icon={<Layers3 size={14} />} label="Reported tokens" value={formatTokens(data.today.tokens)} note={data.today.unreportedRequests ? `${data.today.unreportedRequests} requests without token data` : 'Input + output'} />
      <Stat icon={<CircleDollarSign size={14} />} label="Estimated spend" value={formatCost(data.today.estimatedCostUsd)} note={data.today.estimatedCostUsd === null ? 'No prices available' : `${data.today.pricedRequests}/${data.today.requests} priced requests`} />
    </div>

    <section className="usage-chart-card" aria-labelledby="usage-chart-title">
      <div className="usage-card-heading">
        <div><span className="usage-overline">LAST {days} DAYS · LOCAL REQUEST TREND</span><h2 id="usage-chart-title">Token usage</h2></div>
        <div className="usage-range-switch" role="group" aria-label="Chart date range">
          <button type="button" aria-pressed={days === 7} onClick={() => setDays(7)}>7 days</button>
          <button type="button" aria-pressed={days === 30} onClick={() => setDays(30)}>30 days</button>
        </div>
      </div>
      <div className="usage-chart-summary"><strong>{formatTokens(data.period.tokens)}</strong><span>reported tokens across {data.period.reportedRequests.toLocaleString()} requests</span><b>{formatCost(data.period.estimatedCostUsd)} <small>{data.period.estimatedCostUsd === null ? 'estimated' : `${data.period.pricedRequests}/${data.period.requests} priced`}</small></b></div>
      {!data.period.requests ? <div className="usage-chart-empty"><Activity size={16} /><span>No local requests in the last {days} days</span></div> : !data.period.reportedRequests ? <div className="usage-chart-empty"><Activity size={16} /><span>The providers did not return token counts for these requests</span></div> : <UsageChart days={data.daily} maxTokens={maxTokens} />}
      <div className="usage-chart-legend"><span><i className="usage-legend-input" /> Input tokens</span><span><i className="usage-legend-output" /> Output tokens</span><small>Local time · daily totals</small></div>
    </section>

    <div className="usage-breakdowns">
      <Breakdown title="By provider" groups={data.byProvider} maxRequests={maxRequests} />
      <Breakdown title="By model" groups={data.byModel} maxRequests={maxRequests} />
      <Breakdown title="By project" groups={data.byProject} maxRequests={maxRequests} />
    </div>

    <section className="usage-account-card">
      <div className="usage-account-heading"><div><span className="usage-overline">SEPARATE SOURCE</span><h2>Provider Account Usage</h2></div><span className="usage-not-imported"><X size={11} /> Not imported</span></div>
      <p>Provider-reported billing is not connected to this dashboard. Check each provider’s account for subscription or account-wide usage; those totals may include requests made outside JevCode.</p>
    </section>

    <section className="usage-history-section">
      <div className="usage-section-heading usage-history-heading"><div><span className="usage-overline">REQUEST LOG</span><h2>Recent requests</h2></div><span className="usage-updated">{data.records.length.toLocaleString()} in selected range</span></div>
      {data.records.length ? <>
        <div className="usage-history-scroll"><table className="usage-history-table">
          <caption className="sr-only">Local Agent Usage request history</caption>
          <thead><tr><th>Provider / model</th><th>Project / task</th><th>Input</th><th>Output</th><th>Latency</th><th>Result</th><th>Estimated cost</th></tr></thead>
          <tbody>{shownRecords.map(record => {
            const provider = providersById.get(record.providerId) ?? record.providerId;
            const session = sessionsById.get(record.sessionId);
            const project = projectsById.get(record.projectId || session?.projectId || '') ?? 'Project unavailable';
            const task = session?.title;
            return <tr key={record.id} className={record.success ? '' : 'is-failed'}>
              <td><strong>{provider}</strong><small>{record.modelId}</small></td>
              <td><strong>{project}</strong><small>{task || record.sessionId.slice(0, 8)} · {new Date(record.createdAt).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}</small></td>
              <td>{record.usageAvailable ? record.inputTokens.toLocaleString() : '—'}{record.usageAvailable && record.cachedInputTokens ? <small>{record.cachedInputTokens.toLocaleString()} cached</small> : null}</td>
              <td>{record.usageAvailable ? record.outputTokens.toLocaleString() : '—'}{!record.usageAvailable && <small>Usage not returned</small>}</td>
              <td>{(record.durationMs / 1000).toFixed(2)} s</td>
              <td><span className={`usage-result ${record.success ? 'is-success' : 'is-failure'}`}>{record.success ? <Check size={11} /> : <X size={11} />}{record.success ? 'Success' : record.failureCode ?? 'Failed'}</span></td>
              <td>{formatCost(record.costUsd, true)}</td>
            </tr>;
          })}</tbody>
        </table></div>
        {data.records.length > shownRecords.length && <p className="usage-history-more">Showing the latest {shownRecords.length} requests. Export to download the complete local record.</p>}
      </> : <div className="usage-history-empty"><Clock3 size={15} /><span>Requests will appear here as tasks use a model.</span></div>}
    </section>
    <p className="usage-disclaimer">Estimates use normalized USD per million token rates where available. Cached-input rates are applied when reported. Provider billing, tiers, free quotas, tools, taxes, and discounts may differ.</p>
  </section>;
}

function Stat({ icon, label, value, note }: { icon: React.ReactNode; label: string; value: string; note: string }) {
  return <article className="usage-stat-card"><span className="usage-stat-icon">{icon}</span><span className="usage-stat-label">{label}</span><strong>{value}</strong><small>{note}</small></article>;
}

function UsageChart({ days, maxTokens }: { days: ReturnType<typeof buildUsageDashboard>['daily']; maxTokens: number }) {
  const width = 700;
  const height = 180;
  const left = 26;
  const right = 8;
  const top = 12;
  const bottom = 29;
  const chartWidth = width - left - right;
  const chartHeight = height - top - bottom;
  const column = chartWidth / days.length;
  const barWidth = Math.max(4, Math.min(18, column * 0.48));
  const labelsEvery = days.length <= 7 ? 1 : 5;
  return <div className="usage-chart-plot">
    <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`${days.length}-day local input and output token usage chart`}>
      {[0, 0.5, 1].map(fraction => {
        const y = top + chartHeight * fraction;
        return <g key={fraction}><line x1={left} x2={width - right} y1={y} y2={y} className="usage-chart-gridline" /><text x="0" y={y + 3} className="usage-chart-axis">{formatTokens(Math.round(maxTokens * (1 - fraction)))}</text></g>;
      })}
      {days.map((day, index) => {
        const inputHeight = (day.inputTokens / maxTokens) * chartHeight;
        const outputHeight = (day.outputTokens / maxTokens) * chartHeight;
        const x = left + column * index + (column - barWidth) / 2;
        const yInput = top + chartHeight - inputHeight;
        const yOutput = yInput - outputHeight;
        return <g key={day.key}>
          <title>{`${day.label}: ${day.inputTokens.toLocaleString()} input, ${day.outputTokens.toLocaleString()} output, ${day.requests} requests`}</title>
          {day.inputTokens > 0 && <rect x={x} y={yInput} width={barWidth} height={Math.max(1, inputHeight)} rx="2" className="usage-chart-input" />}
          {day.outputTokens > 0 && <rect x={x} y={yOutput} width={barWidth} height={Math.max(1, outputHeight)} rx="2" className="usage-chart-output" />}
          {(index % labelsEvery === 0 || index === days.length - 1) && <text x={x + barWidth / 2} y={height - 7} textAnchor="middle" className="usage-chart-day">{day.label}</text>}
        </g>;
      })}
    </svg>
  </div>;
}

function Breakdown({ title, groups, maxRequests }: { title: string; groups: ReturnType<typeof buildUsageDashboard>['byProvider']; maxRequests: number }) {
  return <section className="usage-breakdown-card" aria-label={title}>
    <div className="usage-breakdown-heading"><h2>{title}</h2><span>{groups.reduce((sum, item) => sum + item.requests, 0)} requests</span></div>
    {groups.length ? <ul>{groups.map(group => <li key={group.key}>
      <div className="usage-breakdown-row"><div><strong>{group.label}</strong><small>{group.detail} · {group.requests} requests{group.failedRequests ? ` · ${group.failedRequests} failed` : ''}{group.unreportedRequests ? ` · ${group.unreportedRequests} unreported` : ''}</small></div><div className="usage-breakdown-metrics"><b>{formatCost(group.estimatedCostUsd, true)}</b><small>{formatTokens(group.inputTokens + group.outputTokens)} reported tokens</small></div></div>
      <div className="usage-breakdown-track" aria-hidden="true"><i style={{ width: `${Math.max(group.requests ? 3 : 0, group.requests / maxRequests * 100)}%` }} /></div>
    </li>)}</ul> : <p className="usage-breakdown-empty">No usage in this range.</p>}
  </section>;
}
