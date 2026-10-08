import type { Provider, UsageRecord } from '../../types/domain';
export function UsageView({ records, providers }: { records: UsageRecord[]; providers: Provider[] }) {
  const input = records.reduce((sum, record) => sum + record.inputTokens, 0);
  const output = records.reduce((sum, record) => sum + record.outputTokens, 0);
  return <section className="settings-content"><h1>Usage</h1><p className="page-description">Token counts reported by your providers, saved locally.</p><dl className="usage-totals"><div><dt>Input tokens</dt><dd>{input.toLocaleString()}</dd></div><div><dt>Output tokens</dt><dd>{output.toLocaleString()}</dd></div><div><dt>Requests</dt><dd>{records.length.toLocaleString()}</dd></div></dl>
    {records.length ? <div className="table-scroll"><table><caption className="sr-only">Provider request history</caption><thead><tr><th>Provider / model</th><th>Input</th><th>Output</th><th>Duration</th><th>Cost</th></tr></thead><tbody>{[...records].reverse().map(record => <tr key={record.id}><td>{providers.find(provider => provider.id === record.providerId)?.name ?? record.providerId}<small>{record.modelId}</small></td><td>{record.inputTokens.toLocaleString()}</td><td>{record.outputTokens.toLocaleString()}</td><td>{(record.durationMs / 1000).toFixed(2)} s</td><td>{record.costUsd === null ? 'Not available' : `$${record.costUsd.toFixed(4)}`}</td></tr>)}</tbody></table></div> : <div className="empty-usage"><h2>No requests yet</h2><p>Start a session to see usage here. Local preview requests report zero tokens.</p></div>}
    <p className="privacy-note">Cost is unavailable until a pricing source is configured. Counts exclude failed requests and may differ from provider billing.</p>
  </section>;
}
