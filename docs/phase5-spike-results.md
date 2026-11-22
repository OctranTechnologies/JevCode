# Phase 5 — slice-0 spike results

**Status:** slices 0, 0b and 0c are complete as diagnostics; no product code, no enforcement, no repository changes. The design is unchanged except for a link from section 10.

Three spikes ran against `docs/phase5-containment-design.md` sections 3 and 7; results do not transfer between environments.

## Slice 0 — baseline, Windows 11 without admin

**Environment.** Windows 11 development build `10.0.26300.9974` (`CurrentBuild=26300`, `UBR=0x26f6`, display 26H2); the registry reports `ProductName = Windows 10 Home Single Language` and `EditionID = CoreSingleLanguage`. Process not elevated (`TokenElevation=false`, `TokenElevationTypeLimited (3)`), Medium integrity (`S-1-16-8192`), Developer Mode on, already inside a pre-existing outer job. Rust 1.95.0, `windows-sys` 0.61.2. No administrator rights used.

**Verified.**

- Job `KILL_ON_JOB_CLOSE | ACTIVE_PROCESS`, limit 4: the fourth spawn was denied with **1816 (0x718)**; closing the handle signalled all four members (`exit 0x0`, 4/4 gone). Control without the spike job: 6/6 spawns succeeded.
- Job memory 256 MiB: chunk 16 failed with **1455 (0x5AF)** after 240 MiB held; `PeakJobMemoryUsed = 269,811,712` bytes. Control: 512 MiB allocated.
- Breakaway: denied **5 (0x5)** without `BREAKAWAY_OK`; succeeded with `BREAKAWAY_OK`, and the child was outside the spike job.
- AppContainer with zero capabilities: profile create and delete returned `HRESULT 0x00000000` without admin; token `is_appcontainer=1`, Low integrity (`S-1-16-4096`). Write and read outside the granted folder failed **5 (0x5)**; reading `kernel32.dll` succeeded; external TCP failed **10013 (0x271D)**; loopback connect timed out at 2007 ms with no accept.
- `PROC_THREAD_ATTRIBUTE_JOB_LIST` plus `SECURITY_CAPABILITIES` in one `STARTUPINFOEX`: both attribute orders worked; the child was AppContainer and in the job; kill-on-close reaped it.
- Low-integrity restricted token: `CreateProcessAsUserW` succeeded without `SeAssignPrimaryTokenPrivilege` for tokens derived from the caller; writing above Low failed **5**, writing at Low succeeded; `CreateProcessWithTokenW` failed **1314 (0x522)**.

**Not verified.** Explicit WSA code or packet capture for the AppContainer loopback drop; `TokenAppContainerSid` readback (probe returned **122**, buffer too small); Developer Mode off; any elevated path; broad `ALL APPLICATION PACKAGES` read coverage (only `kernel32.dll` was read); low-integrity tokens not derived from the caller; a host not already inside a job; total CPU and handle caps.

**Design consequence.** Several section 10 unknowns are resolved for this build: nested jobs, both `STARTUPINFOEX` attributes, and same-user low-integrity launch work. The launcher's "attribute list fails, self-assign" fallback was not exercised. "Home" is covered only as Home Single Language on a development build, and non-nested hosts remain untested.

## Slice 0b — Edit profile, Windows 11 without admin

**Environment.** Same build and token state as slice 0. Fresh no-capability profile `jevcode.phase5.spike2`, SID `S-1-15-2-1205383247-…-1669796735`; every child was `is_appcontainer=1`, Low integrity, with only `SeChangeNotifyPrivilege` enabled. Grants stayed inside `spike2`; the profile was deleted with `HRESULT 0x00000000`; the final whole-tree SID scan found 0 matches.

**Verified.**

- Tool starts from install folders: `git` 2.55.0 **exit 0**, because that folder already carries `ALL APPLICATION PACKAGES:(RX)`; `node` v24.20.0, `cargo` 1.95.0 and `rustc` 1.95.0 all failed `CreateProcess` with **5**, and reads of their install folders also failed **5**. Full copies inside a granted folder ran (git **0**, cargo **0**, rustc **0**). An exe-only git copy exited **1** (`The system cannot find the path specified.`); a rustup proxy copy exited **1** (no default toolchain).
- Git config reads returned **128** (`failed to stat …: Permission denied`) through normal discovery because git stats ungranted ancestors; with explicit `GIT_DIR`/`GIT_WORK_TREE` they returned **0**.
- Direct writes into `.git` succeeded (`HEAD`, `refs`), but **no tested configuration produced a commit**: every commit exited **128**, `cannot lock ref 'HEAD': unable to create directory for …/.git/HEAD`, under modify (`M`), full control (`F`), `core.fscache=false`, a pre-seeded `HEAD`, and an existing repo with an outside seed commit. `git init` exited **1** at the same step, or **0** with a pre-seeded `HEAD`, which still did not commit.
- Removing the grant: all git verbs returned **128**; writes and reads failed **5**.
- Loopback: non-blocking immediate **10035**, `select` timeout, standard-library `TimedOut`; blocking **10060** at 21,037 ms; external **10013**.
- A three-level process tree kept one AppContainer SID and stayed in the job; kill-on-close signalled 3/3 with `exit 0x0`; post-close `OpenProcess` failed **87**.

**Not verified.** A successful commit; identity attribution inside a commit; `git init --bare` (the only attempt was invalid and exited **128** because `GIT_WORK_TREE` was set); a rustc-only copy; the packet-level loopback mechanism; grants on every ancestor to the drive root; Developer Mode off; install-folder grants for node, cargo or rustc; elevated behaviour.

**Design consequence.** AppContainer currently fails for Edit tasks on Windows. The write path works, but Git cannot commit at `HEAD`, so `git.metadata` write is not enforceable through this backend. Node, cargo and rustc also do not start without per-install ACEs, so `workspace.read` toolchain enumeration is an ACL problem, not only a list problem. The slice 6 "otherwise Blocked + Reduced" branch applies; D1 is exercised.

## Slice 0c — Linux containment, WSL2 Ubuntu

**Environment.** WSL2, kernel `6.18.58.1-microsoft-standard-WSL2`, x86_64, Ubuntu 26.04.1, uid 1000 non-root, no `sudo` and no installs. systemd present; AppArmor module loaded but `enabled = N`; securityfs not mounted; `/tmp` on tmpfs; `/proc/self/cgroup` = `/non-systemd`.

**Verified.**

- Unprivileged user namespaces: `unshare --user --map-root-user`, `--net`, `--pid` and `--mount` all exited **0**; uid 0 maps to 1000; namespace inodes differ inside and outside.
- bubblewrap 0.11.1 at `/usr/bin/bwrap`, not setuid; `--unshare-all` and `--unshare-all --unshare-net` exited **0**; writing to the read-only root failed **EROFS** (exit 2); no host file was created.
- Landlock syscall 444 reports **ABI 7**; self-restriction enforced: `/etc/hostname`, an outside create and `exec /bin/true` were denied **EACCES (13)**; the allowed path read and write succeeded.
- seccomp status only: `Seccomp: 0`, `Seccomp_filters: 0`, `NoNewPrivs: 0`; `CONFIG_SECCOMP_FILTER=y`.
- cgroup v2 mounted with `nsdelegate`; controllers `cpu memory pids`; `/non-systemd` is root-owned and not writable; `user@1000.service` is user-owned and writable, but `cgroup.kill` there is not writable.
- Parent death: child gone at **0.02 s**, grandchild **still alive at 2.02 s** (reparented, still in the old group). `kill(-pgid, SIGKILL)`: child and grandchild gone at **0.02 s**, both reaped.

**Not verified.** Desktop Ubuntu, Fedora or Arch and any AppArmor-enforcing host; `/sys/kernel/security/lsm` (absent); seccomp filter installation and syscall denial; Landlock network rules (ABI 4+), scopes and `ioctl_dev`; cgroup delegation in practice, `cgroup.kill`, the `pids` limit and N+1 denial; the full bubblewrap profile (workspace and temp binds, `--cap-drop ALL`, stdio, an actual connect denial); a descendant that calls `setsid`; Windows containment.

**Design consequence.** All three Linux primitives exist and are usable unprivileged on this WSL2 host, which supports bubblewrap primary and Landlock fallback. It does not verify the design's teardown story: bubblewrap group-kill and pid-namespace teardown need verification, and `cgroup.kill` is not demonstrably usable in the delegated subtree.

## WSL2 is not a desktop Linux test

The slice 0c kernel is Microsoft's `6.18.58.1-microsoft-standard-WSL2` with a WSL-specific config: AppArmor loaded but disabled, no `/sys/kernel/security` mount, `/tmp` on tmpfs, and a non-systemd cgroup for `wsl.exe` processes. A desktop Ubuntu, Fedora or Arch install has a different kernel build, a different AppArmor policy and a different cgroup layout. These results are **not a substitute for a desktop Ubuntu, Fedora or Arch test** and do not close the design's Linux unverified items.

## What this changes in the design

- **Section 3, Windows table and launcher.** AppContainer profile creation without admin is verified on this build, but AppContainer currently fails for Edit tasks: Git commit exits **128** at the `HEAD` lock. Windows Edit therefore becomes `Blocked` unless ReducedSafety is opted in, rather than contained. Toolchain reads need per-install ACEs; read the table's "unverified, unpackaged app" entry with the Edit failure. The self-assign fallback remains untested.
- **Section 3, Linux.** bubblewrap group-kill and pid-namespace teardown need verification; only same-process-group kill was observed. `cgroup.kill` was present but not writable in the delegated directory, so cgroup teardown and the N+1 process limit stay unverified. The AppArmor prerequisite is untouched: WSL2 ran with AppArmor disabled.
- **Section 2 and D6 limits.** The 64-process and 2 GiB defaults were not exercised; the spike used 4 processes and 256 MiB. The fourth-process error is **1816**, not an access-denied code, so the failure message must map it.
- **Section 7, slice-0 test plan.** Windows coverage is Windows 11 Home Single Language on a development build, not plain Home. No UDP, DNS, `fork N`, sleep/heartbeat, wall-clock or CPU-limit test ran, and slice 0c did not use the `test-probe` binary named in the plan. WSL2 coverage does not meet the "Ubuntu/Fedora/Arch without root" requirement.
- **Section 8, slice 6.** The conditional "AppContainer/LowBox plus workspace ACLs if the spike passes; otherwise Blocked + Reduced" now resolves to Blocked + Reduced for Windows Edit tasks on this build.
- **Section 9, D1.** D1 applies as recorded: per-project opt-in ReducedSafety remains the Windows fallback when a profile cannot be enforced. The spikes change none of D1–D6, the capability model or the coverage table.
- **Section 10.** This record is the linked evidence; individual unverified entries are now partly verified with the scope limits above. Untouched items include the manifest execution level, Tauri sidecar bundling, Defender/EDR interference, reboot behaviour and host:port network enforcement.

## Evidence

Source records: `C:\Users\KIIT\AppData\Local\Temp\claude\b592fe15c0085704\13ec2c1b-abb9-47ef-b758-dac763fc509f\scratchpad\phase5\` — `spike\results.md` (slice 0), `spike2\results.md` (0b), `spike3\results.md` (0c). Raw logs are listed in each; 0c's `/tmp` scratch was deleted.
