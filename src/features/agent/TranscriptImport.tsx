import { useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { LoaderCircle, Upload } from 'lucide-react';
import { command, desktopAvailable } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { AgentSession } from '../../types/domain';

/**
 * Adds a Markdown or text transcript from another tool to this task as labeled
 * reference context. Its text is never run, and it cannot approve tools or change
 * the repository. The task must not be running, so the import waits for it.
 */
export function TranscriptImport({ session, projectId, onImported }: { session?: AgentSession; projectId?: string; onImported: (session: AgentSession) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  if (!session || !desktopAvailable || !projectId) return null;

  async function importTranscript(current: AgentSession) {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const path = await open({ multiple: false, directory: false, filters: [{ name: 'Transcript', extensions: ['md', 'txt'] }] });
      if (typeof path !== 'string') return;
      onImported(await command('import_task_transcript', { input: { projectId: projectId!, sessionId: current.id, path } }));
      setNotice('Added as reference context, labeled as external. It grants no permissions.');
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setBusy(false);
    }
  }

  return <section className="task-transcript-import" aria-label="Reference transcript">
    <p className="inspector-section-label">REFERENCE</p>
    <div className="task-plan-actions">
      <button type="button" onClick={() => void importTranscript(session)} disabled={busy}>{busy ? <LoaderCircle size={13} className="spin" /> : <Upload size={13} />}<span>Import transcript…</span></button>
    </div>
    <p className="task-plan-meta">Adds a Markdown or text transcript from another tool as reference. It is labeled as external, its text is never run, and it cannot approve tools or change the repository.</p>
    {notice && <p className="task-plan-meta" role="status">{notice}</p>}
    {error && <p className="task-plan-error" role="alert">{error}</p>}
  </section>;
}
