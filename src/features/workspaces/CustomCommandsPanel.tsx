import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { AlertTriangle, Check, Eye, LoaderCircle, RefreshCw, X } from 'lucide-react';
import { command } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { CustomCommand, CustomCommandFormat, CustomCommandPreview, Project } from '../../types/domain';

type Props = { project: Project; desktopAllowed: boolean };

const formatLabel: Record<CustomCommandFormat, string> = {
  claude_command: 'Claude Code command',
  antigravity_workflow: 'Antigravity workflow',
  gemini_command: 'Gemini CLI command',
};

/** One command with its status. Commands that can be converted offer a preview. */
export function CustomCommandRow({ item, busy, previewOpen, onPreview, children }: {
  item: CustomCommand;
  busy: boolean;
  previewOpen: boolean;
  onPreview: () => void;
  children?: ReactNode;
}) {
  const reviewCount = item.warnings.length;
  return <li className={`project-context-item is-${item.blocked ? 'skipped' : 'importable'}`}>
    <div className="project-context-item-heading">
      <div className="project-context-item-title"><code>{item.id}</code><span>{formatLabel[item.format]}</span></div>
      <span className={`project-context-status is-${item.blocked ? 'skipped' : 'importable'}`}>{item.blocked ? 'Not converted' : 'Ready'}</span>
    </div>
    {item.name && <p className="project-context-summary">Skill <code>{item.name}</code>{item.description ? ` · ${item.description}` : ''}</p>}
    {item.blocked && <p className="project-context-warning is-danger"><AlertTriangle size={12} />{item.blocked}</p>}
    {reviewCount > 0 && <p className="project-context-warning"><AlertTriangle size={12} />{reviewCount} {reviewCount === 1 ? 'behavior change' : 'behavior changes'} to review</p>}
    {!item.blocked && <button className="project-context-preview-toggle" onClick={onPreview} disabled={busy} aria-expanded={previewOpen}><Eye size={12} />Preview conversion</button>}
    {children}
  </li>;
}

/** The conversion preview: where the skill goes, what a skill cannot keep, and the text it would contain. */
export function CommandPreview({ view, acknowledged, busy, onAcknowledge, onCreate, onClose }: {
  view: CustomCommandPreview;
  acknowledged: boolean;
  busy: boolean;
  onAcknowledge: (checked: boolean) => void;
  onCreate: () => void;
  onClose: () => void;
}) {
  const warnings = view.command.warnings;
  const needsAcknowledgement = warnings.length > 0;
  return <div className="project-context-confirm" role="group" aria-label="Conversion preview">
    <div className="project-memory-editor-heading">
      <strong>Create skill {view.command.name}?</strong>
      <button type="button" className="project-icon-action" onClick={onClose} aria-label="Close preview"><X size={14} /></button>
    </div>
    <p>The skill is written to <code>{view.target}</code>. Its source file and a fingerprint of its text are recorded in its metadata. The command text is never run.</p>
    {needsAcknowledgement && <>
      <p>A skill cannot keep these parts of the command:</p>
      <ul>{warnings.map(warning => <li key={warning}>{warning}</li>)}</ul>
      <label><input type="checkbox" checked={acknowledged} onChange={event => onAcknowledge(event.target.checked)} /> I understand these changes</label>
    </>}
    <pre className="project-context-preview">{view.skillText}</pre>
    <div className="project-memory-editor-actions">
      <button type="button" className="project-secondary-action" onClick={onClose} disabled={busy}>Cancel</button>
      <button className="project-save-button" onClick={onCreate} disabled={busy || !view.target || (needsAcknowledgement && !acknowledged)}>
        {busy ? <LoaderCircle size={13} className="spin" /> : <Check size={13} />}{busy ? 'Creating…' : 'Create skill'}
      </button>
    </div>
  </div>;
}

/** Lists the commands other agents keep in this project and converts one into a skill after a preview. */
export function CustomCommandsPanel({ project, desktopAllowed }: Props) {
  const [commands, setCommands] = useState<CustomCommand[] | null>(null);
  const [preview, setPreview] = useState<CustomCommandPreview | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!desktopAllowed) return;
    setBusy(true);
    setError(null);
    try {
      setCommands(await command('list_custom_commands', { projectId: project.id }));
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusy(false); }
  }, [desktopAllowed, project.id]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => { setPreview(null); setAcknowledged(false); setNotice(null); }, [project.id]);

  function closePreview() {
    setPreview(null);
    setAcknowledged(false);
  }

  async function openPreview(item: CustomCommand) {
    setBusy(true);
    setError(null);
    setNotice(null);
    setAcknowledged(false);
    try {
      setPreview(await command('preview_custom_command', { projectId: project.id, id: item.id }));
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusy(false); }
  }

  async function createSkill() {
    if (!preview) return;
    setBusy(true);
    setError(null);
    try {
      const target = await command('convert_custom_command', { projectId: project.id, id: preview.command.id, confirmed: acknowledged });
      closePreview();
      setNotice(`Created ${target}. It is listed with the project’s skills.`);
      await load();
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusy(false); }
  }

  return <section className="project-context-section" aria-labelledby="custom-commands-title">
    <div className="project-memory-heading">
      <div><h2 id="custom-commands-title">Commands from other agents</h2><p>Claude Code commands, Antigravity workflows, and Gemini CLI commands in this project.</p></div>
      <button className="project-icon-action" onClick={() => void load()} disabled={!desktopAllowed || busy} title="Scan again" aria-label="Scan commands again"><RefreshCw size={14} className={busy ? 'spin' : undefined} /></button>
    </div>

    <p className="project-context-intro">Converting a command creates a skill in <code>.agents/skills</code>. You see the skill text and every part a skill cannot keep before anything is written. Existing skills are never overwritten.</p>

    {!desktopAllowed ? <p className="project-memory-empty">Command conversion is available in the desktop application.</p>
      : commands === null ? <p className="project-memory-empty"><LoaderCircle size={13} className="spin" />Looking for commands…</p>
        : !commands.length ? <p className="project-memory-empty">No commands from other agents were found in this project.</p>
          : <ul className="project-context-list">{commands.map(item => <CustomCommandRow key={item.id} item={item} busy={busy} previewOpen={preview?.command.id === item.id} onPreview={() => void openPreview(item)}>
            {preview?.command.id === item.id && <CommandPreview view={preview} acknowledged={acknowledged} busy={busy} onAcknowledge={setAcknowledged} onCreate={() => void createSkill()} onClose={closePreview} />}
          </CustomCommandRow>)}</ul>}

    {notice && <p className="project-context-notice" role="status"><Check size={12} />{notice}</p>}
    {error && <p className="project-memory-error" role="alert">{error}</p>}
  </section>;
}
