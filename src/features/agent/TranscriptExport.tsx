import { useState } from 'react';
import { Download } from 'lucide-react';
import { save } from '@tauri-apps/plugin-dialog';
import { command, desktopAvailable } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { AgentSession } from '../../types/domain';

/**
 * Saves a task as a Markdown transcript: the conversation, tool names, the activity log,
 * and the review diff. Tool outputs, tool arguments, and lines that look like credentials
 * are left out. That check is heuristic, so the file should still be reviewed before sharing.
 */
export function TranscriptExport({ session, projectId }: { session?: AgentSession; projectId?: string }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!session || !desktopAvailable || !projectId) return null;

  async function exportTranscript(current: AgentSession) {
    setBusy(true);
    setError(null);
    try {
      const fileName = current.title.replace(/[\\/:*?"<>|]/g, '-');
      const path = await save({ defaultPath: `${fileName}.md`, filters: [{ name: 'Markdown', extensions: ['md'] }] });
      if (path) await command('export_task_transcript', { input: { projectId: projectId!, sessionId: current.id, path } });
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setBusy(false);
    }
  }

  return <section className="task-transcript" aria-label="Task transcript">
    <p className="inspector-section-label">TRANSCRIPT</p>
    <div className="task-plan-actions">
      <button type="button" onClick={() => void exportTranscript(session)} disabled={busy}><Download size={13} /><span>Export Markdown</span></button>
    </div>
    <p className="task-plan-meta">Keeps messages, tool names, the activity log, and the diff. Tool outputs, tool arguments, and lines that look like credentials are left out, but review the file before sharing it.</p>
    {error && <p className="task-plan-error" role="alert">{error}</p>}
  </section>;
}
