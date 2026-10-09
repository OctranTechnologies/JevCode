import { useCallback, useEffect, useMemo, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import {
  AlertCircle, ArrowRight, ChevronDown, ChevronRight, CircleDot, ExternalLink,
  File, FileCode2, Folder, FolderOpen, GitBranch, HardDrive, LoaderCircle, Play,
  RefreshCw, Save, TerminalSquare,
} from 'lucide-react';
import { command } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import { displayPath } from '../../lib/paths';
import type {
  AgentSession, ChangedFile, PermissionDecision, PermissionPolicy, Project,
  ProjectFileEntry, ProjectOverview as ProjectOverviewData, Provider, TerminalResult,
} from '../../types/domain';

type Props = {
  project: Project;
  sessions: AgentSession[];
  providers: Provider[];
  desktopAllowed: boolean;
  onStartTask: () => void;
  onSelectSession: (session: AgentSession) => void;
  onProjectUpdated: (project: Project) => void;
  onError: (message: string | null) => void;
};

const previewOverview = (project: Project): ProjectOverviewData => ({
  projectId: project.id,
  repositoryRoot: project.path,
  activeBranch: 'feat/path-safety',
  gitStatusAvailable: true,
  isDirty: true,
  changedFiles: [
    { path: 'src/workspaces/paths.rs', status: 'M' },
    { path: 'tests/workspaces.rs', status: 'A' },
  ],
  repositorySizeBytes: 18_640_200,
  scanLimited: false,
  languages: [{ name: 'Rust', files: 28 }, { name: 'TypeScript', files: 17 }, { name: 'CSS', files: 6 }, { name: 'Markdown', files: 5 }],
});

export function ProjectOverview({
  project, sessions, providers, desktopAllowed, onStartTask,
  onSelectSession, onProjectUpdated, onError,
}: Props) {
  const [overview, setOverview] = useState<ProjectOverviewData | null>(null);
  const [branches, setBranches] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [branchBusy, setBranchBusy] = useState(false);
  const [instructions, setInstructions] = useState(project.projectInstructions);
  const [preferredModel, setPreferredModel] = useState(project.preferredModel ?? '');
  const [permissions, setPermissions] = useState<PermissionPolicy>(project.permissions);
  const [saving, setSaving] = useState(false);
  const [terminalOpen, setTerminalOpen] = useState(false);
  const recentTasks = useMemo(() => sessions
    .filter(session => session.projectId === project.id)
    .slice(0, 5), [project.id, sessions]);
  const refreshOverview = useCallback(async () => {
    const [fresh, names] = await Promise.all([
      command('project_overview', { projectId: project.id }),
      command('project_branches', { projectId: project.id }).catch(() => []),
    ]);
    setOverview(fresh);
    setBranches(names);
  }, [project.id]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setOverview(null);
    setBranches([]);
    setInstructions(project.projectInstructions);
    setPreferredModel(project.preferredModel ?? '');
    setPermissions(project.permissions);
    if (!desktopAllowed) {
      setOverview(previewOverview(project));
      setBranches(['feat/path-safety', 'main']);
      setLoading(false);
      return () => { active = false; };
    }
    void Promise.all([
      command('project_overview', { projectId: project.id }),
      command('project_branches', { projectId: project.id }).catch(() => []),
    ]).then(([value, names]) => {
      if (active) { setOverview(value); setBranches(names); }
    }).catch(error => {
      if (active) onError(normalizeError(error).message);
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [desktopAllowed, onError, project]);

  async function switchBranch(next: string) {
    if (!desktopAllowed || !next || next === overview?.activeBranch || branchBusy) return;
    setBranchBusy(true);
    onError(null);
    try {
      const updated = await command('switch_project_branch', { projectId: project.id, branch: next });
      onProjectUpdated(updated);
      await refreshOverview();
    } catch (error) { onError(normalizeError(error).message); }
    finally { setBranchBusy(false); }
  }

  async function saveSettings() {
    if (!desktopAllowed || saving) return;
    setSaving(true);
    onError(null);
    try {
      const updated = await command('update_project_settings', {
        input: {
          projectId: project.id,
          projectInstructions: instructions,
          preferredModel: preferredModel || null,
          permissions,
        },
      });
      onProjectUpdated(updated);
    } catch (error) { onError(normalizeError(error).message); }
    finally { setSaving(false); }
  }

  async function reveal() {
    if (!desktopAllowed) return;
    try { await command('reveal_project', { projectId: project.id }); }
    catch (error) { onError(normalizeError(error).message); }
  }

  const modelOptions = providers.flatMap(provider => provider.models.filter(model => model.status !== 'unavailable').map(model => ({
    value: `${provider.id}|${model.id}`,
    label: `${model.displayName}${model.status === 'deprecated' ? ' · Deprecated' : ''} · ${provider.name}`,
  })));
  if (preferredModel && !modelOptions.some(option => option.value === preferredModel)) {
    modelOptions.unshift({ value: preferredModel, label: `Unavailable · ${preferredModel.replace('|', ' · ')}` });
  }
  const displayedSize = overview ? formatBytes(overview.repositorySizeBytes) : '—';

  return <div className="project-overview-scroll">
    {!desktopAllowed && <div className="project-preview-note"><CircleDot size={13} />Illustrative repository details · no local files were read or changed.</div>}
    <div className="project-overview-content">
      <header className="project-overview-heading">
        <div className="project-heading-copy">
          <span className="project-heading-icon"><FolderOpen size={18} /></span>
          <div><h1>{project.name}</h1><p title={displayPath(project.path)}>{displayPath(project.path)}</p></div>
        </div>
        <div className="project-heading-actions">
          <button className="project-secondary-action" onClick={() => setTerminalOpen(value => !value)}><TerminalSquare size={14} />Terminal</button>
          <button className="project-secondary-action" onClick={() => void reveal()} disabled={!desktopAllowed}><ExternalLink size={14} />Reveal</button>
          <button className="project-primary-action" onClick={onStartTask}><Play size={13} fill="currentColor" />New task</button>
        </div>
      </header>

      <section className="project-facts" aria-label="Repository summary">
        <div className="project-branch-fact">
          <GitBranch size={14} />
          {branches.length > 0 ? <label className="project-branch-select"><span className="sr-only">Git branch</span><select aria-label="Git branch" value={overview?.activeBranch ?? ''} disabled={!desktopAllowed || branchBusy} onChange={event => void switchBranch(event.target.value)}>
            {!overview?.activeBranch && <option value="">Choose a branch</option>}
            {overview?.activeBranch && !branches.includes(overview.activeBranch) && <option value={overview.activeBranch}>{overview.activeBranch}</option>}
            {branches.map(branch => <option key={branch} value={branch}>{branch}</option>)}
          </select><ChevronDown size={12} />{branchBusy && <LoaderCircle size={12} className="spin" />}</label>
          : <span>{overview?.activeBranch ?? (overview?.repositoryRoot ? overview.gitStatusAvailable ? 'No local branches' : 'Git unavailable' : 'Not a Git repository')}</span>}
          {overview?.repositoryRoot && <span className={`repo-state${overview.gitStatusAvailable ? overview.isDirty ? ' is-dirty' : ' is-clean' : ' is-unavailable'}`}><i />{overview.gitStatusAvailable ? overview.isDirty ? 'Changes' : 'Clean' : 'Git unavailable'}</span>}
        </div>
        <div className="project-fact"><HardDrive size={14} /><span>{displayedSize}</span><small>{overview?.scanLimited ? 'first 20,000 files' : 'non-ignored files'}</small>{loading && <span className="fact-skeleton" />}</div>
        <div className="project-fact project-root-fact" title={overview?.repositoryRoot ?? 'No Git root detected'}><Folder size={14} /><span>{overview?.repositoryRoot ? shortenPath(overview.repositoryRoot) : 'Folder workspace'}</span><small>{overview?.repositoryRoot ? 'repository root' : 'project root'}</small></div>
      </section>

      {terminalOpen && <ProjectTerminal projectId={project.id} desktopAllowed={desktopAllowed} onClose={() => setTerminalOpen(false)} onRunComplete={refreshOverview} onError={onError} />}

      <div className="project-overview-grid">
        <section className="project-explorer-panel" aria-label="Project files">
          <div className="project-section-heading"><div><h2>Files</h2><span>Browse folders as needed</span></div><button className="project-icon-action" title="Refresh files" aria-label="Refresh files" onClick={() => window.dispatchEvent(new CustomEvent('project-tree-refresh', { detail: project.id }))}><RefreshCw size={14} /></button></div>
          <ProjectExplorer key={project.id} projectId={project.id} desktopAllowed={desktopAllowed} />
        </section>

        <div className="project-overview-rail">
          <section className="project-section project-changes-section">
            <div className="project-section-heading"><div><h2>Changed files</h2><span>{overview?.changedFiles.length ?? 0} in working tree</span></div><span className={`project-change-count${overview?.changedFiles.length ? ' has-changes' : ''}`}>{overview?.changedFiles.length ?? '—'}</span></div>
            {loading ? <SectionSkeleton rows={2} /> : overview?.changedFiles.length ? <ul className="project-change-list">{overview.changedFiles.slice(0, 7).map(file => <ChangedFileRow key={`${file.status}-${file.path}`} file={file} />)}</ul> : <p className="project-section-empty">No uncommitted changes.</p>}
          </section>

          <section className="project-section project-tasks-section">
            <div className="project-section-heading"><div><h2>Recent tasks</h2><span>Latest activity in this project</span></div></div>
            {recentTasks.length ? <ul className="project-task-list">{recentTasks.map(session => <li key={session.id}><button onClick={() => onSelectSession(session)}><span className={`project-task-status is-${session.status}`} /><span className="project-task-title">{session.title || 'New task'}</span><ArrowRight size={13} /></button><small>{formatDate(session.updatedAt)}</small></li>)}</ul> : <p className="project-section-empty">Tasks started here will appear in this list.</p>}
          </section>

          <section className="project-section project-languages-section">
            <div className="project-section-heading"><div><h2>Languages</h2><span>Detected from visible source files</span></div></div>
            {loading ? <SectionSkeleton rows={3} /> : overview?.languages.length ? <ul>{overview.languages.slice(0, 5).map(language => <li key={language.name}><span>{language.name}</span><small>{language.files} {language.files === 1 ? 'file' : 'files'}</small></li>)}</ul> : <p className="project-section-empty">No recognized source files yet.</p>}
          </section>
        </div>
      </div>

      <section className="project-instructions-section">
        <div className="project-instructions-heading"><div><h2>Project instructions</h2><p>These are included when a new agent task starts in this project.</p></div><button className="project-save-button" onClick={() => void saveSettings()} disabled={!desktopAllowed || saving || (instructions === project.projectInstructions && preferredModel === (project.preferredModel ?? '') && samePolicy(permissions, project.permissions))}>{saving ? <LoaderCircle size={13} className="spin" /> : <Save size={13} />}{saving ? 'Saving' : 'Save changes'}</button></div>
        <textarea value={instructions} onChange={event => setInstructions(event.target.value)} maxLength={50_000} disabled={!desktopAllowed} placeholder="Add instructions that should guide every new task in this project…" aria-label="Project instructions" />
        <div className="project-preferences-row">
          <label>Preferred model<select value={preferredModel} onChange={event => setPreferredModel(event.target.value)} disabled={!desktopAllowed}><option value="">Use workspace default</option>{modelOptions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label>
          <PermissionSelect label="File access" value={permissions.readFiles} onChange={value => setPermissions(current => ({ ...current, readFiles: value }))} disabled={!desktopAllowed} />
          <PermissionSelect label="Git tools" value={permissions.git} onChange={value => setPermissions(current => ({ ...current, git: value }))} disabled={!desktopAllowed} />
          <PermissionSelect label="File edits" value={permissions.writeFiles} onChange={value => setPermissions(current => ({ ...current, writeFiles: value }))} disabled={!desktopAllowed} />
          <PermissionSelect label="Agent commands" value={permissions.shell} onChange={value => setPermissions(current => ({ ...current, shell: value }))} disabled={!desktopAllowed} />
          <PermissionSelect label="Outside project" value={permissions.externalFiles} onChange={value => setPermissions(current => ({ ...current, externalFiles: value }))} disabled={!desktopAllowed} />
        </div>
      </section>
    </div>
  </div>;
}

function ProjectExplorer({ projectId, desktopAllowed }: { projectId: string; desktopAllowed: boolean }) {
  const [children, setChildren] = useState<Record<string, ProjectFileEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(['.']));
  const [loading, setLoading] = useState<Set<string>>(() => new Set());
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (path: string) => {
    setLoading(current => new Set(current).add(path));
    setError(null);
    try {
      return await command('list_project_directory', { projectId, path });
    } finally {
      setLoading(current => { const next = new Set(current); next.delete(path); return next; });
    }
  }, [projectId]);

  useEffect(() => {
    let active = true;
    setChildren({});
    setExpanded(new Set(['.']));
    setError(null);
    if (!desktopAllowed) {
      setChildren({
        '.': [mockEntry('.', 'Cargo.toml', 'file'), mockEntry('.', 'README.md', 'file'), mockEntry('.', 'src', 'directory'), mockEntry('.', 'tests', 'directory')],
        'src': [mockEntry('src', 'agent', 'directory'), mockEntry('src', 'workspaces', 'directory'), mockEntry('src', 'lib.rs', 'file')],
        'src/workspaces': [mockEntry('src/workspaces', 'mod.rs', 'file'), mockEntry('src/workspaces', 'paths.rs', 'file')],
        'tests': [mockEntry('tests', 'workspaces.rs', 'file')],
      });
      return () => { active = false; };
    }
    void load('.').then(entries => { if (active) setChildren({ '.': entries }); }).catch(error => { if (active) setError(normalizeError(error).message); });
    return () => { active = false; };
  }, [desktopAllowed, load, projectId]);

  useEffect(() => {
    const refresh = (event: Event) => {
      if ((event as CustomEvent<string>).detail !== projectId || !desktopAllowed) return;
      setChildren({});
      void load('.').then(entries => setChildren({ '.': entries })).catch(error => setError(normalizeError(error).message));
    };
    window.addEventListener('project-tree-refresh', refresh);
    return () => window.removeEventListener('project-tree-refresh', refresh);
  }, [desktopAllowed, load, projectId]);

  async function toggle(entry: ProjectFileEntry) {
    if (entry.kind !== 'directory') return;
    if (expanded.has(entry.path)) {
      setExpanded(current => { const next = new Set(current); next.delete(entry.path); return next; });
      return;
    }
    setExpanded(current => new Set(current).add(entry.path));
    if (!children[entry.path]) {
      try { const entries = await load(entry.path); setChildren(current => ({ ...current, [entry.path]: entries })); }
      catch (error) { setError(normalizeError(error).message); }
    }
  }

  function renderDirectory(path: string, depth: number): ReactNode {
    const entries = children[path] ?? [];
    return entries.map(entry => <li key={entry.path}>
      <button className={`project-file-row is-${entry.kind}`} style={{ '--tree-depth': depth } as CSSProperties} onClick={() => void toggle(entry)} aria-expanded={entry.kind === 'directory' ? expanded.has(entry.path) : undefined}>
        {entry.kind === 'directory' ? expanded.has(entry.path) ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : <span className="tree-chevron-spacer" />}
        {entry.kind === 'directory' ? expanded.has(entry.path) ? <FolderOpen size={14} /> : <Folder size={14} /> : extensionIcon(entry.name)}
        <span>{entry.name}{entry.isSymlink && <small>link</small>}</span>
        {entry.kind === 'file' && entry.sizeBytes > 0 && <small className="file-size">{formatBytes(entry.sizeBytes)}</small>}
        {loading.has(entry.path) && <LoaderCircle size={12} className="spin" />}
      </button>
      {entry.kind === 'directory' && expanded.has(entry.path) && <ul>{renderDirectory(entry.path, depth + 1)}</ul>}
    </li>);
  }

  return <div className="project-file-tree">
    {error && <div className="project-tree-error"><AlertCircle size={13} />{error}</div>}
    {loading.has('.') && !children['.'] ? <div className="project-tree-skeleton"><i /><i /><i /><i /><i /></div> : children['.']?.length ? <ul>{renderDirectory('.', 0)}</ul> : <p className="project-tree-empty">This folder has no visible files.</p>}
  </div>;
}

function mockEntry(parent: string, name: string, kind: ProjectFileEntry['kind']): ProjectFileEntry {
  return { name, path: parent === '.' ? name : `${parent}/${name}`, kind, sizeBytes: kind === 'file' ? 1800 : 0, isSymlink: false };
}

function ChangedFileRow({ file }: { file: ChangedFile }) {
  const status = file.status.trim();
  return <li><FileCode2 size={13} /><span title={file.path}>{file.path}</span><small className={`change-status is-${status.toLowerCase().replace(/[^a-z]/g, '') || 'modified'}`}>{status || 'M'}</small></li>;
}

function ProjectTerminal({ projectId, desktopAllowed, onClose, onRunComplete, onError }: { projectId: string; desktopAllowed: boolean; onClose: () => void; onRunComplete: () => Promise<void>; onError: (message: string | null) => void }) {
  const [commandText, setCommandText] = useState('');
  const [running, setRunning] = useState(false);
  const [history, setHistory] = useState<Array<{ command: string; result: TerminalResult }>>([]);

  async function run() {
    const value = commandText.trim();
    if (!desktopAllowed || !value || running) return;
    setRunning(true);
    setCommandText('');
    onError(null);
    try {
      const result = await command('run_project_terminal', { projectId, commandText: value });
      setHistory(items => [...items, { command: value, result }].slice(-40));
      await onRunComplete();
    } catch (error) { onError(normalizeError(error).message); }
    finally { setRunning(false); }
  }

  return <section className="project-terminal-panel" aria-label="Project terminal">
    <div className="project-terminal-heading"><div><TerminalSquare size={14} /><strong>Terminal</strong><span>Working directory: project root</span></div><button className="project-icon-action" onClick={onClose} aria-label="Close terminal"><ChevronDown size={15} /></button></div>
    <div className="project-terminal-output" aria-live="polite">
      {!desktopAllowed && <p className="terminal-preview-message">Open the desktop app to run commands in a local project.</p>}
      {desktopAllowed && history.length === 0 && <p className="terminal-preview-message">Commands run in this project folder with your user account permissions. Agent tools cannot access this terminal.</p>}
      {history.map((item, index) => <div className="terminal-entry" key={`${index}-${item.command}`}><div className="terminal-command-line"><span>›</span><code>{item.command}</code></div><pre>{item.result.output || '(no output)'}</pre><small className={item.result.exitCode === 0 ? 'is-success' : 'is-error'}>{item.result.timedOut ? 'Timed out' : `Exit ${item.result.exitCode ?? 'unknown'}`}</small></div>)}
      {running && <div className="terminal-running"><LoaderCircle size={13} className="spin" />Running command…</div>}
    </div>
    <div className="project-terminal-input"><span aria-hidden="true">›</span><textarea aria-label="Terminal command" value={commandText} onChange={event => setCommandText(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); void run(); } }} placeholder={desktopAllowed ? 'Enter a command…' : 'Desktop terminal unavailable in preview'} disabled={!desktopAllowed || running} rows={1} /><button className="project-primary-action" disabled={!desktopAllowed || running || !commandText.trim()} onClick={() => void run()}>{running ? <LoaderCircle size={13} className="spin" /> : <Play size={12} fill="currentColor" />}Run</button></div>
  </section>;
}

function PermissionSelect({ label, value, onChange, disabled }: { label: string; value: PermissionDecision; onChange: (value: PermissionDecision) => void; disabled: boolean }) {
  return <label>{label}<select value={value} onChange={event => onChange(event.target.value as PermissionDecision)} disabled={disabled}><option value="allow">Allow</option><option value="ask">Ask each time</option><option value="deny">Deny</option></select></label>;
}

function SectionSkeleton({ rows }: { rows: number }) {
  return <div className="project-section-skeleton" aria-label="Loading repository data">{Array.from({ length: rows }, (_, index) => <i key={index} />)}</div>;
}

function extensionIcon(name: string) {
  const extension = name.split('.').pop()?.toLowerCase();
  if (['ts', 'tsx', 'js', 'jsx', 'rs', 'py', 'go', 'java', 'c', 'cpp'].includes(extension ?? '')) return <FileCode2 size={14} />;
  return <File size={14} />;
}

function samePolicy(first: PermissionPolicy, second: PermissionPolicy) {
  return first.readFiles === second.readFiles && first.git === second.git && first.writeFiles === second.writeFiles && first.shell === second.shell && first.externalFiles === second.externalFiles && first.maxToolRounds === second.maxToolRounds;
}

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let size = value;
  let unit = -1;
  do { size /= 1024; unit += 1; } while (size >= 1024 && unit < units.length - 1);
  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unit]}`;
}

function shortenPath(path: string) {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-3).join('/')}` : path;
}

function formatDate(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }).format(date);
}
