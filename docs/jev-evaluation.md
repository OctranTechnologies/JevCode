# Jev shadow mode and evaluation data

Stage 0 adds an opt-in Shadow mode and a replayable synthetic evaluation suite.
Stage 1 adds task completion evidence, a bounded recovery hint, and scope checks.
Stage 2 adds a local-only focus baseline for observed files, test-like files,
and tools already available to the task. Shadow suggestions can appear in the
task timeline, but they never steer coding actions, permissions, or file state.

## What is sent to TypeSafe

### Initial task classification

The Jev request contains:

- the latest user objective, capped at 3,000 characters;
- counts of loaded project-instruction files and known repository facts;
- up to eight recent activity-kind labels; and
- whether earlier user-approved task decisions exist.

It does **not** include project paths, file contents, diffs, command arguments,
credential values, or tool results. The user objective is still sent to
TypeSafe, as it is for the existing Advisory and Assisted modes. Keep Shadow
mode off if the user does not want that request sent to Jev.

### Stage 1 task signals in Shadow mode

Stage 1 makes up to five additional, bounded Jev requests per task:

- One completion review can include up to 8 extracted request items, up to 24
  task-scoped workspace-relative changed-file paths, a test-run count, and only
  the aggregate test outcome (`passed`, `failed`, `unknown`, or `not_run`). It
  does not include file contents, diff text, test names, or command output.
- Up to 3 scope checks can include the current objective and one candidate
  workspace-relative write path. Absolute paths and paths containing parent
  traversal are rejected locally.
- At most 1 recovery check can include the objective, a generic tool category
  (`command` or `search`), and a repeat count after the same failed invocation
  recurs. The command/search arguments and tool output are not sent.

The objective is capped at 3,000 characters, each checklist item at 700
characters, each stored path at 300 characters, and all counters are bounded.
Jev sees request text that the existing task mode already sends; Shadow mode
adds only the compact evidence fields above. Stage 1 adds up to five requests,
for a maximum of six Jev task requests total when counting the initial Shadow
evaluation. Requests can count toward TypeSafe usage. The task-local completion
receipt is still created if Jev is disconnected or unavailable.

### Stage 2 local focus baseline

After a successful repository search returns at least two candidate paths, the
runtime records one deterministic local baseline in the task timeline. It ranks
workspace-relative file paths using objective/path token overlap, identifies
test-like paths already discovered by the file tools, and ranks tools from the
current task's available tool definitions using objective keywords and prior
tool-call counts. The candidate paths come only from successful, bounded,
workspace-scoped `read_file`, `read_files`, `search_files`, `search_text`,
`find_symbol`, and `find_references` results. Existing tools continue to apply
ignore rules and workspace validation.

This baseline is not a Jev decision and does not change which files are read,
which tests run, or which tools the coding model can use. It does not grant or
modify tool permissions. The local task timeline stores the selected relative
path and tool label so users can compare it with actual task activity. No Stage
2 filename, path, tool name, description, or test candidate is sent to TypeSafe.
Jev-based ranking remains gated on explicit user authorization for those data
fields. This foundation alone does not demonstrate reduced reads, test time, or
regressions.

### Stage 3 local project-memory foundation

Project Overview lets the user create short project-scoped notes, choose a
category, attach a workspace-relative source file, edit, pin in the local list,
or delete. Native code rejects absolute paths, parent traversal, symlinked
sources, ignored/restricted files, and sources larger than 64 KiB. It stores a
SHA-256 fingerprint rather than source contents. Opening the panel rechecks the
fingerprint and marks notes current, stale, missing, or unavailable. The user
must explicitly refresh a changed source baseline after reviewing it. A
matching fingerprint establishes only that the source file is unchanged; it
does not prove that the note is accurate.

The note and source path are saved locally in SQLite. Source fingerprints stay
native and are omitted from the renderer response. Notes are not included in
task prompts and are not sent to Jev or coding providers in this stage. Pinning
only sorts the note to the top of the local list. The interface warns against
saving secrets, credentials, raw tool output, and large code blocks. Jev's
future memory suggestions and any remote reuse require a separate, explicit
content-sharing decision and evaluation before they are enabled.

### The synthetic evaluation suite

The Settings → Decision assistance **Run evaluation** action replays four fixed
examples from `src-tauri/src/decisions/evaluation_cases.json` against the
currently selected Jev model. Each request contains a synthetic objective,
zeroed context counts, and the same six typed questions used by Assisted mode.
No active task, project, account metadata, or repository information is read or
sent. These are real TypeSafe requests and may count toward the user's Jev API
usage.

The suite is versioned as `jevcode-stage0-v1`. Edit the fixture cases and
increment `version` together when changing expected behavior, so results from
different baselines are distinguishable.

## What is stored locally

Normal Jev decision history records the configured model ID, the model ID
returned by TypeSafe when provided, token counts, latency, result status, and
the bounded structured choice summary. A boolean identifies Shadow-mode
records. Existing session/project identifiers remain the local correlation
keys; task text, file paths, and repository content are not copied into the
decision-usage record. The completion receipt and its task-scoped paths are
stored inside the local task record so the UI can display them after restart.

Evaluation results are stored in SQLite separately from coding-provider usage.
Each result contains the suite version, run and case IDs, case title, requested
and resolved model IDs, expected and actual answer labels, matched/mismatched
fields, token counts, latency, failure code, and timestamp. The request fixture
itself is shipped with the application, so result history does not duplicate
the synthetic objective text. No Jev API key or credential is included.

The history query returns the latest 100 evaluation case results. Older rows
remain in the local database; there is no evaluation-specific retention policy
yet. Usage and evaluation history remain on the device and are not submitted
as a separate telemetry feed.

## How to read the baseline

The suite covers four coarse cases: a small implementation, a test blocked by a
missing runtime, an explanation-only request, and a cross-cutting authentication
change. Each case checks expected task category, first workflow phase, and tool
focus. A match means only that the selected answer was in the fixture's
acceptable set. It is not a measure of code quality, task success, correctness,
or a calibrated confidence score.

TypeSafe's resolved model ID is stored separately from the selected alias so
results can be compared across model updates. Re-run the same suite after
changing the selected model or when the resolved version changes. Do not infer
quality improvements from one run; compare a set of representative JevCode
tasks and actual outcomes before enabling any new assisted behavior.

## Safety boundaries

- Shadow answers never reach the coding provider and never steer tools.
- The completion checklist labels Jev output as a tentative match, separately
  from locally captured file and test evidence. It never infers a passed test.
- Recovery suggestions are one-shot and informational. They do not retry,
  execute a tool, stop a task, or ask a question automatically.
- Scope warnings never block, approve, or revert an edit. Existing permission
  checks still run in their normal order.
- Evaluation fixtures are local, static, and bounded; the command accepts no
  caller-provided prompt or repository path.
- Evaluation is user-triggered, runs one suite at a time, and uses the selected
  model only. It never switches provider, model, API key, or billing route.
- A failed or unavailable evaluation is recorded per case when possible; it
  does not block coding tasks or alter permissions.
- Jev confidence and fixture matches are advisory measurements. Rust remains
  authoritative for permissions, workspace access, process execution,
  cancellation, and task state.
