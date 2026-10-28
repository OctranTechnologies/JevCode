# Jev shadow mode and evaluation data

Stage 0 adds an opt-in Shadow mode and a replayable synthetic evaluation suite.
Shadow mode records Jev's full structured suggestions but never applies them to
the coding-model prompt, tool ordering, or task behavior.

## What is sent to TypeSafe

### A real task in Shadow mode

The Jev request contains:

- the latest user objective, capped at 3,000 characters;
- counts of loaded project-instruction files and known repository facts;
- up to eight recent activity-kind labels; and
- whether earlier user-approved task decisions exist.

It does **not** include project paths, file contents, diffs, command arguments,
credential values, or tool results. The user objective is still sent to
TypeSafe, as it is for the existing Advisory and Assisted modes. Keep Shadow
mode off if the user does not want that request sent to Jev.

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
keys; the user objective and repository content are not copied into the
decision-usage record.

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
- Evaluation fixtures are local, static, and bounded; the command accepts no
  caller-provided prompt or repository path.
- Evaluation is user-triggered, runs one suite at a time, and uses the selected
  model only. It never switches provider, model, API key, or billing route.
- A failed or unavailable evaluation is recorded per case when possible; it
  does not block coding tasks or alter permissions.
- Jev confidence and fixture matches are advisory measurements. Rust remains
  authoritative for permissions, workspace access, process execution,
  cancellation, and task state.
