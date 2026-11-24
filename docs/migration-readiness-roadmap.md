# Octran Code migration and parity roadmap

**Research checked:** 15 November 2026  
**Implementation status:** Progress is tracked in the status section at the end of this document. Work continues on the `improvement` branch.

## Product goal

Make it practical to move an existing codebase and its working habits into Octran Code without losing project guidance, reusable procedures, task safety, or reviewability. Octran Code should interoperate with other agents while preserving its local-first model: imported instructions are visible, imported credentials are never silently trusted, and planning cannot acquire write access by prompt wording alone.

Feature parity is not a goal by itself. Preserve Octran Code's useful differences—native provider adapters, local SQLite task history, OS-keychain credentials, task-scoped diff/revert, and optional Git worktrees—while closing the gaps that make migration confusing or unsafe.

## Current baseline

| Area | Available in source | Remaining limitation |
| --- | --- | --- |
| Providers and accounts | OpenAI, Anthropic, Gemini, and OpenCode Zen/Go adapters; native account and key handling | Octran Code invokes its own provider adapters; it is not yet a general launcher/control plane for installed coding-agent harnesses |
| Tasks | Local persistence, streaming, stop/follow-up, user questions, archive/rename/fork/duplicate, interrupted-run recovery | External transcripts and saved plans do not yet become first-class Octran Code task artifacts |
| Repository context | Project instructions plus root `AGENTS.md` and `CLAUDE.md`; the new Context from other agents panel identifies additional files and lets users review and copy selected content into project instructions | Imported scoped rules become project-wide; nested instructions and native skills are not resolved at runtime; no exact effective-context view |
| Tools and trust | Bounded file/Git tools, patch operations, shell commands, MCP, permission prompts/rules, project policy | Approval decisions do not contain subprocess filesystem or network authority; commands run with the user's OS authority |
| Review and isolation | Task change diffs, accept/revert, optional task worktrees, branch controls | Direct tasks can overlap in one checkout; there is no complete cross-task ownership/conflict workflow |
| Plan mode | Persisted immutable Plan task mode; Rust rejects edits, arbitrary commands, MCP and unknown tools before permission grants; the UI explains the new-Agent-task handoff | It is a tool boundary, not an OS sandbox. It does not stop the user or another process from changing the checkout, and the current handoff is not a durable plan artifact |
| Jev assistance | Optional advisory/shadow decisions and local recovery/scope hints | Outcome evidence and promotion gates need real task evaluations before autonomy expands |

The migration panel also warns about likely secrets, scopes, and unsupported files. It never writes imported content into a repository file, reads foreign settings/MCP files, or copies credentials. The skill index is a pointer for the model to read a `SKILL.md` through existing file tools, not native skill execution. Both feature commits have branch-reported type/lint/build/compile checks; no tests were added or run, and the desktop flow was not exercised end to end.

## Competitor findings and what they imply

| Product | Documented workflow | Roadmap implication |
| --- | --- | --- |
| Codex | Instruction discovery is layered from global guidance through the repository path, with override/fallback names. Its read-only setting still permits commands inside an OS sandbox; sandbox capability and approval policy are separate controls. [AGENTS.md discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md) · [approvals and security](https://developers.openai.com/codex/agent-approvals-security) | Implement native, path-aware context resolution. Keep approval prompts distinct from actual process containment. Octran Code's current Plan tool allowlist is narrower than Codex's read-only sandbox command surface. |
| Claude Code | `CLAUDE.md` and `AGENTS.md` can provide session guidance; `.claude/rules` scopes guidance to files. The `plan` permission mode reads and may permit classifier-approved commands when auto mode is available. [Memory and rules](https://code.claude.com/docs/en/memory) · [permission modes](https://code.claude.com/docs/en/permission-modes) | Support the file conventions without claiming identical mode semantics. Preserve scoped-rule metadata instead of flattening it into global instructions. |
| Google Antigravity | Planning mode researches the codebase, creates structured plan artifacts/task groups, and exposes an artifact review policy before implementation continues. [Artifact review](https://antigravity.google/docs/artifact-review) | Add durable reviewable plans and a deliberate implementation handoff. The cited docs establish a review workflow, not an OS-level security guarantee. |
| Cursor | Plan mode asks questions, researches the codebase, produces an editable plan, and lets the user choose when to build; plans can be saved to the workspace. [Plan mode](https://cursor.com/docs/agent/plan-mode) | Treat a plan as a reviewable artifact with source task, edits, and an explicit new Agent task—not as a prompt prefix. |
| T3 Code | The product page describes a control plane for Codex, Claude Code, Antigravity, OpenCode, Cursor and other harnesses; it advertises per-thread branches, subagents, remote machines, inline diff review and GitHub PR workflows. [T3 Code](https://t3.codes/) | Validate demand for harness orchestration, per-task ownership and delivery workflows. Adopt the useful workflow, but keep provider credentials and remote execution boundaries explicit. |
| Gemini CLI and GitHub Copilot | Their documented project context includes `GEMINI.md`/skills and repository instruction files, respectively. [Gemini context](https://geminicli.com/docs/cli/gemini-md/) · [Copilot repository instructions](https://docs.github.com/en/copilot/how-tos/configure-custom-instructions/add-repository-instructions) | Treat these formats as migration inputs, even before adding first-class harness adapters. |

These pages describe vendor-supported workflows. They are not independent tests of competitor security, reliability, or quality. The implications above are product inferences from those documented workflows and the current Octran Code source.

## Phased implementation plan

### Phase 0 — Make migration visible and planning actually read-only (implemented locally)

1. **Project context discovery:** identify common instruction/rule/skill/procedure files; label each as loaded, importable, imported, unsupported or skipped; show scope and warnings; preview before explicit import; re-scan natively at import time; block likely secrets; keep imports in Octran Code's own project settings.
2. **Plan capability boundary:** persist task mode; allow only a built-in read-only tool set; deny extension/MCP, edits, shell and unknown tools before normal approvals; recheck queued, resumed and parallel calls; retain existing read permissions; make the UI say that implementation starts in a new Agent task.

**Release gate:** add automated coverage for migration scanning/import and the Plan denial boundary, including symlinks, path traversal, secret masking, legacy sessions, pending approvals, parallel calls and unknown future tools. Exercise the actual desktop UI on supported Windows/Linux builds. Confirm no imported content or credentials are emitted in diagnostics. These gates are planned; they have not been run as part of this integration.

### Phase 1 — Resolve effective project context natively (P0 next)

**Deliverables**

- Introduce a versioned context resolver that distinguishes project root, opened folder and current task working directory.
- Let users choose precedence for root guidance (`AGENTS.override.md`, `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, Copilot instructions and configured fallbacks) instead of silently merging conflicting sources. Keep the existing behavior until the user changes the policy.
- Load nested instruction files and file-scoped rules only when they apply to a path. Preserve source path, rule metadata, and precedence in the task snapshot; never widen a scoped rule to the whole project without warning and confirmation.
- Add an **Effective context** inspector showing the exact sources and text sent for a new task, with byte/token budget, ignored files and reason, changed-since-task status, and secret warnings.

**Acceptance criteria**

- Opening a monorepo subfolder resolves only its ancestor guidance plus matching nested rules, in a documented stable order.
- A rule such as `src/**/*.ts` is absent for a docs-only task and present when a matching source path is touched; the activity log identifies the source rule.
- Symlinks and paths outside the chosen project boundary are never followed implicitly; oversized/invalid inputs remain visible as skipped.
- A running task uses its captured context snapshot; changing project settings affects new tasks only.

**Dependencies:** context source IDs and scanner from Phase 0; bounded prompt assembly and task activity persistence.

### Phase 2 — Native skills and safe procedure migration (P1)

**Deliverables**

- Implement Agent Skills-compatible discovery for project and user scopes, beginning with `.agents/skills` and `.claude/skills`, then adding documented Gemini/Antigravity locations.
- Index validated `name` and `description` metadata, load a skill body only when selected or relevant, and offer explicit skill search/invocation from the composer.
- Keep `allowed-tools` metadata informational until Octran Code has a separately reviewed capability model. Skill scripts run only through existing command permissions; reading a skill never grants tool access.
- Offer a preview-and-convert path for supported custom commands/workflows. Preserve source metadata and require confirmation for behavior changes; never execute imported command text automatically.

**Acceptance criteria**

- A skill is discoverable by name/description without adding every full skill body to every task prompt.
- Invoking a skill includes its source and version in the effective-context view; scripts and shell steps use the ordinary approval/containment path.
- Invalid frontmatter, duplicate names, symlinked files and oversized skills produce clear warnings without crashing a task.

**Dependencies:** Phase 1 context resolver and a safe, bounded on-demand file load.

### Phase 3 — Safe setup and portable project profiles (P1)

**Deliverables**

- Add a guided setup path that distinguishes Octran Code-native provider login from connecting an installed external harness. Detect availability; never assume a provider's CLI, IDE and app are interchangeable.
- Add versioned project profile export/import for instructions, model preferences and selected project settings. Exclude OAuth tokens, API keys, machine-specific paths, permission grants, and trusted MCP decisions.
- Add explicit MCP-config import for supported formats: parse to a preview, validate commands and environment-variable names, move secret values directly to the OS credential store when appropriate, and keep every imported server disabled/untrusted until user review.
- For unsupported credential formats, show which provider must be reconnected in Octran Code rather than attempting token extraction.

**Acceptance criteria**

- Exported profiles contain no secret values or reusable permission grants; importing on another machine never authenticates silently.
- An imported MCP server cannot run before the user reviews the command/environment and explicitly trusts it; secret values never appear in logs or previews.
- Duplicate/unknown fields and newer unsupported schema versions are rejected with actionable messages, while the current settings remain unchanged.

**Dependencies:** schema versioning, keychain-backed secret writes, and existing MCP trust UX.

### Phase 4 — Plans and task history move with the user (P1)

**Deliverables**

- Persist a first-class plan artifact linked to its read-only Plan task, separate from chat text; support edit, save, version history, review comments and export to Markdown.
- Add **Start Agent task from plan**. Create a new task with the approved plan, source-task link and chosen workspace; do not carry pending calls, one-time/session permission grants, MCP approvals or write authority from the Plan task.
- Support importing external Markdown/text transcripts as clearly marked reference context. Do not deserialize external tool calls into executable Octran Code history. Add safe export for Octran Code tasks, diffs and activity, omitting tokens, credential values and sensitive tool arguments.

**Acceptance criteria**

- A plan is still read-only after restart/compaction and cannot be promoted by changing its task mode.
- The new Agent task has a distinct ID and fresh permission state; its first message cites the plan artifact and source task.
- Imported transcripts are visibly labeled as external context and cannot create approvals or mutate the repository by themselves.

**Dependencies:** stable task-context snapshots, new-task creation, and the Phase 0 Plan mode boundary.

### Phase 5 — Contain agent processes and prevent cross-task collisions (P0 security, P2 workflow)

**Deliverables**

- Design OS-level command containment separately from approval policy. Define allowed workspace reads/writes, temporary directories, network policy, process tree limits, and how an explicit approval temporarily changes capabilities.
- Implement and document platform-specific enforcement for supported Windows/Linux targets; provide a clearly labeled reduced-safety mode or block execution when the platform cannot enforce the selected profile.
- Apply containment to subprocesses, hooks, Git helpers and MCP child processes. Keep read-only inspection and approval prompts usable without making arbitrary shell execution equivalent to a sandbox.
- Prefer a dedicated task worktree for concurrent/agentic writes; display task-to-branch ownership, changed paths, conflicts, and explicit apply/merge/rebase actions.

**Acceptance criteria**

- A malicious project script cannot write outside the configured workspace, spawn an uncontained child process, or use network access when those capabilities are denied.
- Approval records exactly which capability is granted and for what operation; it does not disable the sandbox globally.
- Two concurrent tasks cannot silently overwrite the same direct checkout; conflicts are surfaced with a recoverable resolution path.
- Security validation covers process descendants, symlinks, Git hooks/helpers, MCP, app restart, cancellation, and each supported OS. Unsupported platform behavior is explicit.

**Dependencies:** platform process/sandbox research, explicit tool capability model, and stable worktree lifecycle.

### Phase 6 — Multi-harness orchestration and delivery (P2)

**Deliverables**

- Keep model/provider adapters separate from harness adapters. Add a capability-based interface for launching and observing installed Codex, Claude Code, Antigravity and OpenCode harnesses where supported protocols permit it.
- Show the user which host, harness, model and branch own a task; support controlled model/harness switching only at safe turn boundaries.
- Add agent delegation with explicit child-task scope, branch/worktree ownership, cancellation, budgets and reviewable returned diffs.
- Add GitHub draft-PR creation/status as an opt-in workflow. Show the generated title/body/files and ask before push, PR creation, merge or any external write. Keep remote machine execution an explicit user-configured deployment option.

**Acceptance criteria**

- Missing executable/protocol/version is detected before task start, with a setup path and no silent fallback to a different harness.
- Every write-capable thread has a visible branch/worktree owner and a reviewable diff.
- Delegated changes return as isolated proposals; merging, pushing and publishing always require an explicit user action.
- User credentials remain on the configured host and no provider token is copied between harnesses.

**Dependencies:** Phase 5 containment and task ownership, stable adapters, and external GitHub authorization.

### Phase 7 — Prove quality before increasing autonomy (ongoing)

**Deliverables**

- Establish a privacy-reviewed evaluation set for context fidelity, task completion, recovery after compaction, permission correctness, Plan denials and worktree conflict resolution.
- Fix frontend test discovery so UI `.test.tsx` cases run; add native unit tests for scanner and runtime boundaries and desktop flow checks per supported OS.
- Record opt-in, local-first task outcomes and verification evidence; keep Jev advisory/shadow features behind promotion gates until improvements are measured.
- Add semantic navigation through LSP or tree-sitter only where it improves real tasks over current text-based search; label the capability accurately.

**Acceptance criteria**

- Every release candidate passes the defined behavior/security suite and its OS-specific smoke flow; reports distinguish test coverage from manual verification.
- Autonomy changes are tied to a published baseline and regression thresholds, not synthetic happy-path results alone.
- Users can inspect and delete local outcome records; external telemetry is opt-in and documented.

## Recommended order

1. **Before shipping the current increments:** add scanner/Plan-mode regression coverage and exercise the desktop flow on Windows and Linux.
2. **Next:** Phase 1 effective context, because migration stops being a one-time copy and becomes faithful as users open different folders and files.
3. **Then:** Phase 2 skills and Phase 3 portable profiles, the two highest-leverage ways to carry reusable behavior and settings across agents.
4. **In parallel planning, before broadening automation:** define Phase 5 containment and Phase 4 plan handoff. Containment must land before CLI harnesses or broader delegation can execute project-defined commands safely.
5. **After the safety/ownership foundation:** Phase 6 harness orchestration and PR delivery, then expand based on Phase 7 evidence.

## Success measures

- **First-task migration:** time from opening an existing repository to launching a task with the intended context; track how often files are skipped or copied and whether imported content is later edited.
- **Context fidelity:** proportion of fixture rules that resolve to the expected task/path scope, plus zero silent source omissions in the inspector.
- **Safe planning:** Plan tasks make no forbidden tool invocation across restart, queued/parallel calls and legacy-session recovery.
- **Portable setup:** exported/imported profile round-trips without secrets or permission grants; MCP imports remain untrusted until reviewed.
- **Workflow reliability:** conflict/recovery rate for concurrent tasks and successful, user-reviewed handoffs from plan to implementation.
- **Outcome quality:** task completion and verification rates measured on representative repositories before increasing Jev assistance or automatic execution.

## Status on `improvement`

Each item below was implemented and committed on its own. Rust tests, clippy, formatting, typecheck, ESLint, and Vitest passed for each. The new screens were not run in the desktop app; that check is listed under Phase 0.

| Phase | Status | What is in place | Not yet |
| --- | --- | --- | --- |
| 0 | Mostly done | Scanner tests (symlinks, secret blocking and masking, oversized and non-UTF-8 files, selection safety); one Plan denial rule (`TaskMode::permits`) on every execution path, tested; the `.test.tsx` files now run; runtime tests for approvals inside a batch of parallel calls, denial, and a gated call after automatic reads | Desktop flow checks on Windows and Linux |
| 1 | Done | Ancestor guidance inside the repository, outermost first; nested folder guidance and path-scoped rules (`.claude/rules`, `.cursor/rules`, `.github/instructions`) applied when a matching path is touched, with the activity log naming the rule; effective context view with budget, changes since start, and masked previews; `AGENTS.override.md` precedence as a project setting; an 80 KiB per-task guidance budget; a versioned resolver recorded with each task; a root guidance setting that chooses AGENTS.md, AGENTS.override.md, GEMINI.md, or the repository's Copilot instructions, and lists the other files as not used; isolated tasks read guidance from their own workspace | None. A worktree task still needs a manual check in the desktop app |
| 2 | Largely done | Agent Skills discovery in `.agents/skills`, `.claude/skills`, and `.gemini/skills`, with Gemini CLI and Antigravity user folders, project and user scope; validated names and descriptions; bodies read only on invocation, with source, version, and fingerprint recorded; `allowed-tools` shown as informational; a composer picker that searches when more than five skills are available; Antigravity skills read from parent folders up to the repository root; Claude Code and Antigravity commands previewed and converted into skills, with behavior changes listed, confirmation required, and no overwrites | Gemini CLI commands are listed as not converted, because Octran Code has no TOML parser yet |
| 3 | Largely done | Versioned profile export and import (instructions, preferred model, precedence); no credentials, grants, trust decisions, or machine paths; unknown and repeated fields rejected; newer versions refused; import previews and writes only on apply; MCP servers imported from `.mcp.json`, Cursor, VS Code, and Gemini CLI settings, saved disabled and untrusted, with literal values in the OS credential store; harness detection on PATH, with guidance on which sign-in belongs to which tool | Launching or connecting external harnesses (Phase 6); detecting IDE or desktop apps; VS Code files with comments are not read |
| 4 | Done | Plans saved from finished Plan tasks, edited with bounded history, exported to Markdown; "Start Agent task from plan" creates a fresh task that cites the plan and its source; a Task-tab Markdown export of the conversation, tool names, activity, and diff, without tool outputs, tool arguments, system instructions, or credential-like lines; review comments that travel with the plan's export and handoff; imported transcripts labeled as external reference, never parsed into history | Importing a transcript as structured history; transcripts are text only |
| 5 | Not started | Containment is designed in this document only | OS-level containment for subprocesses, hooks, Git helpers, and MCP children; task-to-branch ownership display |
| 6 | Not started | None | Harness adapters, delegation, and GitHub draft-PR workflow |
| 7 | Not started | Test discovery fixed | Evaluation set, desktop smoke flows per OS, outcome records |
