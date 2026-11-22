# Phase 5 — command containment and task ownership

**Decisions recorded: D1 per-project opt-in ReducedSafety on Windows fallback; D2 Isolated worktree default; D3 no allow-once overlap bypass; D4 network grant per task; D5 per-task env grants for Git signing and SSH; D6 64 processes and 2 GiB default. Spikes 0, 0b and 0c have run; results are in docs/phase5-spike-results.md. Slice 1 is committed as 7f04e19. Slice 2 is in progress. No enforcement is active.**

Merges proposals A2 and B2; unresolved disagreements are in section 9.

## 1. Threat model

Untrusted: model-generated `run_command` arguments; project scripts, builds and tests; Git hooks, filters and helpers; stdio MCP servers; extension tools. Trusted: the signed-in user operating the project terminal.

Existing spawns keep the user's full OS authority. `run_program_streaming` (`src-tauri/src/tools/process.rs:80`) clears the environment and sets `kill_on_drop` on the direct child, but applies no filesystem, network or process-tree containment. Direct Git spawns (`src-tauri/src/git/mod.rs:10`, `:111`) do not clear the environment. `kill_on_drop` kills only the direct child; no job, process group or parent-death kill exists.

Harms containment must prevent:

- **H1** read secrets outside the workspace: `~/.ssh`, `~/.aws`, browser profiles, other checkouts, app data.
- **H2** persist outside: Startup, shell rc files, other repositories, global Git config, PATH/DLL-hijack locations.
- **H3** network egress: exfiltration, second-stage download, localhost services, cloud metadata.
- **H4** process-tree escape: double-fork/`setsid`, `CREATE_BREAKAWAY_FROM_JOB`, COM activation, orphans after cancel or crash.
- **H5** resource exhaustion: fork bombs, memory, CPU, handles, disk.
- **H6** helper bypass: hooks, `core.fsmonitor`, `filter.*`, `diff.external`, `textconv`, pager, `credential.helper`, MCP grandchildren.
- **H7** approval laundering: one approval becoming permanent, global, or a sandbox-off switch.
- **H8** data loss outside the workspace or corrupted Git state.

Non-goals: local-admin or kernel attackers, malicious signed toolchains, physical access, deliberate terminal use. COM/brokers and app-package reads are known gaps.

## 2. Capability model and grant rules

| Capability | Meaning |
|---|---|
| `workspace.read` | Task root plus enumerated read-only toolchains (system, Git, rustup/cargo, node/python, caches). Never `$HOME`, `%APPDATA%` or credential files wholesale. |
| `workspace.write` | Task root only. File tools still deny `.git` (`src-tauri/src/workspaces/mod.rs:171`). |
| `temp.write` | `app_data/tasks/<task_id>/tmp` only; exported as `TMP`, `TEMP`, `TMPDIR`. |
| `git.metadata` | Common Git dir and task worktree gitdir; read in Inspect, read/write in Edit. |
| `process.spawn` | One root child; descendants inherit containment and are not separately grantable. Job Objects and Landlock cannot express "no grandchildren". |
| `process.limit` | 64 live processes including the root, 2 GiB aggregate, 300 s wall, 120 s CPU and 1,024 fds per process where supported (Linux; Windows CPU unverified). |
| `network` | Outbound TCP, UDP and DNS, any destination. Host/port scope needs a proxy; deferred. |
| `env.inherit(keys)` | Named extra keys over the fixed allowlist (`src-tauri/src/tools/process.rs:153-180`). |
| `external.read(path)` / `external.write(path)` | One canonical absolute path; never a parent directory or symlink target. |
| `mcp.invoke(server, tool)` | One MCP tool call, already individually approved. |

Profiles are immutable per task:

- **Inspect** (Plan): `workspace.read`, `git.metadata` read, app-chosen `git`/`rg` spawn, `process.limit`. `run_command` and MCP are denied by `TaskMode::permits` (`src-tauri/src/domain.rs:288`).
- **Edit**: adds `workspace.write`, `temp.write`, `git.metadata` write. No network.
- **Edit + Network**: Edit plus `network` by grant.
- **ReducedSafety**: `process.limit` only; everything else explicitly unenforced. Opt-in, badged, never called contained.

Each capability is `Enforced`, `Reduced` or `Blocked`; a task is `Enforced` only when all required are, else `Blocked` unless reduced safety is on. Process-only is never called a sandbox.

An approval creates a grant, not a mode change:

```text
CapabilityGrant { id, task_id, capability, operation_fingerprint, target,
                  scope, uses, expires_at, granted_by }
```

`operation_fingerprint` is SHA-256 over the tool/argument fingerprint (`src-tauri/src/permissions.rs:153-160`) plus normalized argv, cwd, executable, workspace-root digest and task id.

- **Allow once** is one use bound to the fingerprint; **for this task** lasts until completion, cancel or restart; **always for project** requires the base profile, `can_always_allow` (`:132`) and no added capability, and suppresses prompts only.
- Grants are audited, inactive after restart; a cached prompt or stored `PermissionRule` never bypasses containment.

An approval does **not** change the workspace/temp root, fixed env allowlist, limits, `TaskMode::Plan`, `git.metadata`, MCP trust, or another task's claims or containment. `env.inherit` is a per-spawn grant, not an allowlist change. No approval disables containment; approval dialogs are host UI.

Read allowlisting is enumerated. Deny `.ssh`, `.aws`, `.config/gh`, `.git-credentials`, `.npmrc`, `.cargo/credentials.toml`, `.env*`. Filesystem tools run in-process; `search_files` falls back to the walker when `rg` or containment is unavailable (`src-tauri/src/tools/filesystem.rs:424`, `:558`).

## 3. Mechanism per platform

### Windows 11 without admin

| Candidate | No admin? | Workspace | Stdio / descendants | Cannot stop |
|---|---|---|---|---|
| Job object (`KILL_ON_JOB_CLOSE`, `ActiveProcessLimit`, `JOB_OBJECT_LIMIT_JOB_MEMORY`) | Yes (Home unverified) | None | Launcher joins first; pipes inherited; descendants in job | Files, network, COM, services, WMI |
| AppContainer/LowBox (no capability SIDs) | Unverified, unpackaged app | Package-SID ACEs on workspace/temp; toolchain ACEs unverified | Token inherited; no `internetClient` means no network | App-package reads, brokered COM. Loopback is blocked by default (WSA 10060), so a local server needs a designed exemption, which is unverified |
| Low-integrity restricted token | Unverified | ACEs plus low-IL label | Inherited | Allows reads; no network control; defense only |

A `jevcode-containment` launcher sidecar holds the Windows code. The app creates the named job and limits, then starts the launcher, which joins the job, applies the AppContainer token through a `STARTUPINFOEX` attribute list, starts the target with inherited stdio and waits. If the attribute list fails on Home, the launcher self-assigns before spawning; the attribute-list failure path is untested. The launcher hosts the `windows` crate, so the Tauri crate needs no new Win32 dependency (`src-tauri/Cargo.toml:20-55`). Kill-on-close covers cancel, exit and crash; breakaway is denied. COM/WMI/services remain a gap.

Windows job limits verified so far: process count and job memory (slice 0). Wall clock is not a job limit; the app enforces it with a timer. CPU-time and handle caps are unverified and stay `Reduced`. Network enforcement depends on AppContainer, since per-process firewall rules need admin (unverified).

### Linux without root

- **bubblewrap primary:** bind workspace rw, task temp rw, `/usr`, `/lib*`, `/bin`, `/etc` ro; `--unshare-all`, `--share-net` only with `network`, `--die-with-parent`, `--new-session`, `--cap-drop ALL`, `--proc`, `--dev`, `--clearenv` plus allowlisted `--setenv`. Stdio passes; descendants share namespaces. Needs unprivileged user namespaces and a permitting AppArmor policy (unverified). Cannot stop writes in rw binds or kernel bugs.
- **Landlock fallback:** unprivileged `no_new_privs`; rules in `pre_exec`: workspace/temp rw, enumerated toolchains and `/proc` ro, else denied. Irreversible and inherited. ABI 1 is 5.13; network ABI 4 is 6.7 (unverified). Cannot restrict UDP, DNS, AF_UNIX, open fds or `/proc`; TCP rules are port-based.
- **seccomp:** unprivileged with `no_new_privs`; inherited. When network is denied, deny `socket(AF_INET)`/`AF_INET6`, closing Landlock's UDP/DNS gap; also deny `ptrace`, `mount`, `unshare`, `bpf`, `io_uring_setup`, `keyctl`, `perf_event_open`. No path filtering; per-architecture.
- **cgroups v2:** the `user@1000.service` subtree is delegated on WSL2, but `cgroup.kill` was not writable there (slice 0c), so cgroup teardown is unverified. Without a working cgroup, process count is `Reduced`. Never `RLIMIT_NPROC`, which counts the UID.
- **`setsid` + `PDEATHSIG` + `kill(-pgid)`:** reliability only, not a boundary.

Order: bubblewrap, then Landlock + seccomp, else `Blocked`. `--die-with-parent` covers the sandbox's direct child on app crash. The parent-death signal does not reach grandchildren (slice 0c: a grandchild survived 2.02 s), and bubblewrap pid-namespace teardown of descendants is unverified.

### macOS

Out of scope. `sandbox-exec` is deprecated and unused. The unsupported-platform policy applies: agent command execution is `Blocked`; file tools and approvals still work.

## 4. Coverage table

Target state after slices 2–11; none is contained today. "Yes" means it passes the seam. Rust paths are under `src-tauri/src/`.

| Spawn site | Contained | Mechanism | Notes |
|---|---|---|---|
| `tools/process.rs:80` `run_program_streaming` (called by `run_program:69`) | Yes | Task profile | Single seam. |
| `tools/shell.rs:63` `run_command` | Yes | Edit profile; network by grant | 30 s at `:67`; arbitrary shell is never a sandbox. |
| `tools/repository.rs:20` `git_command` | Yes | `run_program`; Inspect | 12 s. |
| `tools/repository.rs:96` `git_status` → `git/mod.rs:111`; `:100` `git_branch` → `git/mod.rs:149` | Yes | Inspect | Fixed read-only arguments. |
| `tools/filesystem.rs:405` `search_files`; `:539` `search_text` | Yes | Inspect `rg` | Walker fallback `:424`, `:558`. |
| `git/mod.rs:10` `output` (callers `:43`, `:149`, `:174`, `:211`, `:222`) | Yes, per caller | Inspect except `switch_branch:211` (Edit) | No `env_clear` today. |
| `git/mod.rs:111` `status` | Yes | Inspect | Fixed argv. |
| `git/worktree.rs:234` create; `:280/:295/:319` inspect; `:358` diff; `:475` commit; `:530` apply; `:583` remove | Yes | Inspect or Edit; `git.metadata` | Hooks contained and announced. |
| `review.rs:618` `restore --staged` | Yes | Edit | Index write. |
| `review.rs:853` `diff --no-index` | Yes | Inspect | Two app temp files (`:849-850`). |
| `review.rs:1024` `add` | Yes | Edit | Index write. |
| `mcp.rs:780` `connect_server` stdio | Yes | Per-server profile | `env_clear:787`; project cwd `:788`, user scope neutral temp cwd; secrets `:813`; project scope read-only until trust; network only if declared. |
| HTTP MCP (`mcp.rs:839`) | n/a | No child; `reqwest` | Per-call approval. |
| `commands/mod.rs:1308` `run_project_terminal` | **No, by design** | `terminal_process:1360/1373/1380` | User-operated, project-rooted, absent from the tool registry (`tools/mod.rs:74-86`), registered at `lib.rs:129`. Label "runs as you, outside containment"; test no tool route. |
| `commands/mod.rs:1160/1166/1172`, `:1213/1215/1217`, `:3241/3247/3253`, `:3576/3582/3588` open/reveal/`rundll32` | No | Shell integration | User-initiated, not agent-launched. |
| Git hooks and helpers | Yes, inherited | Same job/namespace as Git parent | Read-only verbs get an empty hooks path, no fsmonitor/external diff/credential helper, `GIT_TERMINAL_PROMPT=0` and `GIT_ASKPASS=`; write verbs run hooks contained with a notice. No app hook system exists (`git/worktree.rs:477`). |
| Test-only Git: `git/mod.rs:265`, `worktree.rs:624`, `tools/mod.rs:497`, `workspaces/mod.rs:423`, `guidance.rs:864`, `review.rs:1190/1312/1413`, `commands/mod.rs:4333` | No | `#[cfg(test)]` | Keep. |

`.git/config` and `.gitattributes` are project-controlled, so config hardening is defense in depth; the OS boundary is the guarantee. Exact hook-disable behavior across Git versions is unverified.

## 5. Unsupported-platform policy and the process-only state

States: `Enforced`, `ReducedSafety { unsupported }`, `Blocked { reason }`, per task and per spawn. Reasons: `containment_unavailable`, `containment_profile_unsupported`, `containment_probe_failed`, `capability_denied`.

Default: **Block agent command execution** until the probe verifies a profile. In-process file reads and searches keep working; Git tools that need a child are disabled; never silently downgrade.

Process-only is `ReducedSafety`: process-tree limits, environment filtering, kill-on-close and no orphans. It does **not** prevent filesystem writes outside the workspace, credential reads or network access. It is off by default, enabled per project with a typed confirmation, and a machine-level switch disables it everywhere. Plan cannot use it. Approvals cannot enable it.

- Blocked: "Command execution is unavailable: JevCode cannot enforce this task's containment profile on this system (reason). File edits still work. **Run diagnostics** · **Containment settings**"
- Reduced: "**Reduced safety is on.** Commands can read and write anything your user account can reach and can use the network. JevCode is not containing them. Continue only for repositories you trust. **Turn off**"
- Badge: "Uncontained" in the task list and timeline, plus an activity event.

If containment cannot be created for a spawn, the spawn fails; it never runs uncontained. `rg` may fall back to the in-process walker.

Criterion 1 is claimable only after slice 6, criterion 2 after 7, criterion 3 after 10, criterion 4 after 12; until then do not claim criterion 1.

## 6. Task ownership and concurrency

New `task_path_claims` table: task, project, path, mode, branch, worktree, heartbeat, state. Claims derive from successful writes, not model text. `write_call_paths` (`src-tauri/src/agent/mod.rs:1667`) covers only `apply_patch`, `create_file`, `delete_file` and `move_file`; `session_write_paths` (`:1679`) aggregates session, pending and queued calls. `run_command` claims the whole checkout (`**`).

- **Direct:** one writer per checkout. The first write claims the path. An overlapping active claim blocks the write before execution, replacing the advisory warning at `agent/mod.rs:1242`, whose text says "does not block the edit" (`:1313`). Dialog: pause the other task, switch to Isolated, take over (moves the claim and blocks the other task), or cancel.
- **Concurrent writable tasks:** default to Isolated. The picker says "Another task owns this checkout" and defaults to a worktree. Direct remains for one task with an explicit "share this checkout" confirmation.
- **Isolated:** `task_branch()` (`src-tauri/src/commands/mod.rs:1731`) plus `app_data/task-workspaces/<session_id>` (`:1659`) gives a unique `jevcode/<slug>-<8 hex>` branch and worktree. The worktree is the claim. `worktree::inspect` (`src-tauri/src/git/worktree.rs:242`) supplies changed paths, conflicts, additions/deletions and an ahead count (`:319-326`); it has no behind count, so "base moved N commits" is new.
- **Display:** header and list show branch, base, mode, owner, changed paths and Enforced/Reduced/Blocked; the project panel shows conflicts. Two writers in one Direct checkout show "shared checkout — 2 writers".
- **Apply:** `worktree::apply` merges `--no-ff` (`git/worktree.rs:524-530`), requires a clean project tree (`:508-511`) and aborts on conflict (`:534-546`). Add a per-repository mutex, **Rebase onto target** (conflicts stay in the task worktree with **Abort rebase**), **Apply**, **Open workspace**, **Remove**. Two tasks touching one file: the second apply aborts and changes nothing; recover by rebase, resolve and apply. No force-push, no auto-resolve.
- **Restart/cancel:** claims expire with the heartbeat; dirty worktrees are never deleted. `recover_interrupted` (`src-tauri/src/persistence/mod.rs:1030`) resets only `Queued`, `Planning` and `Working` (`:1033-1035`), not `WaitingForPermission`/`WaitingForUser`; add both and an orphan sweep. `cancel_session` (`src-tauri/src/commands/mod.rs:2442`) signals cancellation; `kill_on_drop` kills only the direct child (`tools/process.rs:84`, `mcp.rs:786`).

## 7. Test plan

**Slice-0 spike.** A `test-probe` binary with modes: write/read outside, spawn detached, fork N, allocate, connect TCP/UDP/DNS, sleep/heartbeat. Run on Windows 11 Home as a standard user and Ubuntu/Fedora/Arch without root. Record AppContainer profile creation, `STARTUPINFOEX`/job-list, nested jobs, low-IL limits, bwrap/userns/AppArmor, Landlock ABI, seccomp per architecture and cgroup delegation. Output: diagnostics, per-platform pass/fail; CI without user namespaces reports "not verified", not pass.

**Unit:** profile resolution and immutability; grant fingerprint/argv/cwd/task binding; one-use, task-end and restart expiry; "always" cannot add a capability; env allowlist; path-overlap logic; claim transitions; unsupported-state resolution.

**Integration, per OS:** grandchild outlives parent; `setsid`/double-fork; Windows breakaway fails; cancel kills the whole job/cgroup; app killed leaves no descendant within 2 s; N+1th process denied; memory allocation fails without killing the app; TCP/UDP/DNS fail when denied and succeed only under grant.

**Paths:** symlink escape (existing Unix test `src-tauri/src/workspaces/mod.rs:553`); Windows junction; UNC; `\\?\C:\`; `\\.\`; drive-relative; `CON`/`NUL`/`COM1`; ADS; trailing dots/spaces; case collisions; non-existent tails.

**Environment:** no `OPENAI_API_KEY`, `GH_TOKEN`, `AWS_*`, `GIT_ASKPASS`; `APPDATA`/`LOCALAPPDATA` absent unless granted; MCP sees only its configured names; `TMPDIR` exported.

**Git:** `post-checkout`, `pre-commit`, malicious `pre-push`, `core.fsmonitor`, `diff.external`, `textconv`, `credential.helper`; assert contained, no outside write or network, hook notice. Plan: `git_status`/`git_branch` work contained; `run_command` denied.

**MCP:** stub writes outside, opens a socket and spawns a grandchild; all denied; server and grandchild die at task end. HTTP MCP stays per-call approved.

**Restart:** `WaitingForPermission` reset; orphan sweep; claims expire; dirty worktree survives.

**Manual checklist:** Windows 11 Home with and without admin (job limits, AppContainer, low-IL, Defender/EDR interference, reboot after a killed app); Ubuntu/Fedora/Arch without root (bwrap, Landlock ABI, cgroups); macOS blocked banner; approvals and file tools usable while blocked.

## 8. Implementation slices

D1–D6 refer to section 9.

0. **Spike and probe.** Diagnostics only; no product code, no enforcement. Its results answer D1.
1. **Types, inventory, CI guard.** `ContainmentProfile`, `CapabilityGrant`, enforcement states, spawn log; CI rejects new `Command::new` outside the containment module, the user terminal/openers and tests. No behavior change. **First implementation slice; committable alone.**
2. **Funnel spawns.** Route `tools/process.rs`, Git, review, filesystem and MCP through `containment::spawn` with a pass-through backend; parity tests for argv, env, cwd and stdio. No behavior change.
3. **Windows process containment.** Launcher, job limits, cancel, kill-on-close, restart cleanup. Commands stay Blocked; Reduced is opt-in. *Needs D1 and D6.*
4. **Linux process containment.** bubblewrap profile, mounts, netns, die-with-parent; fail closed when missing.
5. **Linux filesystem/network.** Landlock + seccomp + optional cgroups, or Blocked.
6. **Windows filesystem/network.** AppContainer/LowBox plus workspace ACLs if the spike passes; otherwise Blocked + Reduced. *Needs D1.*
7. **Capability grants.** Audit table, approval fields, restart invalidation, task capability panel, no global switch. *Needs D4.*
8. **Git and hooks.** Route direct spawns, curated env, hook policy, config hardening, hook notice. *Needs D5.*
9. **MCP.** Per-server profile, network flag, trust consent, child kill.
10. **Ownership.** Claims, blocking overlap, Isolated default, take-over, display. *Needs D2 and D3.*
11. **Apply/rebase.** Repository mutex, rebase, conflict recovery.
12. **Security suite and docs.** Cases in section 7, per-OS results, explicit "not verified" list.

Slices 0 and 1 change no enforcement behavior. Slice 0 ships no product code.

## 9. Decisions for the user

1. **Windows 11 Home without a working AppContainer.** (a) Block command execution; (b) offer per-project `ReducedSafety` with typed confirmation and a machine kill switch. **Recommend (b):** keeps the primary Windows machine usable without claiming filesystem or network containment. **Tradeoff:** commands still read secrets and use the network.
2. **Default workspace mode for new Agent tasks in a Git repository.** (a) Isolated worktree; (b) Direct. **Recommend (a):** gives task-to-branch ownership, avoids a Windows ACL journal on the user's checkout and makes concurrent writes safe. **Tradeoff:** every task pays worktree creation and an explicit apply/rebase step.
3. **Overlap bypass in a Direct checkout.** (a) No allow-once: pause, switch to Isolated, take over, or cancel; (b) a recorded allow-once. **Recommend (a):** a second writer is exactly the failure criterion 3 names. **Tradeoff:** two deliberate writers in one checkout must use Isolated.
4. **Network grant breadth.** (a) One command; (b) one task; (c) a project-wide rule. **Recommend (b)** with a prompt on first use: dependency installs, test servers and fetches usually span several commands. **Tradeoff:** broader than a command grant and must expire at task end; project-wide is hardest to revoke.
5. **Git signing and SSH environment.** (a) Strict allowlist; signing/SSH fail until named keys are granted; (b) inherit signing/SSH variables. **Recommend (a)** with `env.inherit(keys)` per task. **Tradeoff:** signed commits and SSH remotes fail on first use; (b) widens the environment and can leak agent sockets.
6. **Default resource limits.** (a) 64 processes / 2 GiB; (b) higher defaults. **Recommend (a)** with a task-level raise and a clear failure message. **Tradeoff:** large builds fail until raised; raising weakens the fork-bomb and memory-exhaustion guarantee.

## 10. Verified facts and unverified claims

**Spike results:** [Phase 5 spike results](phase5-spike-results.md) records slices 0, 0b and 0c.

**Verified in this checkout:**

- Spawn seam: `run_program_streaming` (`tools/process.rs:80`) sets `kill_on_drop` (`:84`), clears env (`:85`), pipes stdio, times out (`:117`) and kills only the direct child (`:122`); `run_program` delegates (`:69`). `run_command` (`tools/shell.rs:12`) scopes cwd (`:41-56`) and calls it at `:63` with 30 s (`:67`).
- Git: `git_command` uses `run_program` with 12 s (`tools/repository.rs:20`); `git_status` calls `git::status` (`:96`), `git_branch` calls `git::branch` (`:100`). `git::output` spawns at `git/mod.rs:10` without `env_clear`; `status` at `:111`; `branch`/`branches`/`switch_branch`/`changed_files` call it at `:149`, `:174`, `:211`, `:222`.
- Search: `rg` at `tools/filesystem.rs:405` and `:539`; walker fallbacks at `:424`, `:558`.
- Worktrees: `output` at `git/worktree.rs:1`, `:234`, `:280`, `:358`, `:475`, `:530`, `:583`; ahead count `:319-326`, no behind; `apply` uses `--no-ff` (`:524-530`), requires a clean tree (`:508-511`) and aborts conflicts (`:534-546`).
- Review: `restore --staged` at `review.rs:618`; `diff --no-index` at `:853` over temp files (`:849-850`); `add` at `:1024`.
- MCP: stdio spawn at `mcp.rs:780`, `kill_on_drop` `:786`, `env_clear` `:787`, project cwd `:788`, env list `:796-812`, secrets `:813-824`; HTTP uses `reqwest` from `:839`.
- Terminal: `run_project_terminal` (`commands/mod.rs:1284`, registered `lib.rs:129`) runs `powershell.exe` (`:1360`), `/bin/zsh` (`:1373`) or `/bin/sh` (`:1380`), inherits env, and is absent from the tool registry (`tools/mod.rs:74-86`).
- Modes: `WorkspaceMode` `domain.rs:246`, `TaskMode` `:255`, `permits` `:288`, `PermissionMode` `:1076`, `PermissionRule` `:1117`, `PermissionPolicy` `:1453`; `assess` `permissions.rs:28`, `can_always_allow` `:132`, `fingerprint` `:153-160`.
- Ownership: overlap warning `agent/mod.rs:1242`, text `:1313`; `write_call_paths` `:1667`, `session_write_paths` `:1679`; `task_branch` `commands/mod.rs:1731`, worktree path `:1659`; `recover_interrupted` `persistence/mod.rs:1030`, statuses `:1033-1035`; `resolve_permission` `commands/mod.rs:2224/2250/2258/2282`; `cancel_session` `:2442`.
- Paths: `scoped_path` `workspaces/mod.rs:136`, restricted names `:171`, symlink test `:553`.
- Misc: `custom_commands.rs` has no spawn; the only production `hook` is `git/worktree.rs:477`; `Cargo.toml` has no `windows`/`nix`/`libc` (`:48-55`).

**Unverified:**

- Windows 11 Home, no admin: AppContainer/LowBox profile creation for an unpackaged app; `STARTUPINFOEX`/`PROC_THREAD_ATTRIBUTE_JOB_LIST`; nested jobs; launcher self-assign; low-IL `CreateProcessAsUser` exception; default `ALL APPLICATION PACKAGES` read ACEs; loopback exemption; per-process firewall admin requirement; total CPU/handle caps; the manifest's execution level.
- Tauri 2 sidecar bundling and update behavior for `jevcode-containment`.
- Linux: unprivileged user namespaces and AppArmor; Landlock ABI; seccomp per architecture; cgroup v2 delegation for a GUI app; `PDEATHSIG` and bubblewrap descendants.
- Exact Git hook-disable behavior across versions; `rg` versus walker parity for every ignore rule.
- Host:port network enforcement without a proxy; portable no-admin disk quotas.
