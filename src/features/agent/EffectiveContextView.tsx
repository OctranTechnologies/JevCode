import { useCallback, useEffect, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { command, desktopAvailable } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { EffectiveContext, EffectiveContextSource } from '../../types/domain';

const scopeLabel: Record<EffectiveContextSource['scope'], string> = {
  settings: 'Project settings',
  ancestor: 'Repository',
  folder: 'This folder',
  nested: 'Folder guidance',
  scoped: 'Path rule',
};

const statusLabel: Record<EffectiveContextSource['status'], string> = {
  included: 'Included',
  pending: 'Applies on match',
  skipped: 'Skipped',
};

const changeLabel: Record<NonNullable<EffectiveContextSource['change']>, string> = {
  unchanged: 'Unchanged',
  changed: 'Changed',
  removed: 'Removed',
  added: 'Added',
};

/** Shows the guidance a task carries, or would carry, with its budget and what changed. */
export function EffectiveContextView({ projectId, sessionId }: { projectId?: string; sessionId: string | null }) {
  const [view, setView] = useState<EffectiveContext | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!desktopAvailable || !projectId) return;
    setBusy(true);
    try {
      setView(await command('effective_context', { input: { projectId, sessionId } }));
      setError(null);
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setBusy(false);
    }
  }, [projectId, sessionId]);

  useEffect(() => { void load(); }, [load]);

  if (!desktopAvailable) return <p className="effective-context-empty">Effective context is available in the desktop app.</p>;
  if (!projectId) return <p className="effective-context-empty">Open a project to see the guidance a task uses.</p>;

  return <section className="effective-context" aria-label="Effective context">
    <div className="effective-context-heading">
      <span>{view?.basis === 'task' ? 'Captured when this task started' : 'What a new task would send now'}</span>
      <button type="button" onClick={() => void load()} disabled={busy} aria-label="Refresh effective context" title="Refresh">
        <RefreshCw size={12} className={busy ? 'spin' : undefined} />
      </button>
    </div>
    {error && <p className="effective-context-error" role="alert">{error}</p>}
    {view && <>
      <p className="effective-context-budget">
        <span>{formatBytes(view.usedBytes)} of {formatBytes(view.budgetBytes)}</span>
        <span>≈ {Math.ceil(view.usedBytes / 4).toLocaleString()} tokens</span>
      </p>
      {view.sources.length === 0
        ? <p className="effective-context-empty">No guidance files or settings apply to this folder.</p>
        : <ul className="effective-context-list">{view.sources.map((item, index) => <SourceRow key={`${item.scope}:${item.path}:${index}`} item={item} />)}</ul>}
    </>}
  </section>;
}

export function SourceRow({ item }: { item: EffectiveContextSource }) {
  return <li className={`effective-context-source is-${item.status}`}>
    <div className="effective-context-source-head">
      <span className="effective-context-status">{statusLabel[item.status]}</span>
      <code title={item.path}>{item.path}</code>
    </div>
    <p className="effective-context-meta">
      <span>{scopeLabel[item.scope]}</span>
      <span>{formatBytes(item.bytes)}</span>
      {item.globs.length > 0 && <span title="Applies to paths matching">applies to {item.globs.join(', ')}</span>}
      {item.change && <span className={`effective-context-change is-${item.change}`}>{changeLabel[item.change]}</span>}
    </p>
    <p className="effective-context-summary">{item.summary}</p>
    {item.preview && <details>
      <summary>Preview</summary>
      <pre>{item.preview}</pre>
    </details>}
  </li>;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toLocaleString(undefined, { maximumFractionDigits: 1 })} KiB`;
}
