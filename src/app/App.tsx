import { useCallback, useEffect, useMemo, useState } from 'react';
import type { CSSProperties } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import {
  AlertCircle, ArrowUpRight, BriefcaseBusiness, CircleDot, Command, ShieldCheck,
  FileCode2, FolderOpen, FolderPlus, ListChecks, Moon, PanelLeftClose, PanelLeftOpen, Plus,
  Settings2, Sun, Terminal,
} from 'lucide-react';
import { useDesktop } from './useDesktop';
import { demoProject, demoSession } from './demo';
import { command, desktopAvailable } from '../lib/ipc';
import { normalizeError } from '../lib/errors';
import { WorkspaceSidebar } from '../features/workspaces/WorkspaceSidebar';
import { WorkspaceHeader } from '../features/workspaces/WorkspaceHeader';
import { Conversation } from '../features/agent/Conversation';
import { WorkspaceComposer } from '../features/agent/WorkspaceComposer';
import { InspectorPanel, type InspectorTab } from '../features/agent/InspectorPanel';
import { ProviderSettings } from '../features/providers/ProviderSettings';
import { UsageView } from '../features/usage/UsageView';
import { PermissionsSettings } from '../features/settings/PermissionsSettings';
import { ProjectOverview } from '../features/workspaces/ProjectOverview';
import { CreateProjectDialog } from '../features/workspaces/CreateProjectDialog';
import { CommandPalette, type PaletteAction } from '../components/CommandPalette';
import { Brand } from '../components/Brand';
import type { AgentSession, Model, ModelReference, PermissionResolution, Project, ReviewAllAction, ReviewFileAction, SessionChanges, SessionFileDiff } from '../types/domain';
import type { View } from '../features/workspaces/Sidebar';

const demoBranch = 'feat/path-safety';

export default function App() {
  const desktop = useDesktop();
  const dispatch = desktop.dispatch;
  const setError = desktop.setError;
  const [view, setView] = useState<View>(() => desktopAvailable ? 'overview' : 'agent');
  const [projectChoice, setProjectChoice] = useState(() => desktopAvailable ? '' : demoProject.id);
  const [sessionId, setSessionId] = useState<string | null>(() => desktopAvailable ? null : demoSession.id);
  const [providerChoice, setProviderChoice] = useState('preview');
  const [modelChoice, setModelChoice] = useState('');
  const [draft, setDraft] = useState('');
  const [attachments, setAttachments] = useState<string[]>([]);
  const [sending, setSending] = useState(false);
  const [opening, setOpening] = useState(false);
  const [createProjectOpen, setCreateProjectOpen] = useState(false);
  const [creatingProject, setCreatingProject] = useState(false);
  const [permissionBusy, setPermissionBusy] = useState(false);
  const [askReads, setAskReads] = useState(false);
  const [includeProject, setIncludeProject] = useState(true);
  const [contextOpen, setContextOpen] = useState(false);
  const [mode, setMode] = useState<'agent' | 'plan'>('agent');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [showArchivedTasks, setShowArchivedTasks] = useState(false);
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);
  const [inspectorOpen, setInspectorOpen] = useState(() => window.innerWidth >= 1180);
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>('changes');
  const [reviewChanges, setReviewChanges] = useState<SessionChanges | null>(null);
  const [selectedReviewPath, setSelectedReviewPath] = useState<string | null>(null);
  const [reviewDiff, setReviewDiff] = useState<SessionFileDiff | null>(null);
  const [reviewBusy, setReviewBusy] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(252);
  const [inspectorWidth, setInspectorWidth] = useState(318);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [branch, setBranch] = useState<string | null>(null);
  const [windowWidth, setWindowWidth] = useState(() => window.innerWidth);
  const [darkMode, setDarkMode] = useState(() => {
    const saved = localStorage.getItem('jevcode-theme');
    return saved ? saved === 'dark' : window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false;
  });

  const data = desktop.data;
  const projectList = data?.workspace.projects;
  const actualProjects = useMemo(() => projectList ?? [], [projectList]);
  const actualSession = desktop.sessions.find(item => item.id === sessionId);
  const demo = !desktopAvailable && sessionId === demoSession.id;
  const session: AgentSession | undefined = actualSession ?? (demo ? demoSession : undefined);
  const sessionKey = session?.id;
  const sessionVersion = session?.updatedAt;
  const projectId = (session?.projectId ?? projectChoice) || actualProjects[0]?.id || '';
  const project: Project | undefined = actualProjects.find(item => item.id === projectId)
    ?? (!desktopAvailable && projectId === demoProject.id ? demoProject : undefined);
  const projects = useMemo(() => !desktopAvailable ? [demoProject] : actualProjects, [actualProjects]);
  const allSessions = useMemo(() => !desktopAvailable ? [demoSession] : desktop.sessions, [desktop.sessions]);
  const recentSessions = useMemo(() => allSessions.filter(item => !item.archivedAt), [allSessions]);
  const providerId = session?.providerId ?? providerChoice;
  const provider = data?.providers.find(item => item.id === providerId);
  const modelId = (session?.modelId ?? modelChoice) || provider?.models[0]?.id || '';
  const busy = !!session && ['queued', 'planning', 'working', 'waiting_for_permission'].includes(session.status);
  const tokens = desktop.usage.reduce((sum, record) => sum + record.inputTokens + record.outputTokens, 0);

  const findAvailableModel = useCallback((reference: ModelReference | null | undefined): ModelReference | undefined => {
    if (!reference) return undefined;
    const owner = data?.providers.find(item => item.id === reference.providerId);
    return owner?.models.some(model => model.id === reference.modelId && model.status !== 'unavailable') ? reference : undefined;
  }, [data?.providers]);

  const preferredFor = useCallback((target: Project | undefined): ModelReference | undefined => {
    const [projectProvider, projectModel] = target?.preferredModel?.split('|') ?? [];
    return findAvailableModel(projectProvider && projectModel ? { providerId: projectProvider, modelId: projectModel } : undefined)
      ?? findAvailableModel(data?.modelPreferences.defaultModel)
      ?? data?.providers.flatMap(item => item.models.filter(model => model.status !== 'unavailable').map(model => ({ providerId: item.id, modelId: model.id })))[0];
  }, [data?.modelPreferences.defaultModel, data?.providers, findAvailableModel]);

  const updateRecent = useCallback((reference: ModelReference) => {
    if (!data) return;
    dispatch({ type: 'model-preferences', preferences: {
      ...data.modelPreferences,
      recent: [reference, ...data.modelPreferences.recent.filter(item => item.providerId !== reference.providerId || item.modelId !== reference.modelId)].slice(0, 8),
    } });
  }, [data, dispatch]);

  useEffect(() => {
    if (!data || !project || session) return;
    const currentIsAvailable = data.providers.some(item => item.id === providerChoice && item.models.some(model => model.id === modelChoice && model.status !== 'unavailable'));
    if (currentIsAvailable) return;
    const fallback = preferredFor(project);
    if (fallback) { setProviderChoice(fallback.providerId); setModelChoice(fallback.modelId); }
  // Resolve only when the current selection is absent from the active catalog.
  }, [data, preferredFor, project, session, providerChoice, modelChoice]);

  useEffect(() => {
    document.documentElement.dataset.theme = darkMode ? 'dark' : 'light';
    localStorage.setItem('jevcode-theme', darkMode ? 'dark' : 'light');
  }, [darkMode]);

  useEffect(() => {
    let previousWidth = window.innerWidth;
    function updateWindowWidth() {
      const nextWidth = window.innerWidth;
      setWindowWidth(nextWidth);
      if ((previousWidth >= 1180 && nextWidth < 1180) || (previousWidth >= 840 && nextWidth < 840)) setInspectorOpen(false);
      if (previousWidth >= 840 && nextWidth < 840) setMobileSidebarOpen(false);
      previousWidth = nextWidth;
    }
    window.addEventListener('resize', updateWindowWidth);
    return () => window.removeEventListener('resize', updateWindowWidth);
  }, []);

  useEffect(() => {
    let active = true;
    if (!desktopAvailable) { setBranch(session?.id === demoSession.id ? demoBranch : null); return; }
    if (!projectId) { setBranch(null); return; }
    setBranch(null);
    void command('project_branch', { projectId }).then(value => { if (active) setBranch(value); }).catch(() => { if (active) setBranch(null); });
    return () => { active = false; };
  }, [projectId, session?.id]);

  useEffect(() => {
    let active = true;
    if (!desktopAvailable || !sessionKey || sessionKey === demoSession.id) {
      setReviewChanges(null);
      setSelectedReviewPath(null);
      setReviewDiff(null);
      return;
    }
    void command('session_changes', { sessionId: sessionKey }).then(changes => {
      if (!active) return;
      setReviewChanges(changes);
      setSelectedReviewPath(current => current && changes.files.some(file => file.path === current) ? current : changes.files[0]?.path ?? null);
    }).catch(error => { if (active) setError(normalizeError(error).message); });
    return () => { active = false; };
  }, [sessionKey, sessionVersion, setError]);

  useEffect(() => {
    let active = true;
    if (!desktopAvailable || !sessionKey || !selectedReviewPath) { setReviewDiff(null); return; }
    void command('session_file_diff', { sessionId: sessionKey, path: selectedReviewPath }).then(value => { if (active) setReviewDiff(value); }).catch(() => { if (active) setReviewDiff(null); });
    return () => { active = false; };
  }, [sessionKey, selectedReviewPath, reviewChanges]);

  useEffect(() => {
    const readPermission = session?.permissionPolicy.readFiles ?? project?.permissions.readFiles;
    setIncludeProject(readPermission !== 'deny');
    setAskReads(readPermission === 'ask');
  }, [project?.id, project?.permissions.readFiles, session?.id, session?.permissionPolicy.readFiles]);

  const newTask = useCallback(() => {
    const preferred = preferredFor(project);
    if (preferred) { setProviderChoice(preferred.providerId); setModelChoice(preferred.modelId); }
    setSessionId(null);
    setReviewChanges(null);
    setSelectedReviewPath(null);
    setReviewDiff(null);
    setDraft('');
    setAttachments([]);
    setMode('agent');
    setView('agent');
    setMobileSidebarOpen(false);
    setError(null);
  }, [preferredFor, project, setError]);

  const chooseProject = useCallback((id: string) => {
    setProjectChoice(id);
    const selected = actualProjects.find(item => item.id === id);
    if (desktopAvailable && selected) {
      void command('open_project', { path: selected.path })
        .then(reopened => dispatch({ type: 'project', project: reopened }))
        .catch(error => setError(normalizeError(error).message));
    }
    const preferred = preferredFor(selected);
    if (preferred) { setProviderChoice(preferred.providerId); setModelChoice(preferred.modelId); }
    setSessionId(null);
    setReviewChanges(null);
    setSelectedReviewPath(null);
    setReviewDiff(null);
    setDraft('');
    setAttachments([]);
    setView('overview');
    setMobileSidebarOpen(false);
    setError(null);
  }, [actualProjects, dispatch, preferredFor, setError]);

  const selectSession = useCallback((item: AgentSession) => {
    const activate = (selected: AgentSession) => {
      setProjectChoice(selected.projectId);
      setProviderChoice(selected.providerId);
      setModelChoice(selected.modelId);
      setSessionId(selected.id);
      setReviewChanges(null);
      setSelectedReviewPath(null);
      setReviewDiff(null);
      setDraft('');
      setAttachments([]);
      setShowArchivedTasks(false);
      setView('agent');
      setMobileSidebarOpen(false);
      setError(null);
    };
    if (item.archivedAt && desktopAvailable) {
      void command('resume_session', { sessionId: item.id }).then(resumed => { dispatch({ type: 'session', session: resumed }); activate(resumed); }).catch(error => setError(normalizeError(error).message));
    } else activate(item);
  }, [dispatch, setError]);

  const renameSession = useCallback(async (item: AgentSession) => {
    const title = window.prompt('Rename task', item.title);
    if (title === null || !title.trim()) return;
    try { dispatch({ type: 'session', session: await command('rename_session', { input: { sessionId: item.id, title } }) }); }
    catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, setError]);

  const toggleSessionArchive = useCallback(async (item: AgentSession) => {
    try { dispatch({ type: 'session', session: await command('archive_session', { sessionId: item.id, archived: !item.archivedAt }) }); }
    catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, setError]);

  const duplicateSession = useCallback(async (item: AgentSession) => {
    try {
      const duplicate = await command('duplicate_session', { sessionId: item.id });
      dispatch({ type: 'session', session: duplicate });
      selectSession(duplicate);
    } catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, selectSession, setError]);

  const forkSession = useCallback(async (item: AgentSession) => {
    try {
      const fork = await command('fork_session', { sessionId: item.id });
      dispatch({ type: 'session', session: fork });
      selectSession(fork);
    } catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, selectSession, setError]);

  const deleteSession = useCallback(async (item: AgentSession) => {
    if (!window.confirm(`Delete “${item.title}” and its saved task history? This cannot be undone.`)) return;
    try {
      await command('delete_session', { sessionId: item.id });
      dispatch({ type: 'delete-session', sessionId: item.id });
      if (sessionId === item.id) { setSessionId(null); setView('overview'); }
    } catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, sessionId, setError]);

  const chooseView = useCallback((next: View) => {
    setView(next);
    setMobileSidebarOpen(false);
  }, []);

  const selectModel = useCallback(async (reference: ModelReference) => {
    if (session && desktopAvailable) {
      try {
        const updated = await command('update_session_model', { input: { sessionId: session.id, providerId: reference.providerId, modelId: reference.modelId } });
        dispatch({ type: 'session', session: updated });
        updateRecent(reference);
      } catch (error) { setError(normalizeError(error).message); return; }
    }
    setProviderChoice(reference.providerId);
    setModelChoice(reference.modelId);
  }, [dispatch, session, setError, updateRecent]);

  const toggleModelFavorite = useCallback(async (reference: ModelReference) => {
    if (!desktopAvailable) return;
    try { dispatch({ type: 'model-preferences', preferences: await command('toggle_model_favorite', { selection: reference }) }); }
    catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, setError]);

  const setDefaultModel = useCallback(async (reference: ModelReference) => {
    if (!desktopAvailable) return;
    try { dispatch({ type: 'model-preferences', preferences: await command('set_default_model', { selection: reference }) }); }
    catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, setError]);

  const updateModelCatalog = useCallback((providerId: string, models: Model[]) => {
    if (!data) return;
    dispatch({ type: 'providers', providers: data.providers.map(provider => provider.id === providerId ? { ...provider, models } : provider) });
  }, [data, dispatch]);

  const openFolder = useCallback(async () => {
    setOpening(true);
    try {
      if (!desktopAvailable) { await command('open_project', { path: '' }); return; }
      const path = await open({ directory: true, multiple: false, title: 'Open a project folder' });
      if (typeof path === 'string') {
        const opened = await command('open_project', { path });
        dispatch({ type: 'project', project: opened });
        chooseProject(opened.id);
      }
    } catch (error) { setError(normalizeError(error).message); }
    finally { setOpening(false); }
  }, [chooseProject, dispatch, setError]);

  const createProject = useCallback(async (name: string, parentPath: string) => {
    setCreatingProject(true);
    setError(null);
    try {
      const created = await command('create_project', { name, parentPath });
      dispatch({ type: 'project', project: created });
      setCreateProjectOpen(false);
      chooseProject(created.id);
    } catch (error) { setError(normalizeError(error).message); }
    finally { setCreatingProject(false); }
  }, [chooseProject, dispatch, setError]);

  const removeFromRecents = useCallback(async (item: Project) => {
    try {
      const updated = await command('remove_project_from_recents', { projectId: item.id });
      dispatch({ type: 'project', project: updated });
    } catch (error) { setError(normalizeError(error).message); }
  }, [dispatch, setError]);

  const revealProject = useCallback(async (item: Project) => {
    try { await command('reveal_project', { projectId: item.id }); }
    catch (error) { setError(normalizeError(error).message); }
  }, [setError]);

  const updateProject = useCallback((updated: Project) => {
    dispatch({ type: 'project', project: updated });
    const preferred = preferredFor(updated);
    if (preferred) { setProviderChoice(preferred.providerId); setModelChoice(preferred.modelId); }
  }, [dispatch, preferredFor]);

  async function attachFiles() {
    if (!desktopAvailable || !project) return;
    try {
      const value = await open({ directory: false, multiple: true, title: 'Attach project files', defaultPath: project.path });
      if (!value) return;
      const paths = Array.isArray(value) ? value : [value];
      const root = project.path.replace(/[\\/]+$/, '');
      const lowerRoot = root.toLocaleLowerCase();
      const relativePaths: string[] = [];
      for (const path of paths) {
        const normalized = path.replaceAll('\\', '/');
        if (!normalized.toLocaleLowerCase().startsWith(`${lowerRoot.replaceAll('\\', '/')}/`)) {
          desktop.setError('Choose files inside the selected project so JevCode can read them safely.');
          return;
        }
        relativePaths.push(normalized.slice(root.length + 1).replaceAll('\\', '/'));
      }
      setAttachments(current => [...new Set([...current, ...relativePaths])].slice(0, 8));
      desktop.setError(null);
    } catch (error) { desktop.setError(normalizeError(error).message); }
  }

  async function send(content = draft) {
    if (!data || !projectId || !content.trim() || busy || sending) return;
    setSending(true);
    desktop.setError(null);
    try {
      let active = session;
      if (!active) {
        active = await command('create_session', { input: {
          projectId, providerId, modelId,
          permissionPolicy: {
            ...data.permissionPolicy,
            readFiles: !includeProject ? 'deny' : askReads ? 'ask' : 'allow',
            git: project?.permissions.git ?? data.permissionPolicy.git,
            writeFiles: project?.permissions.writeFiles ?? data.permissionPolicy.writeFiles,
            shell: project?.permissions.shell ?? data.permissionPolicy.shell,
            externalFiles: project?.permissions.externalFiles ?? data.permissionPolicy.externalFiles,
            maxToolRounds: project?.permissions.maxToolRounds ?? data.permissionPolicy.maxToolRounds,
          },
        } });
        desktop.dispatch({ type: 'session', session: active });
        updateRecent({ providerId: active.providerId, modelId: active.modelId });
        setSessionId(active.id);
      }
      let prompt = content.trim();
      if (mode === 'plan') prompt += '\n\nPlanning mode: inspect relevant project files and return a clear step-by-step plan. Do not edit files or run commands.';
      if (!includeProject) prompt += '\n\nDo not inspect project files for this task; answer using only the conversation context.';
      if (attachments.length) prompt += `\n\nPlease use these selected project files as context:\n${attachments.map(path => `- ${path}`).join('\n')}`;
      const updated = await command('send_message', { sessionId: active.id, content: prompt });
      desktop.dispatch({ type: 'session', session: updated });
      setDraft('');
      setAttachments([]);
    } catch (error) { desktop.setError(normalizeError(error).message); }
    finally { setSending(false); }
  }

  async function resolvePermission(resolution: PermissionResolution) {
    if (!session?.pendingToolCall || permissionBusy) return;
    setPermissionBusy(true);
    try {
      const updated = await command('resolve_permission', { sessionId: session.id, toolCallId: session.pendingToolCall.id, resolution });
      desktop.dispatch({ type: 'session', session: updated });
    } catch (error) { desktop.setError(normalizeError(error).message); }
    finally { setPermissionBusy(false); }
  }

  async function stopTask() {
    if (!session) return;
    try { await command('cancel_session', { sessionId: session.id }); }
    catch (error) { desktop.setError(normalizeError(error).message); }
  }

  const reviewFileAction = useCallback(async (path: string, action: ReviewFileAction) => {
    if (!session || !desktopAvailable || reviewBusy) return;
    setReviewBusy(true);
    try {
      const updated = await command('review_file_action', { sessionId: session.id, path, action });
      setReviewChanges(updated);
      if (action !== 'open' && action !== 'stage' && selectedReviewPath === path && !updated.files.some(file => file.path === path)) {
        setSelectedReviewPath(updated.files[0]?.path ?? null);
        setReviewDiff(null);
      }
      if (action === 'open') return;
      if (action === 'stage') setError(null);
    } catch (error) { setError(normalizeError(error).message); }
    finally { setReviewBusy(false); }
  }, [reviewBusy, selectedReviewPath, session, setError]);

  const reviewAllAction = useCallback(async (action: ReviewAllAction) => {
    if (!session || !desktopAvailable || reviewBusy) return;
    setReviewBusy(true);
    try {
      const updated = await command('review_all_action', { sessionId: session.id, action });
      setReviewChanges(updated);
      setSelectedReviewPath(updated.files[0]?.path ?? null);
      setReviewDiff(null);
    } catch (error) { setError(normalizeError(error).message); }
    finally { setReviewBusy(false); }
  }, [reviewBusy, session, setError]);

  const selectReviewFile = useCallback((path: string) => {
    setSelectedReviewPath(path);
    setReviewDiff(null);
    setInspectorTab('diff');
    setInspectorOpen(true);
  }, []);

  function retryTask() {
    const lastUserPrompt = [...(session?.messages ?? [])].reverse().find(message => message.role === 'user')?.content;
    if (lastUserPrompt) void send(lastUserPrompt);
  }

  const paletteActions = useMemo<PaletteAction[]>(() => {
    const actions: PaletteAction[] = [
      { id: 'new-task', label: 'New task', detail: 'Start a task in the current project', group: 'Actions', icon: <Plus size={15} />, shortcut: 'Ctrl N', run: newTask },
      { id: 'open-project', label: 'Open project', detail: 'Choose a local project folder', group: 'Actions', icon: <FolderOpen size={15} />, run: () => void openFolder() },
      { id: 'create-project', label: 'Create project', detail: 'Create a new folder and register it as a project', group: 'Actions', icon: <FolderPlus size={15} />, run: () => setCreateProjectOpen(true) },
      { id: 'toggle-sidebar', label: sidebarCollapsed ? 'Show sidebar' : 'Hide sidebar', group: 'Actions', icon: <BriefcaseBusiness size={15} />, shortcut: 'Ctrl B', run: () => setSidebarCollapsed(value => !value) },
      { id: 'settings', label: 'Open accounts', detail: 'Manage providers and API keys', group: 'Navigation', icon: <Settings2 size={15} />, run: () => setView('providers') },
      { id: 'permissions', label: 'Manage permissions', detail: 'Set a permission mode and revoke project rules', group: 'Navigation', icon: <ShieldCheck size={15} />, run: () => setView('permissions') },
      { id: 'usage', label: 'View usage', group: 'Navigation', icon: <CircleDot size={15} />, run: () => setView('usage') },
      { id: 'overview', label: 'Show project overview', group: 'Navigation', icon: <FolderOpen size={15} />, run: () => setView('overview') },
      { id: 'files', label: 'Show changed files', group: 'Panels', icon: <FileCode2 size={15} />, run: () => { setView('agent'); setInspectorTab('changes'); setInspectorOpen(true); } },
      { id: 'diff', label: 'Show diff', group: 'Panels', icon: <ListChecks size={15} />, run: () => { setView('agent'); setInspectorTab('diff'); setInspectorOpen(true); } },
      { id: 'terminal', label: 'Show terminal activity', group: 'Panels', icon: <Terminal size={15} />, run: () => { setView('agent'); setInspectorTab('terminal'); setInspectorOpen(true); } },
      { id: 'context', label: 'Show task context', group: 'Panels', icon: <Command size={15} />, run: () => { setView('agent'); setInspectorTab('context'); setInspectorOpen(true); } },
      { id: 'task-info', label: 'Show task information', group: 'Panels', icon: <ArrowUpRight size={15} />, run: () => { setView('agent'); setInspectorTab('task'); setInspectorOpen(true); } },
      { id: 'theme', label: darkMode ? 'Switch to light appearance' : 'Switch to dark appearance', group: 'Appearance', icon: <CircleDot size={15} />, run: () => setDarkMode(value => !value) },
    ];
    for (const item of projects) actions.push({ id: `project-${item.id}`, label: item.name, detail: item.path, group: 'Projects', icon: <FolderOpen size={15} />, run: () => chooseProject(item.id) });
    for (const item of allSessions) actions.push({ id: `task-${item.id}`, label: item.title || 'New task', detail: item.archivedAt ? 'Archived task · resume' : ['queued', 'planning', 'working'].includes(item.status) ? 'Working' : 'Recent task', group: 'Tasks', icon: <CircleDot size={15} />, run: () => selectSession(item) });
    return actions;
  }, [allSessions, chooseProject, darkMode, newTask, openFolder, projects, selectSession, sidebarCollapsed]);

  useEffect(() => {
    function handleShortcuts(event: KeyboardEvent) {
      const commandKey = event.ctrlKey || event.metaKey;
      if (!commandKey || event.altKey || event.isComposing) return;
      const key = event.key.toLowerCase();
      if (key === 'k') { event.preventDefault(); setPaletteOpen(true); }
      else if (key === 'n') { event.preventDefault(); newTask(); }
      else if (key === 'b') { event.preventDefault(); setSidebarCollapsed(value => !value); }
    }
    function handleEscape() {
      setContextOpen(false);
      setMobileSidebarOpen(false);
      if (windowWidth < 1180) setInspectorOpen(false);
    }
    function handleEscapeKey(event: KeyboardEvent) { if (event.key === 'Escape') handleEscape(); }
    window.addEventListener('keydown', handleShortcuts);
    window.addEventListener('keydown', handleEscapeKey);
    return () => {
      window.removeEventListener('keydown', handleShortcuts);
      window.removeEventListener('keydown', handleEscapeKey);
    };
  }, [newTask, windowWidth]);

  if (desktop.loading) return <WorkspaceSkeleton />;
  if (!data) return <main className="startup-error"><div className="startup-error-card"><AlertCircle size={24} /><h1>JevCode couldn’t open</h1><p>{desktop.error ?? 'The local workspace service could not be reached.'}</p><button className="button-primary" onClick={desktop.retry}>Try again</button></div></main>;

  const isAgentView = view === 'agent';
  const isProjectView = view === 'overview';
  const activeTaskIsDemo = !!session && session.id === demoSession.id;
  const selectedProvider = data.providers.find(item => item.id === providerId);
  const compactClasses = [
    'workbench',
    sidebarCollapsed ? 'sidebar-is-collapsed' : '',
    inspectorOpen && isAgentView ? 'inspector-is-visible' : '',
    mobileSidebarOpen ? 'mobile-sidebar-is-open' : '',
    inspectorOpen && windowWidth < 1180 ? 'mobile-inspector-is-open' : '',
  ].filter(Boolean).join(' ');

  return <div className={compactClasses} style={{ '--sidebar-size': `${sidebarCollapsed ? 68 : sidebarWidth}px`, '--inspector-size': `${inspectorWidth}px` } as CSSProperties}>
    {(mobileSidebarOpen || (inspectorOpen && windowWidth < 1180)) && <button className="panel-backdrop" aria-label="Close open panel" onClick={() => { setMobileSidebarOpen(false); setInspectorOpen(false); }} />}
    <WorkspaceSidebar projects={projects} sessions={allSessions} projectId={projectId} sessionId={session?.id ?? null} view={view} provider={provider} collapsed={sidebarCollapsed} opening={opening} showArchived={showArchivedTasks} onShowArchived={() => setShowArchivedTasks(value => !value)} onCollapse={() => setSidebarCollapsed(value => !value)} onProject={chooseProject} onSession={selectSession} onRenameSession={item => void renameSession(item)} onArchiveSession={item => void toggleSessionArchive(item)} onDeleteSession={item => void deleteSession(item)} onDuplicateSession={item => void duplicateSession(item)} onForkSession={item => void forkSession(item)} onView={chooseView} onNew={newTask} onOpen={() => void openFolder()} onCreate={() => setCreateProjectOpen(true)} onRemoveRecent={item => void removeFromRecents(item)} onReveal={item => void revealProject(item)} onSearch={() => setPaletteOpen(true)} />
    {!sidebarCollapsed && <ResizeDivider label="Resize sidebar" width={sidebarWidth} min={210} max={340} onResize={setSidebarWidth} />}

    <main className="workbench-main">
      {isAgentView ? <WorkspaceHeader projects={projects} projectId={projectId} project={project} branch={branch} providers={data.providers} modelPreferences={data.modelPreferences} providerId={providerId} modelId={modelId} onProject={chooseProject} onModel={reference => void selectModel(reference)} onToggleFavorite={reference => void toggleModelFavorite(reference)} onSetDefault={reference => void setDefaultModel(reference)} session={session} modelLocked={!desktopAvailable || busy || sending || session?.status === 'waiting_for_user'} sidebarCollapsed={windowWidth < 840 ? !mobileSidebarOpen : sidebarCollapsed} onToggleSidebar={() => { if (windowWidth < 840) { setSidebarCollapsed(false); setMobileSidebarOpen(value => !value); } else setSidebarCollapsed(value => !value); }} inspectorOpen={inspectorOpen} onToggleInspector={() => setInspectorOpen(value => !value)} darkMode={darkMode} onToggleTheme={() => setDarkMode(value => !value)} onSearch={() => setPaletteOpen(true)} askReads={askReads} onAskReads={setAskReads} /> : <header className="settings-toolbar"><button className="quiet-icon-button toolbar-sidebar-toggle" onClick={() => { if (windowWidth < 840) { setSidebarCollapsed(false); setMobileSidebarOpen(true); } else setSidebarCollapsed(value => !value); }} aria-label="Toggle sidebar">{sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}</button><div><span>JevCode</span><strong>{isProjectView ? project?.name ?? 'Projects' : view === 'providers' ? 'Settings' : view === 'permissions' ? 'Permissions' : 'Usage'}</strong></div><button className="quiet-icon-button theme-toggle" onClick={() => setDarkMode(value => !value)} aria-label="Toggle appearance">{darkMode ? <Sun size={15} /> : <Moon size={15} />}</button></header>}
      {desktop.error && <div className="workspace-error-banner" role="alert"><AlertCircle size={15} /><span>{desktop.error}</span><button onClick={() => desktop.setError(null)} aria-label="Dismiss error">Dismiss</button></div>}

      {view === 'agent' ? <>
        <div className="workspace-content">
          <Conversation session={session} projectName={project?.name} demo={activeTaskIsDemo} review={reviewChanges} streamingText={session ? desktop.streaming[session.id] ?? '' : ''} toolOutput={desktop.toolOutput} onSuggestion={setDraft} onPermission={resolution => void resolvePermission(resolution)} onRetry={retryTask} onReviewFile={selectReviewFile} onReviewChanges={() => { setInspectorTab('changes'); setInspectorOpen(true); }} permissionBusy={permissionBusy} />
          <WorkspaceComposer draft={draft} setDraft={setDraft} providers={data.providers} modelPreferences={data.modelPreferences} providerId={providerId} modelId={modelId} onModel={reference => void selectModel(reference)} onToggleFavorite={reference => void toggleModelFavorite(reference)} onSetDefault={reference => void setDefaultModel(reference)} modelLocked={!desktopAvailable || busy || session?.status === 'waiting_for_user'} mode={mode} onMode={setMode} onSubmit={() => void send()} onStop={() => void stopTask()} onAttach={() => void attachFiles()} attachments={attachments} onRemoveAttachment={path => setAttachments(current => current.filter(item => item !== path))} busy={busy} sending={sending} hasProject={!!project} sessionLocked={!!session} desktopAllowed={desktopAvailable} providerConnected={!!selectedProvider?.connected} contextOpen={contextOpen} setContextOpen={setContextOpen} includeProject={includeProject} setIncludeProject={setIncludeProject} />
        </div>
        <footer className="workspace-statusbar"><span><span className={`status-indicator${busy ? ' is-busy' : ''}`} />{busy ? session?.status === 'waiting_for_permission' ? 'Waiting for approval' : session?.status === 'planning' ? 'Planning task' : 'Agent working' : session?.status === 'waiting_for_user' ? 'Waiting for your answer' : 'Ready'}</span><span className="status-readonly"><ShieldCheck size={12} />{permissionModeLabel(session?.permissionPolicy.mode ?? data.permissionPolicy.mode)}</span><span className="status-saved">Saved locally</span>{!activeTaskIsDemo && <span className="status-token-count">{tokens.toLocaleString()} tokens used</span>}</footer>
      </> : view === 'overview' ? project ? <ProjectOverview project={project} sessions={recentSessions} providers={data.providers} desktopAllowed={desktopAvailable} onStartTask={newTask} onSelectSession={selectSession} onProjectUpdated={updateProject} onError={desktop.setError} /> : <ProjectEmpty opening={opening} onOpen={() => void openFolder()} onCreate={() => setCreateProjectOpen(true)} /> : <div className="settings-content-scroll">{view === 'providers' ? <ProviderSettings providers={data.providers} accounts={data.accounts} onAccount={account => dispatch({ type: 'account', account })} onModels={updateModelCatalog} /> : view === 'permissions' ? <PermissionsSettings mode={data.permissionPolicy.mode} projects={projects} onMode={mode => desktop.dispatch({ type: 'permission-mode', mode })} onError={desktop.setError} /> : <UsageView records={desktop.usage} providers={data.providers} />}</div>}
    </main>

    {isAgentView && <>
      {inspectorOpen && <ResizeDivider label="Resize task details" width={inspectorWidth} min={276} max={480} reverse onResize={setInspectorWidth} />}
      <InspectorPanel project={project} branch={branch} session={session} provider={provider} demo={activeTaskIsDemo} review={reviewChanges} reviewDiff={reviewDiff} selectedPath={selectedReviewPath} reviewBusy={reviewBusy || busy} usage={desktop.usage} onSelectFile={selectReviewFile} onFileAction={(path, action) => void reviewFileAction(path, action)} onAllAction={action => void reviewAllAction(action)} open={inspectorOpen} onClose={() => setInspectorOpen(false)} tab={inspectorTab} onTab={setInspectorTab} />
    </>}
    <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} actions={paletteActions} />
    <CreateProjectDialog open={createProjectOpen} busy={creatingProject} onClose={() => setCreateProjectOpen(false)} onCreate={createProject} onOpenExisting={() => { setCreateProjectOpen(false); void openFolder(); }} />
  </div>;
}

function ResizeDivider({ label, width, min, max, reverse = false, onResize }: { label: string; width: number; min: number; max: number; reverse?: boolean; onResize: (width: number) => void }) {
  const drag = useMemo(() => ({ start: 0, width: 0, active: false }), []);
  const factor = reverse ? -1 : 1;
  return <div className={`pane-resize-divider${reverse ? ' is-reverse' : ''}`} role="separator" tabIndex={0} aria-orientation="vertical" aria-label={label} aria-valuemin={min} aria-valuemax={max} aria-valuenow={width}
    onPointerDown={event => { drag.start = event.clientX; drag.width = width; drag.active = true; event.currentTarget.setPointerCapture(event.pointerId); document.body.classList.add('is-resizing-pane'); }}
    onPointerMove={event => { if (drag.active) onResize(Math.max(min, Math.min(max, Math.round(drag.width + (event.clientX - drag.start) * factor)))); }}
    onPointerUp={() => { drag.active = false; document.body.classList.remove('is-resizing-pane'); }}
    onPointerCancel={() => { drag.active = false; document.body.classList.remove('is-resizing-pane'); }}
    onKeyDown={event => { if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') { event.preventDefault(); onResize(Math.max(min, Math.min(max, width + (event.key === 'ArrowRight' ? factor : -factor) * 12))); } }}
  />;
}

function WorkspaceSkeleton() {
  return <div className="workbench-loading" role="status" aria-label="Opening workspace"><aside><div className="skeleton-brand"><Brand /><span /></div><div className="skeleton-action" /><div className="skeleton-label" /><div className="skeleton-row" /><div className="skeleton-row short" /><div className="skeleton-label lower" /><div className="skeleton-row" /><div className="skeleton-row short" /></aside><main><div className="skeleton-toolbar"><span /><span /><span /></div><div className="skeleton-loading-copy"><span /><span /><span /></div><div className="skeleton-composer" /><small>Opening your local workspace…</small></main></div>;
}

function ProjectEmpty({ opening, onOpen, onCreate }: { opening: boolean; onOpen: () => void; onCreate: () => void }) {
  return <section className="project-empty-state">
    <div className="project-empty-mark"><FolderOpen size={20} /></div>
    <h1>Open a project to begin</h1>
    <p>Choose a local folder or Git repository. JevCode keeps project details on this device and reads files through project-scoped commands.</p>
    <div><button className="project-primary-action" onClick={onOpen} disabled={opening}><FolderOpen size={14} />{opening ? 'Opening folder…' : 'Open folder or repository'}</button><button className="project-secondary-action" onClick={onCreate}><FolderPlus size={14} />Create project</button></div>
  </section>;
}

function permissionModeLabel(mode: 'ask' | 'workspace_write' | 'full_access') {
  return mode === 'ask' ? 'Ask mode' : mode === 'workspace_write' ? 'Workspace Write' : 'Full Access';
}
