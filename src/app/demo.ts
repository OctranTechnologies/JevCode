import type { AgentSession, Project } from '../types/domain';

export const demoProject: Project = {
  id: 'sample-northstar',
  workspaceId: 'sample-workspace',
  name: 'Northstar',
  path: '/projects/northstar',
  repositoryRoot: '/projects/northstar',
  activeBranch: 'feat/path-safety',
  lastOpenedAt: '2026-10-09T09:12:00.000Z',
  projectInstructions: '',
  preferredModel: null,
  permissions: { mode: 'ask', readFiles: 'allow', git: 'allow', writeFiles: 'allow', shell: 'allow', externalFiles: 'ask', maxToolRounds: 8 },
  isRecent: true,
  createdAt: '2026-10-08T14:15:00.000Z',
};

const earlier = '2026-10-09T09:12:00.000Z';

export const demoSession: AgentSession = {
  id: 'sample-task',
  projectId: demoProject.id,
  providerId: 'preview',
  modelId: 'workspace-explorer',
  title: 'Centralize project path validation',
  status: 'completed',
  messages: [
    {
      id: 'sample-user', role: 'user',
      content: 'Centralize project path validation and cover the edge cases. Keep the current behavior for valid paths.',
      toolCalls: [], toolResult: null, createdAt: earlier, providerData: null,
    },
    {
      id: 'sample-assistant-1', role: 'assistant',
      content: 'I’ll trace the workspace boundary checks first, then compare the existing test coverage before proposing the smallest change.',
      toolCalls: [{ id: 'sample-call-1', name: 'list_directory', arguments: { path: 'src/workspaces', limit: 50 } }],
      toolResult: null, createdAt: '2026-10-09T09:12:08.000Z', providerData: null,
    },
    {
      id: 'sample-tool-1', role: 'tool', content: 'mod.rs\npaths.rs\nproject.rs', toolCalls: [],
      toolResult: { toolCallId: 'sample-call-1', name: 'list_directory', content: 'mod.rs\npaths.rs\nproject.rs', isError: false, durationMs: 84, structuredContent: null },
      createdAt: '2026-10-09T09:12:09.000Z', providerData: null,
    },
    {
      id: 'sample-assistant-2', role: 'assistant',
      content: 'The checks are spread across project opening and tool reads. I found the existing traversal and symlink cases, so I’m checking their shared assumptions.',
      toolCalls: [
        { id: 'sample-call-2', name: 'read_file', arguments: { path: 'src/workspaces/paths.rs' } },
        { id: 'sample-call-3', name: 'read_file', arguments: { path: 'tests/workspaces.rs' } },
      ],
      toolResult: null, createdAt: '2026-10-09T09:12:14.000Z', providerData: null,
    },
    {
      id: 'sample-tool-2', role: 'tool', content: 'Path checks were duplicated in three call sites. Symlink targets are already resolved before reads.', toolCalls: [],
      toolResult: { toolCallId: 'sample-call-2', name: 'read_file', content: 'Path checks were duplicated in three call sites. Symlink targets are already resolved before reads.', isError: false, durationMs: 52, structuredContent: null },
      createdAt: '2026-10-09T09:12:15.000Z', providerData: null,
    },
    {
      id: 'sample-tool-3', role: 'tool', content: 'Existing cases cover parent traversal, absolute paths, hidden directories, and symlink escapes.', toolCalls: [],
      toolResult: { toolCallId: 'sample-call-3', name: 'read_file', content: 'Existing cases cover parent traversal, absolute paths, hidden directories, and symlink escapes.', isError: false, durationMs: 43, structuredContent: null },
      createdAt: '2026-10-09T09:12:16.000Z', providerData: null,
    },
    {
      id: 'sample-assistant-final', role: 'assistant',
      content: 'The path checks now share one project-scoped resolver. Valid relative paths keep their current behavior; absolute paths, parent traversal, excluded directories, and symlink escapes stay blocked.\n\nThe example diff adds regression coverage for Windows separators and paths that resolve through a symlink. The sample test run is shown in the Terminal panel. No files were changed in this preview.',
      toolCalls: [], toolResult: null, createdAt: '2026-10-09T09:16:32.000Z', providerData: null,
    },
  ],
  permissionPolicy: { mode: 'ask', readFiles: 'allow', git: 'allow', writeFiles: 'allow', shell: 'allow', externalFiles: 'ask', maxToolRounds: 8 },
  pendingToolCall: null, pendingPermission: null, sessionPermissionGrants: [], oneTimePermissionGrants: [], pendingUserInput: null, queuedToolCalls: [],
  iterations: 0, toolCalls: 0, activityEvents: [],
  createdAt: earlier, updatedAt: '2026-10-09T09:16:32.000Z', error: null, toolRounds: 2,
  archivedAt: null, gitBranch: 'feat/path-safety', worktreePath: null,
  workingContext: { objective: 'Centralize project path validation and cover the edge cases. Keep the current behavior for valid paths.', decisions: [], repositoryFacts: [], implementationState: '', outstandingTasks: [], protectedInstructions: [], compactedThrough: null, compactedTurns: 0 },
  projectInstructionFiles: [],
};

export const demoChanges = [
  { path: 'src/workspaces/paths.rs', additions: 26, deletions: 8, state: 'modified' as const },
  { path: 'tests/workspaces.rs', additions: 42, deletions: 0, state: 'added' as const },
];

export const demoDiff = [
  { kind: 'hunk' as const, old: '', next: '@@ -18,12 +18,30 @@ pub fn resolve_project_path(root: &Path, input: &str) -> AppResult<PathBuf> {' },
  { kind: 'context' as const, old: '18', next: '18', text: '    let candidate = root.join(input);' },
  { kind: 'delete' as const, old: '19', next: '', text: '    reject_hidden_path(&candidate)?;' },
  { kind: 'delete' as const, old: '20', next: '', text: '    reject_parent_segments(input)?;' },
  { kind: 'add' as const, old: '', next: '19', text: '    let candidate = normalize_project_input(root, input)?;' },
  { kind: 'add' as const, old: '', next: '20', text: '    reject_excluded_path(&candidate)?;' },
  { kind: 'context' as const, old: '21', next: '21', text: '    let resolved = candidate.canonicalize()?;' },
  { kind: 'add' as const, old: '', next: '22', text: '    ensure_inside_project(root, &resolved)?;' },
  { kind: 'context' as const, old: '22', next: '23', text: '    Ok(resolved)' },
];

export const demoTerminal = `$ cargo test workspace_paths\n\nrunning 6 tests\ntest rejects_parent_traversal ... ok\ntest rejects_absolute_paths ... ok\ntest resolves_windows_separators ... ok\ntest rejects_symlink_escape ... ok\ntest allows_project_relative_file ... ok\ntest blocks_hidden_directories ... ok\n\ntest result: ok. 6 passed; 0 failed`;
