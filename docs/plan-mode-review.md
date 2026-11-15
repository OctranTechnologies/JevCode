# JevCode inventory, comparison, and Plan safety implementation

Reviewed 2026-11-15 against local base `e03a707`. This report describes source present in this checkout, not a verified installer release. Competitor statements below come from first-party pages retrieved during this task; they are documentation claims, not independently exercised guarantees.

## 1. JevCode feature inventory

| Area | Present in source | Evidence / limits |
| --- | --- | --- |
| Desktop workspace | Tauri 2, React 19, projects, repository explorer, branch selection, command palette, attachments, terminal, settings | `README.md`, `src/app/App.tsx`, `src-tauri/src/commands/mod.rs`; Windows/Linux release pipeline, no macOS installer claim |
| Providers | OpenAI, Anthropic, Google Gemini, OpenCode Zen/Go; catalog, model favorites/default, model changes between runs | `src-tauri/src/providers`, `src/features/models`; route-specific authentication and quota |
| Accounts | Native credential storage, OpenAI ChatGPT OAuth/API key, Google OAuth/API key, account metadata | `src-tauri/src/auth`, `credentials`; Google route uses configured Cloud project, not Code Assist entitlement |
| Task lifecycle | SQLite sessions, streaming text/output, activity events, stop, follow-up, questions, archive, rename, duplicate/fork, interrupted-run recovery | `domain.rs`, `agent/mod.rs`, `persistence/mod.rs`, `commands/mod.rs` |
| Context | Repository instructions, project settings, user-authored memory notes and freshness, durable working context and compaction preserving original requests | `workspaces/mod.rs`, `agent/mod.rs`, `ProjectMemoryPanel.tsx`; instruction/skills migration UI is outside this branch |
| Inspection | Bounded file reads, multi-read, directory listing, filename/text searches, symbol/reference text lookup, metadata, project summary; Git status/diff/log/show/branches | `tools/mod.rs`; symbol lookup is text based, not demonstrated language-server semantics |
| Actions | Exact text patches, create/delete/move file, executable plus argument-array commands, MCP integrations | `tools`, `extensions`, `mcp`; commands run with user's OS authority |
| Permissions | Rust validation/path bounds, read/write/command/network/danger assessment, project policy, once/session/project grants, audit and trust for MCP | `permissions.rs`, `commands/mod.rs`, `tools/mod.rs`; approvals are not an OS sandbox |
| Review / isolation | Task-scoped baselines/checkpoints, diffs, accept/revert, direct or Git worktree tasks, worktree commit/apply/remove UI | `review.rs`, `git/worktree.rs`, `TaskWorktreesPanel.tsx`; direct tasks can overlap |
| Jev assistance | Optional separate advisory/shadow decision service; bounded recovery/scope hints, local focus ranking, overlap warning, synthetic evaluations | `decisions`, `docs/jev-roadmap.md`, `docs/jev-evaluation.md`; promotion gates and outcome-quality evidence remain open |
| Operations | Local usage and estimated cost, diagnostics, signed updater, release workflow | `usage`, settings components, `.github/workflows`; no demonstrated enterprise policy, remote orchestration, or PR babysitting product |
| Plan mode at base | Renderer appended a user-message instruction only; runtime did not receive task mode | Base `App.tsx` in `send()`, `CreateSession`, `AgentSession`; no capability barrier existed |

## 2. Sourced competitor comparison

| Product | Documented behavior | Implication for JevCode (inference) |
| --- | --- | --- |
| Codex | Separates OS-enforced sandbox capabilities from approval policy; read-only permits inspection and sandbox-contained commands, with escalation depending on policy. [Official security documentation](https://developers.openai.com/codex/agent-approvals-security) | A prompt alone is insufficient. This patch implements a narrower tool boundary; it does not provide Codex-style OS isolation. |
| Claude Code | Permission modes include Plan; it reads/explores without editing source, with read-only commands and, depending on availability/settings, classifier-reviewed commands or command approval prompts. Modes can change during a session. [Permission modes](https://code.claude.com/docs/en/permission-modes). Hooks cannot simply approve non-read-only file edits while still planning. [Permissions](https://code.claude.com/docs/en/permissions) | Do not equate every product's Plan label with identical semantics. JevCode deliberately blocks every arbitrary command/MCP call and requires a new Agent task. |
| Google Antigravity | Planning mode researches and creates implementation-plan artifacts/task groups. Artifact review pauses for review; `/plan` waits for approval, and the docs describe how prompts/review settings affect continuation. [Artifact review](https://antigravity.google/docs/artifact-review/) | Reviewable plan artifacts and an explicit implementation handoff are useful workflow gaps. The cited page does not establish a persisted read-only runtime guarantee. |
| T3 Code | First-party product page describes orchestrating multiple coding harnesses with existing credentials, switching models mid-thread, branch-based work, inline diff review, and generated draft/stack PR workflows. [Official product page](https://t3.codes/) | JevCode has native provider adapters and local task review; comparable harness orchestration and PR lifecycle features were not found. T3's page is product material, not proof of Plan enforcement. |
| Cursor | Plan mode asks questions, researches the codebase, creates a reviewable/editable plan, and offers a build action; plans can be saved to the workspace. [Plan mode documentation](https://cursor.com/docs/agent/plan-mode) | A durable plan artifact plus a deliberate new Agent-task handoff would improve JevCode. The cited page does not prove an OS sandbox or universal tool denial. |

No measured quality, speed, security superiority, or exhaustive feature parity is claimed. No undocumented competitor feature is treated as absent.

## 3. Prioritized gaps

1. **P0 — Plan capability boundary:** fixed in this branch. Previously a model could ignore the planning prompt and reach edits/commands/MCP through normal permissions.
2. **P0 — OS containment for Agent commands/MCP:** still open. Approval grants do not restrict subprocess filesystem/network authority. Design a platform-aware sandbox separately.
3. **P1 — Durable plan-to-implementation handoff:** still open. Current safe workflow is a new Agent task with the plan copied into its request; no automatic promotion of Plan permissions.
4. **P1 — Measured completion and recovery quality:** existing synthetic/shadow foundations need real evaluation and visible completion evidence before autonomy increases.
5. **P1 — Parallel task ownership:** overlap warning exists; semantic conflicts, budgets, branch integration policy, and concurrent command/MCP write attribution remain open.
6. **P2 — PR lifecycle, remote work, richer code intelligence:** source inspection did not establish integrated PR monitoring, remote harness orchestration, or semantic indexing comparable to broader agent products. Validate demand before expanding scope.

Project instruction/skills migration is intentionally reserved for Claude Opus's separate branch; no UI or migration work for that area was implemented here.

## 4. Phased roadmap and acceptance criteria

| Phase | Deliverable | Acceptance criteria |
| --- | --- | --- |
| A — this patch | Persisted immutable Plan task mode, native gates, clear UI | Mode survives save/load/follow-up/fork/copy; reads retain policy/path approval; edits, commands, MCP and unknown future tools fail before approval/grants; blocked calls receive tool errors; parallel candidates cannot bypass checks; UI explains new Agent task requirement. These are design criteria; behavioral verification was not run under this task's no-tests constraint. |
| B | Explicit plan artifact and new Agent-task handoff | Show editable plan and source task link; user action creates a distinct Agent session with no copied grants, queues, or pending calls; Plan session remains immutable/read-only. |
| C | Agent execution containment | Document Windows/Linux filesystem and network policy; define subprocess/MCP boundaries; surface unsupported environments; verify deny/allow behavior across restart, symlinks, hooks, child processes, and network access before release. |
| D | Reliable orchestration and delivery | Introduce task budgets and explicit write ownership; demonstrate conflict handling and recoverable cancellation; add reviewable PR creation/status only with explicit external-action authorization. |
| E | Outcome-driven expansion | Establish representative completion/recovery evaluations and privacy scope; publish measured evidence before promoting Jev assistance or adding remote/harness integrations. |

## 5. Implementation details

- `src-tauri/src/domain.rs`: `TaskMode::{Agent, Plan}` and explicit built-in inspection allowlist. New tool names default to blocked in Plan; mode is separate from permission policy and grants.
- `src-tauri/src/commands/mod.rs`: creation accepts persisted `taskMode`; duplicate/fork preserve it; permission resolution rejects blocked calls before assessment or grant mutation. There is no mode-update IPC.
- `src-tauri/src/agent/mod.rs`: Plan advertises only allowed built-ins and skips MCP discovery. Queue dispatch gates before discovery, approval assessment, grants, overlap review, and edit snapshots. Parallel candidates are gated individually and must have an automatic permission assessment; otherwise they stay queued for the full permission path. Both serial/streamed and parallel execution recheck the immutable copied mode before invoking extensions/tools. Allowed reads still use the existing policy, external-path checks, and approval flow. Plan skips the write-review baseline. A system notice is reconstructed on every provider turn, including after compaction/resume.
- `src-tauri/src/persistence/mod.rs`: every database session read recognizes the exact historical renderer Plan marker only when `taskMode` is absent. Other legacy tasks default to Agent. An old blocked pending permission is settled with error results and a visible failure notice, permitting a safe read-only follow-up. Recovery persists Plan metadata. Exact-marker inference is conservative: a manually quoted marker may classify a legacy task as Plan; an altered/removed marker cannot be recovered reliably.
- `src-tauri/src/tools/repository.rs`: Git diff/show disable text conversion as well as external diffs so read inspection cannot invoke configured conversion commands.
- `src/types/domain.ts`, `src/lib/schemas.ts`, `src/lib/ipc.ts`: typed mode crosses IPC; legacy frontend payloads default to Agent.
- `src/app/App.tsx`, `src/features/agent/WorkspaceComposer.tsx`: send mode at creation, render persisted mode, lock selector after creation/during send, show read-only badge and accessible notice explaining the new Agent-task requirement; remove the prompt-only control.
- `src/app/demo.ts`: sample session has explicit Agent mode.
- `src-tauri/src/git/worktree.rs`, `src-tauri/src/review.rs`, and existing runtime fixture constructors: existing literals initialized with Agent for source compatibility. No tests were added or run.

This is a model-tool runtime boundary. The user can still use independent desktop terminal/Git/review controls; app-data writes, provider requests, and explicitly requested worktree setup remain possible. Trusted Git/ripgrep executables and host environment remain assumptions. Plan does not protect a task from another process or Agent task writing to the same checkout. Previously executed side effects cannot be undone by migration.

## 6. Validation record

Passed: `npm run typecheck`, `npm run lint`, `npm run build`, `cargo check --manifest-path src-tauri/Cargo.toml --lib`, `cargo clippy --manifest-path src-tauri/Cargo.toml --lib -- -D warnings`, `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`, and `git diff --check`. The local commit SHA is provided in the implementation handoff.

Frontend dependencies were initially absent; installed with `npm ci --ignore-scripts`. npm reported 11 dependency vulnerabilities (10 moderate, 1 high); dependencies were not upgraded in this bounded patch. Vite reported dependency comment-annotation warnings and a bundle-size warning; build completed. No tests were added or run, and no PR or push is part of this task. A desktop behavioral exercise, runtime security exercise, and packaged application validation remain unperformed. Existing Rust fixture constructors were only updated for the required new field.
