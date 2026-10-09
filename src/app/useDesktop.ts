import { useEffect, useReducer, useState } from 'react';
import { desktopReducer } from './store';
import { loadBootstrap, subscribeEvents } from '../lib/ipc';
import { normalizeError } from '../lib/errors';
import { logEvent } from '../lib/logger';

export function useDesktop() {
  const [state, dispatch] = useReducer(desktopReducer, { data: null, sessions: [], usage: [] });
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [reload, setReload] = useState(0);
  const [streaming, setStreaming] = useState<Record<string, string>>({});
  const [toolOutput, setToolOutput] = useState<Record<string, { stdout: string; stderr: string }>>({});
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    setLoading(true);
    setError(null);
    void (async () => {
      try {
        const cleanup = await subscribeEvents(
          session => {
            dispatch({ type: 'session', session });
            if (['waiting_for_permission', 'waiting_for_user', 'completed', 'failed', 'cancelled'].includes(session.status)) {
              setStreaming(current => { const next = { ...current }; delete next[session.id]; return next; });
            }
          },
          record => dispatch({ type: 'usage', record }),
          () => { logEvent('ipc_event_invalid', 'error'); setError('An update could not be read. Reload JevCode.'); },
          chunk => setStreaming(current => ({ ...current, [chunk.sessionId]: chunk.reset ? '' : `${current[chunk.sessionId] ?? ''}${chunk.delta}` })),
          chunk => setToolOutput(current => {
            const previous = current[chunk.toolCallId] ?? { stdout: '', stderr: '' };
            const key = chunk.stream;
            const next = `${previous[key]}${chunk.chunk}`;
            const bounded = next.length > 32_000 ? `[earlier output truncated]\n${next.slice(-31_970)}` : next;
            return { ...current, [chunk.toolCallId]: { ...previous, [key]: bounded } };
          }),
          account => dispatch({ type: 'account', account }),
        );
        if (disposed) { cleanup(); return; }
        unlisten = cleanup;
        const data = await loadBootstrap();
        if (!disposed) dispatch({ type: 'bootstrap', data });
      } catch (error) { if (!disposed) { setError(normalizeError(error).message); logEvent('bootstrap_failed', 'error'); } }
      finally { if (!disposed) setLoading(false); }
    })();
    return () => { disposed = true; unlisten?.(); };
  }, [reload]);
  return { ...state, dispatch, streaming, toolOutput, loading, error, setError, retry: () => setReload(value => value + 1) };
}
