import { useCallback, useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { AlertTriangle, Bookmark, FileText, LoaderCircle, Pencil, Plus, RefreshCw, Save, Trash2, X } from 'lucide-react';
import { command } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { ProjectMemoryCategory, ProjectMemorySourceStatus, ProjectMemoryView } from '../../types/domain';

type Props = { projectId: string; desktopAllowed: boolean };
type Draft = { id: string | null; title: string; category: ProjectMemoryCategory; content: string; sourcePath: string; pinned: boolean };

const categories: Array<{ value: ProjectMemoryCategory; label: string }> = [
  { value: 'convention', label: 'Convention' },
  { value: 'architecture', label: 'Architecture' },
  { value: 'workflow', label: 'Workflow' },
  { value: 'testing', label: 'Testing' },
  { value: 'decision', label: 'Decision' },
  { value: 'other', label: 'Other' },
];

const emptyDraft = (): Draft => ({ id: null, title: '', category: 'convention', content: '', sourcePath: '', pinned: false });

const statusLabel: Record<ProjectMemorySourceStatus, string> = {
  current: 'Source unchanged',
  stale: 'Source changed',
  missing: 'Source missing',
  unverified: 'No source check',
  unavailable: 'Source unavailable',
};

export function ProjectMemoryPanel({ projectId, desktopAllowed }: Props) {
  const [memories, setMemories] = useState<ProjectMemoryView[]>([]);
  const [loading, setLoading] = useState(desktopAllowed);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!desktopAllowed) return;
    setLoading(true);
    setError(null);
    try { setMemories(await command('list_project_memories', { projectId })); }
    catch (cause) { setError(normalizeError(cause).message); }
    finally { setLoading(false); }
  }, [desktopAllowed, projectId]);

  useEffect(() => { void load(); }, [load]);

  function edit(memory?: ProjectMemoryView) {
    setError(null);
    setDraft(memory ? {
      id: memory.id,
      title: memory.title,
      category: memory.category,
      content: memory.content,
      sourcePath: memory.sourcePath ?? '',
      pinned: memory.pinned,
    } : emptyDraft());
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!draft || !desktopAllowed) return;
    setBusyId(draft.id ?? 'new');
    setError(null);
    try {
      await command('save_project_memory', { input: {
        projectId,
        id: draft.id,
        title: draft.title,
        category: draft.category,
        content: draft.content,
        sourcePath: draft.sourcePath.trim() || null,
        pinned: draft.pinned,
      } });
      setDraft(null);
      await load();
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusyId(null); }
  }

  async function togglePinned(memory: ProjectMemoryView) {
    setBusyId(memory.id);
    setError(null);
    try {
      await command('save_project_memory', { input: {
        projectId,
        id: memory.id,
        title: memory.title,
        category: memory.category,
        content: memory.content,
        sourcePath: memory.sourcePath,
        pinned: !memory.pinned,
      } });
      await load();
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusyId(null); }
  }

  async function refreshSource(memory: ProjectMemoryView) {
    setBusyId(memory.id);
    setError(null);
    try {
      await command('refresh_project_memory_source', { projectId, memoryId: memory.id });
      await load();
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusyId(null); }
  }

  async function remove(memory: ProjectMemoryView) {
    setBusyId(memory.id);
    setError(null);
    try {
      await command('delete_project_memory', { projectId, memoryId: memory.id });
      setMemories(current => current.filter(item => item.id !== memory.id));
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusyId(null); }
  }

  return <section className="project-memory-section" aria-labelledby="project-memory-title">
    <div className="project-memory-heading">
      <div><h2 id="project-memory-title">Project memory</h2><p>Short, source-checked notes saved on this device.</p></div>
      <button className="project-save-button" onClick={() => edit()} disabled={!desktopAllowed || Boolean(draft) || memories.length >= 100}>
        <Plus size={13} />Add note
      </button>
    </div>

    <div className="project-memory-notice"><AlertTriangle size={14} /><p><strong>Local only in this stage.</strong> Notes are not sent to Jev or coding providers yet. Pinning keeps a note at the top of this list; it does not add it to task context. “Source unchanged” means only that the file matches its saved fingerprint; it does not verify the note's accuracy. Changed or missing sources are marked for review. Avoid secrets, credentials, raw tool output, and large code blocks.</p></div>

    {!desktopAllowed ? <p className="project-memory-empty">Memory management is available in the desktop application.</p>
      : loading ? <p className="project-memory-empty"><LoaderCircle size={13} className="spin" />Checking saved notes…</p>
        : memories.length ? <div className="project-memory-list">{memories.map(memory => <article className="project-memory-card" key={memory.id}>
          <div className="project-memory-card-heading">
            <div className="project-memory-title"><FileText size={14} /><div><h3>{memory.title}</h3><span>{categories.find(item => item.value === memory.category)?.label ?? 'Other'}{memory.pinned ? ' · Pinned locally' : ''}</span></div></div>
            <span className={`project-memory-status is-${memory.sourceStatus}`}>{statusLabel[memory.sourceStatus]}</span>
          </div>
          <p className="project-memory-content">{memory.content}</p>
          <div className="project-memory-source">{memory.sourcePath ? <><code>{memory.sourcePath}</code><span>Checked {formatDate(memory.sourceCheckedAt)}</span></> : <span>No source file attached</span>}</div>
          <div className="project-memory-actions">
            {memory.sourcePath && <button onClick={() => void refreshSource(memory)} disabled={busyId === memory.id} title="Record the current file as the source for this note"><RefreshCw size={12} />Refresh source</button>}
            <button onClick={() => void togglePinned(memory)} disabled={busyId === memory.id} title={memory.pinned ? 'Unpin note in this list' : 'Pin note in this list'}><Bookmark size={12} fill={memory.pinned ? 'currentColor' : 'none'} />{memory.pinned ? 'Unpin' : 'Pin'}</button>
            <button onClick={() => edit(memory)} disabled={Boolean(busyId)}><Pencil size={12} />Edit</button>
            <button className="is-destructive" onClick={() => void remove(memory)} disabled={busyId === memory.id}><Trash2 size={12} />Delete</button>
          </div>
        </article>)}</div> : <p className="project-memory-empty">No project notes yet. Save a concise, reviewed convention or decision to avoid rediscovering it later.</p>}

    {draft && <form className="project-memory-editor" onSubmit={event => void save(event)}>
      <div className="project-memory-editor-heading"><strong>{draft.id ? 'Edit project note' : 'New project note'}</strong><button type="button" className="project-icon-action" onClick={() => setDraft(null)} aria-label="Close editor"><X size={14} /></button></div>
      <label>Title<input required maxLength={120} value={draft.title} onChange={event => setDraft({ ...draft, title: event.target.value })} placeholder="For example: How tests are run" /></label>
      <label>Category<select value={draft.category} onChange={event => setDraft({ ...draft, category: event.target.value as ProjectMemoryCategory })}>{categories.map(item => <option value={item.value} key={item.value}>{item.label}</option>)}</select></label>
      <label>Note<textarea required maxLength={2500} value={draft.content} onChange={event => setDraft({ ...draft, content: event.target.value })} placeholder="A short fact or decision reviewed by you…" /></label>
      <label>Source file <span className="project-memory-field-hint">workspace-relative path, such as AGENTS.md</span><input value={draft.sourcePath} onChange={event => setDraft({ ...draft, sourcePath: event.target.value, pinned: event.target.value.trim() ? draft.pinned : false })} placeholder="AGENTS.md" /></label>
      <label className="project-memory-pin-option"><input type="checkbox" checked={draft.pinned} onChange={event => setDraft({ ...draft, pinned: event.target.checked })} />Pin at top of this local list</label>
      <div className="project-memory-editor-actions"><button type="button" className="project-secondary-action" onClick={() => setDraft(null)}>Cancel</button><button className="project-save-button" type="submit" disabled={busyId !== null}>{busyId ? <LoaderCircle size={13} className="spin" /> : <Save size={13} />}{busyId ? 'Saving…' : 'Save note'}</button></div>
    </form>}
    {error && <p className="project-memory-error" role="alert">{error}</p>}
  </section>;
}

function formatDate(value: string | null) {
  if (!value) return 'never';
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? 'unknown' : date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}
