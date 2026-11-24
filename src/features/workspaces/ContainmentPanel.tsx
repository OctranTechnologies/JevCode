import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, LoaderCircle, ShieldAlert, ShieldCheck, X } from 'lucide-react';
import { getContainmentStatus, setProjectReducedSafety } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type {
  ContainmentEnforcement, ContainmentProfileKind, ContainmentStatus, Project,
} from '../../types/domain';

/** Exact phrase the backend accepts when a project opts into reduced safety. */
export const REDUCED_SAFETY_CONFIRMATION = 'reduced safety';

type ContainmentCondition = 'enforced' | 'reduced' | 'blocked' | 'machine_disabled';

const conditionLabel: Record<ContainmentCondition, string> = {
  enforced: 'Enforced',
  reduced: 'Reduced',
  blocked: 'Blocked',
  machine_disabled: 'Disabled',
};

const rowLabel: Record<ContainmentCondition, string> = {
  enforced: 'Contained',
  reduced: 'Process limits only',
  blocked: 'Unavailable',
  machine_disabled: 'Unavailable',
};

function profile(status: ContainmentStatus, kind: ContainmentProfileKind): ContainmentEnforcement | null {
  return status.profiles.find(item => item.kind === kind)?.enforcement ?? null;
}

function combine(first: ContainmentEnforcement | null, second: ContainmentEnforcement | null): ContainmentEnforcement | null {
  if (!first) return second;
  if (!second) return first;
  if (first.state === 'blocked' || second.state === 'blocked') return { state: 'blocked', reason: 'containment_unavailable' };
  if (first.state === 'reduced_safety' || second.state === 'reduced_safety') {
    const unsupported = [
      ...(first.state === 'reduced_safety' ? first.unsupported : []),
      ...(second.state === 'reduced_safety' ? second.unsupported : []),
    ];
    return { state: 'reduced_safety', unsupported: [...new Set(unsupported)] };
  }
  return { state: 'enforced' };
}

function conditionOf(status: ContainmentStatus): ContainmentCondition {
  if (!status.machineAllowed) return 'machine_disabled';
  const states = status.profiles.map(item => item.enforcement.state);
  if (states.length > 0 && states.every(state => state === 'enforced')) return 'enforced';
  if (status.projectOptedIn) return 'reduced';
  return 'blocked';
}

function rowValue(condition: ContainmentCondition, value: ContainmentEnforcement | null, newTask = false): string {
  if (condition === 'machine_disabled' || condition === 'blocked') {
    return newTask ? 'Cannot start' : 'Unavailable';
  }
  if (condition === 'reduced') {
    return newTask ? 'Can start with limits' : 'Process limits only';
  }
  return value?.state === 'enforced' ? 'Contained' : rowLabel[condition];
}

function summaryCopy(condition: ContainmentCondition, platform: ContainmentStatus['platform']): string {
  if (condition === 'machine_disabled') {
    return 'Reduced safety is disabled on this machine. Every project stays blocked until you allow reduced safety in Settings → Permissions.';
  }
  if (condition === 'blocked') {
    return platform === 'windows'
      ? 'This project is blocked until reduced safety is turned on for it. Agent commands and child-process Git stay unavailable; file edits and in-app file reads still work.'
      : 'Command execution is blocked on this system because Octran Code cannot enforce this task’s containment profile. File edits and in-app file reads still work.';
  }
  if (condition === 'reduced') {
    return 'Reduced safety is on. Process limits only. Commands can read and write anything your user account can reach and can use the network. Octran Code is not containing them. Continue only for repositories you trust.';
  }
  return 'Commands, Git, and search run inside this project’s task containment profile.';
}

function unsupportedCapabilities(status: ContainmentStatus): string[] {
  return [...new Set(status.profiles.flatMap(item =>
    item.enforcement.state === 'reduced_safety' ? item.enforcement.unsupported : [],
  ))];
}

export function ContainmentPanelView({ status, busy, error, onEnable, onDisable }: {
  status: ContainmentStatus;
  busy?: boolean;
  error?: string | null;
  onEnable: () => void;
  onDisable: () => void;
}) {
  const condition = conditionOf(status);
  const inspect = profile(status, 'inspect');
  const edit = profile(status, 'edit');
  const reduced = profile(status, 'reduced_safety');
  const unsupported = unsupportedCapabilities(status);
  const rows = [
    { label: 'Agent commands', value: rowValue(condition, edit) },
    { label: 'Git and search', value: rowValue(condition, inspect) },
    { label: 'New tasks', value: rowValue(condition, combine(inspect, edit), true) },
  ];
  return <section className="project-containment-section" aria-labelledby="project-containment-heading">
    <div className="project-section-heading">
      <div><h2 id="project-containment-heading">Containment</h2><span>What Octran Code can enforce for this project</span></div>
      <span className={`project-containment-state is-${condition}`}>{conditionLabel[condition]}</span>
    </div>
    <div className="project-containment-body">
      <dl className="project-containment-facts">
        {rows.map(row => <div key={row.label}><dt>{row.label}</dt><dd className={`is-${condition}`}>{row.value}</dd></div>)}
      </dl>
      <p className={`project-containment-note is-${condition}`}><AlertTriangle size={14} aria-hidden="true" /><span>{summaryCopy(condition, status.platform)}</span></p>
      {condition === 'reduced' && unsupported.length > 0 && <div className="project-containment-unsupported">
        <span>Not enforced</span>
        <ul>{unsupported.map(capability => <li key={capability}><code>{capability}</code></li>)}</ul>
      </div>}
      {condition === 'enforced' && reduced && <p className="project-containment-support"><ShieldCheck size={13} />Reduced safety is off for this project.</p>}
      {error && <p className="project-containment-error" role="alert">{error}</p>}
      <div className="project-containment-actions">
        {((condition === 'blocked' && status.platform === 'windows') || condition === 'machine_disabled') && <button
          type="button"
          className="project-secondary-action"
          onClick={onEnable}
          disabled={busy || condition === 'machine_disabled'}
          title={condition === 'machine_disabled' ? 'Allow reduced safety on this machine in Settings first.' : undefined}
        >{busy ? <LoaderCircle size={13} className="spin" /> : <ShieldAlert size={13} />}Turn on reduced safety</button>}
        {condition === 'reduced' && <button type="button" className="project-secondary-action" onClick={onDisable} disabled={busy}>
          {busy ? <LoaderCircle size={13} className="spin" /> : <ShieldCheck size={13} />}Turn off reduced safety
        </button>}
      </div>
    </div>
  </section>;
}

/** The dialog body is pure so the typed-phrase gate can be tested without a DOM. */
export function ReducedSafetyDialogView({ projectName, phrase, busy, error, onPhrase, onClose, onConfirm }: {
  projectName: string;
  phrase: string;
  busy: boolean;
  error?: string | null;
  onPhrase: (value: string) => void;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const matches = phrase === REDUCED_SAFETY_CONFIRMATION;
  return <div className="project-dialog-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
    <section className="project-create-dialog project-containment-dialog" role="dialog" aria-modal="true" aria-labelledby="reduced-safety-title">
      <header>
        <div><span className="project-dialog-icon"><ShieldAlert size={15} /></span><h2 id="reduced-safety-title">Turn on reduced safety</h2></div>
        <button className="project-icon-action" onClick={onClose} aria-label="Close dialog" disabled={busy}><X size={16} /></button>
      </header>
      <form onSubmit={event => { event.preventDefault(); if (matches && !busy) onConfirm(); }}>
        <p className="project-containment-dialog-lead">Reduced safety turns off containment for commands in <strong>{projectName}</strong>. It is not a sandbox.</p>
        <div className="project-containment-dialog-limits">
          <strong>It does not restrict</strong>
          <ul>
            <li>Writes outside the workspace</li>
            <li>Reads of credentials and other folders</li>
            <li>Network access</li>
          </ul>
        </div>
        <p className="project-containment-dialog-warning">Commands can read and write anything your user account can reach and can use the network. Octran Code is not containing them. Continue only for repositories you trust.</p>
        <label>Type “{REDUCED_SAFETY_CONFIRMATION}” to confirm<input autoFocus value={phrase} onChange={event => onPhrase(event.target.value)} disabled={busy} autoComplete="off" spellCheck={false} /></label>
        {phrase.length > 0 && !matches && <p className="project-containment-dialog-hint" role="status">The phrase must match exactly, in lowercase.</p>}
        {error && <p className="project-containment-error" role="alert">{error}</p>}
        <footer>
          <span />
          <button type="button" className="project-secondary-action" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="project-primary-action" type="submit" disabled={!matches || busy}>{busy ? <LoaderCircle size={13} className="spin" /> : <ShieldAlert size={13} />}{busy ? 'Turning on' : 'Turn on reduced safety'}</button>
        </footer>
      </form>
    </section>
  </div>;
}

export function ContainmentPanel({ project, desktopAllowed, onProjectUpdated, onError }: {
  project: Project;
  desktopAllowed: boolean;
  onProjectUpdated: (project: Project) => void;
  onError: (message: string | null) => void;
}) {
  const [status, setStatus] = useState<ContainmentStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!desktopAllowed) { setLoading(false); return; }
    try {
      setStatus(await getContainmentStatus(project.id));
      setError(null);
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setLoading(false);
    }
  }, [desktopAllowed, project.id]);

  useEffect(() => {
    let active = true;
    setStatus(null);
    setLoading(true);
    setError(null);
    setDialogOpen(false);
    if (!desktopAllowed) { setStatus(null); setLoading(false); return () => { active = false; }; }
    void getContainmentStatus(project.id)
      .then(next => { if (active) setStatus(next); })
      .catch(cause => { if (active) setError(normalizeError(cause).message); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [desktopAllowed, project.id]);

  async function changeReducedSafety(enabled: boolean, confirmation: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    onError(null);
    try {
      const next = await setProjectReducedSafety(project.id, enabled, confirmation);
      setStatus(next);
      onProjectUpdated({ ...project, reducedSafetyOptIn: next.projectOptedIn });
      setDialogOpen(false);
    } catch (cause) {
      const message = normalizeError(cause).message;
      setError(message);
      onError(message);
    } finally {
      setBusy(false);
    }
  }

  if (!desktopAllowed) {
    return <section className="project-containment-section" aria-labelledby="project-containment-heading">
      <div className="project-section-heading"><div><h2 id="project-containment-heading">Containment</h2><span>What Octran Code can enforce for this project</span></div></div>
      <p className="project-section-empty">Containment status is available in the desktop app.</p>
    </section>;
  }

  return <>
    {status
      ? <ContainmentPanelView status={status} busy={busy} error={error} onEnable={() => setDialogOpen(true)} onDisable={() => void changeReducedSafety(false, '')} />
      : <section className="project-containment-section" aria-labelledby="project-containment-heading">
        <div className="project-section-heading"><div><h2 id="project-containment-heading">Containment</h2><span>What Octran Code can enforce for this project</span></div></div>
        {loading && <p className="project-section-empty"><LoaderCircle size={13} className="spin" /> Loading containment status…</p>}
        {!loading && error && <p className="project-containment-error" role="alert">{error}<button type="button" className="project-text-action" onClick={() => void load()}>Retry</button></p>}
      </section>}
    {dialogOpen && <ReducedSafetyDialog
      projectName={project.name}
      busy={busy}
      error={error}
      onClose={() => setDialogOpen(false)}
      onConfirm={phrase => void changeReducedSafety(true, phrase)}
    />}
  </>;
}

function ReducedSafetyDialog({ projectName, busy, error, onClose, onConfirm }: {
  projectName: string;
  busy: boolean;
  error: string | null;
  onClose: () => void;
  onConfirm: (phrase: string) => void;
}) {
  const [phrase, setPhrase] = useState('');
  return <ReducedSafetyDialogView
    projectName={projectName}
    phrase={phrase}
    busy={busy}
    error={error}
    onPhrase={setPhrase}
    onClose={onClose}
    onConfirm={() => onConfirm(phrase)}
  />;
}
