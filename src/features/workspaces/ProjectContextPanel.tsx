import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, Check, ChevronRight, Eye, EyeOff, LoaderCircle, Plus, RefreshCw, X } from 'lucide-react';
import { command } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { ContextAgent, ContextItemStatus, Project, ProjectContextItem, ProjectContextScan } from '../../types/domain';

type Props = { project: Project; desktopAllowed: boolean; settingsDirty: boolean; onProjectUpdated: (project: Project) => void };

const agentLabel: Record<ContextAgent, string> = {
  jevcode: 'Octran Code',
  codex: 'Codex',
  claude_code: 'Claude Code',
  cursor: 'Cursor',
  gemini_cli: 'Gemini CLI',
  antigravity: 'Antigravity',
  copilot: 'GitHub Copilot',
};

const statusLabel: Record<ContextItemStatus, string> = {
  loaded: 'Loaded',
  importable: 'Not loaded',
  imported_stale: 'Source changed',
  imported: 'Added',
  unsupported: 'Not supported',
  skipped: 'Skipped',
};

const groups: Array<{ title: string; hint: string; statuses: ContextItemStatus[]; collapsible: boolean }> = [
  { title: 'Loaded by Octran Code', hint: 'Included when a new task starts.', statuses: ['loaded'], collapsible: false },
  { title: 'Ready to add', hint: 'Not loaded. Review a file, then add a copy to the project instructions below.', statuses: ['importable', 'imported_stale'], collapsible: false },
  { title: 'Added to project instructions', hint: 'Copied into the instructions below, where you can edit or remove it.', statuses: ['imported'], collapsible: false },
  { title: 'Not supported yet', hint: 'Detected, but Octran Code has no equivalent and uses nothing from these.', statuses: ['unsupported'], collapsible: true },
  { title: 'Skipped', hint: 'Not used because it could not be read safely.', statuses: ['skipped'], collapsible: true },
];

const selectable = (item: ProjectContextItem) => (item.status === 'importable' || item.status === 'imported_stale') && !item.importBlocked;

export function ProjectContextPanel({ project, desktopAllowed, settingsDirty, onProjectUpdated }: Props) {
  const [scan, setScan] = useState<ProjectContextScan | null>(null);
  const [loading, setLoading] = useState(desktopAllowed);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [previews, setPreviews] = useState<Set<string>>(() => new Set());
  const [confirming, setConfirming] = useState(false);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!desktopAllowed) return;
    setLoading(true);
    setError(null);
    try {
      const fresh = await command('scan_project_context', { projectId: project.id });
      setScan(fresh);
      // A selection survives a refresh only while the item can still be added.
      setSelected(current => new Set(fresh.items.filter(item => current.has(item.id) && selectable(item)).map(item => item.id)));
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setLoading(false); }
  }, [desktopAllowed, project.id]);

  // Saved instructions decide which files read as added, so a change rescans.
  useEffect(() => { void load(); }, [load, project.projectInstructions]);
  useEffect(() => { setNotice(null); setConfirming(false); setSelected(new Set()); setPreviews(new Set()); }, [project.id]);

  function toggle(set: Set<string>, id: string) {
    const next = new Set(set);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  }

  async function addSelected() {
    if (!selected.size || importing || settingsDirty) return;
    setImporting(true);
    setError(null);
    try {
      const count = selected.size;
      const updated = await command('import_project_context', { input: { projectId: project.id, itemIds: [...selected] } });
      setSelected(new Set());
      setConfirming(false);
      setNotice(`Added ${count} ${count === 1 ? 'item' : 'items'} to the project instructions. New tasks in this project use them.`);
      onProjectUpdated(updated);
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setImporting(false); }
  }

  const items = scan?.items ?? [];
  const selectedItems = items.filter(item => selected.has(item.id));
  const scopedSelections = selectedItems.filter(item => item.warnings.some(warning => warning.startsWith('Octran Code has no file-scoped'))).length;
  const hasReady = items.some(item => item.status === 'importable' || item.status === 'imported_stale');

  function renderItem(item: ProjectContextItem) {
    const canSelect = selectable(item);
    const otherAgents = item.agents.filter(agent => agent !== 'jevcode').map(agent => agentLabel[agent]).join(' · ');
    const previewOpen = previews.has(item.id);
    return <li className={`project-context-item is-${item.status}`} key={item.id}>
      <div className="project-context-item-heading">
        {canSelect
          ? <input type="checkbox" checked={selected.has(item.id)} onChange={() => { setConfirming(false); setSelected(current => toggle(current, item.id)); }} disabled={importing} aria-label={`Select ${item.path}`} />
          : item.status === 'loaded' || item.status === 'imported' ? <Check size={13} className="project-context-mark" /> : <span className="project-context-mark-placeholder" />}
        <div className="project-context-item-title"><code>{item.path}</code>{otherAgents && <span>{otherAgents}</span>}</div>
        <span className={`project-context-status is-${item.status}`}>{statusLabel[item.status]}</span>
      </div>
      <p className="project-context-summary">{item.summary}</p>
      {item.scope && <p className="project-context-scope">{item.scope}</p>}
      {item.secretLines.length > 0 && <p className="project-context-warning is-danger"><AlertTriangle size={12} />Possible secret on line {item.secretLines.slice(0, 8).join(', ')}{item.secretLines.length > 8 ? '…' : ''}. The value is not shown here.</p>}
      {item.importBlocked && <p className="project-context-warning is-danger"><AlertTriangle size={12} />Cannot be added: {item.importBlocked}</p>}
      {item.warnings.map(warning => <p className="project-context-warning" key={warning}><AlertTriangle size={12} />{warning}</p>)}
      {item.preview !== null && <>
        <button className="project-context-preview-toggle" onClick={() => setPreviews(current => toggle(current, item.id))} aria-expanded={previewOpen}>
          {previewOpen ? <EyeOff size={12} /> : <Eye size={12} />}{previewOpen ? 'Hide text' : 'Review text'}
        </button>
        {previewOpen && <pre className="project-context-preview">{item.preview}</pre>}
      </>}
    </li>;
  }

  return <section className="project-context-section" aria-labelledby="project-context-title">
    <div className="project-memory-heading">
      <div><h2 id="project-context-title">Context from other agents</h2><p>Instruction, rule, and skill files in this project, and what Octran Code does with each.</p></div>
      <button className="project-icon-action" onClick={() => void load()} disabled={!desktopAllowed || loading || importing} title="Scan again" aria-label="Scan project context again"><RefreshCw size={14} className={loading ? 'spin' : undefined} /></button>
    </div>

    <p className="project-context-intro">Octran Code loads the root <code>AGENTS.md</code> and <code>CLAUDE.md</code> and the project instructions below. Other files are only listed here. Adding one copies its text into Octran Code’s project instructions on this device; project files are never changed, and agent settings files are not read.</p>

    {!desktopAllowed ? <p className="project-memory-empty">Context scanning is available in the desktop application.</p>
      : loading && !scan ? <p className="project-memory-empty"><LoaderCircle size={13} className="spin" />Looking for instruction files…</p>
        : !items.length ? <p className="project-memory-empty">No instruction, rule, or skill files from other agents were found.</p>
          : <div className="project-context-groups">
            {groups.map(group => {
              const groupItems = items.filter(item => group.statuses.includes(item.status));
              if (!groupItems.length) return null;
              const heading = <><span className="project-context-group-title">{group.title}</span><span className="project-context-group-count">{groupItems.length}</span></>;
              const list = <><p className="project-context-group-hint">{group.hint}</p><ul className="project-context-list">{groupItems.map(renderItem)}</ul></>;
              return group.collapsible
                ? <details className="project-context-group" key={group.title}><summary><ChevronRight size={12} className="project-context-chevron" />{heading}</summary>{list}</details>
                : <div className="project-context-group" key={group.title}><div className="project-context-group-heading">{heading}</div>{list}</div>;
            })}
          </div>}

    {scan?.scanLimited && <p className="project-context-group-hint">This project is large, so some folders were not scanned.</p>}

    {hasReady && desktopAllowed && !confirming && <div className="project-context-actions">
      <span>{settingsDirty ? 'Save or undo your project setting changes before adding files.' : selected.size ? `${selected.size} selected` : 'Select files to add'}</span>
      <button className="project-save-button" onClick={() => { setNotice(null); setConfirming(true); }} disabled={!selected.size || settingsDirty || importing}><Plus size={13} />Add to instructions</button>
    </div>}

    {confirming && <div className="project-context-confirm" role="group" aria-label="Confirm adding context">
      <div className="project-memory-editor-heading"><strong>Add {selectedItems.length} {selectedItems.length === 1 ? 'item' : 'items'} to project instructions?</strong><button type="button" className="project-icon-action" onClick={() => setConfirming(false)} aria-label="Cancel"><X size={14} /></button></div>
      <ul>{selectedItems.map(item => <li key={item.id}><code>{item.path}</code>{item.status === 'imported_stale' ? ' · replaces the earlier copy' : ''}</li>)}</ul>
      <p>Octran Code copies the current text into this project’s instructions, stored on this device, and sends it with every new task. Project files stay unchanged.{scopedSelections ? ` ${scopedSelections} selected ${scopedSelections === 1 ? 'rule is' : 'rules are'} scoped in its original tool and will apply to every task here.` : ''}</p>
      <div className="project-memory-editor-actions"><button type="button" className="project-secondary-action" onClick={() => setConfirming(false)} disabled={importing}>Cancel</button><button className="project-save-button" onClick={() => void addSelected()} disabled={importing || settingsDirty || !selectedItems.length}>{importing ? <LoaderCircle size={13} className="spin" /> : <Plus size={13} />}{importing ? 'Adding…' : 'Add to instructions'}</button></div>
    </div>}

    {notice && <p className="project-context-notice" role="status"><Check size={12} />{notice}</p>}
    {error && <p className="project-memory-error" role="alert">{error}</p>}
  </section>;
}
