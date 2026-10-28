# JevCode + Jev: Product Roadmap

This document expands the researched ideas for making TypeSafe Jev a more useful
decision assistant inside JevCode. It is a proposal, not a statement that these
features have already been implemented.

## Product direction

Jev is most valuable when it makes small, bounded decisions throughout a coding
task. The selected coding provider remains responsible for writing explanations
and code. Jev can help decide what to inspect next, whether the task is supported
by evidence, or which relevant option deserves attention.

The existing Jev integration is optional and separate from coding providers. It
uses typed Choice, Score, and Noul questions. In Assisted mode, its output can
guide workflow and reorder eligible tool definitions; it cannot remove tools,
approve permissions, execute actions, or replace the agent runtime. Repository
file contents are not sent to Jev by the current integration. The roadmap below
preserves those boundaries.

### Principles for every feature

1. **Rust remains authoritative.** Jev may recommend; native code validates
   paths, permissions, command risk, budgets, cancellation, and task state.
2. **A recommendation is not evidence.** Show the underlying test result, diff,
   command result, or user decision separately from Jev's judgment.
3. **Use the least context needed.** Prefer compact descriptions, metadata, and
   locally gathered evidence. Ask before sending code snippets, diffs, or other
   repository content to Jev.
4. **Keep control visible.** Make Jev's mode, use, and effect understandable in
   task activity. Provide an easy way to disable it.
5. **Measure before steering.** Initially record recommendations in shadow mode
   and compare them with outcomes. Promote a feature to Assisted behavior only
   after it performs well on JevCode tasks.
6. **Bound the decision.** Use short option lists, explicit “none” or “ask the
   user” choices where appropriate, timeouts, request limits, and deterministic
   fallbacks.

## Proposed features

### 1. Completion checklist with evidence

**User problem:** Coding agents can say a task is complete even when a requested
behavior was never implemented or verified. A final paragraph alone does not
show which parts of the request are supported by the work.

**Experience:** At the end of a task, JevCode shows the original requirements as
a checklist. Each item can be marked:

- **Verified** — supported by a successful, relevant check or directly inspected
  change.
- **Partially verified** — some evidence exists, but it does not cover the whole
  requirement.
- **Not verified** — no relevant check was run or available.
- **Needs attention** — the evidence suggests the requirement may not be met.

Selecting an item reveals its evidence: changed file and diff, test name and
result, command exit status, or a clear reason no check was possible. For example,
“Move the workspace selector to the header” could link to the resulting UI code,
show that the old composer selector was removed, and report the frontend checks.

**Jev's role:** Match a requirement to a small set of candidate evidence records
and classify whether the evidence supports it. It may identify a gap or suggest
one more check. It does not invent test results or mark a deterministic check as
successful.

**Application's role:** Capture requirements and evidence from task events,
diffs, and actual check results. Define which checks count as verification, keep
the evidence links, and render “not run” distinctly from “passed.” The user can
correct the checklist.

**Safety and privacy:** Treat repository text and tool output as untrusted data.
Do not let instructions embedded in a file or test output change the task,
permissions, or evidence rules. Send only the bounded text needed for matching,
and make any content sharing policy explicit.

**How to evaluate it:** Measure missed requirements found in review, unsupported
completion claims, user corrections, and how often cited evidence is actually
relevant. A Jev confidence value is not a test pass.

### 2. Recovery when the agent is stuck

**User problem:** An agent can repeat a failed command, search the same terms, or
make edits without learning anything new. This wastes time and can make a simple
environment issue look like a coding problem.

**Experience:** JevCode notices concrete signs of a loop, such as the same
normalized command failing repeatedly, repeated searches with no new results, or
multiple tool calls without a meaningful change. The timeline explains the
reason for the recovery suggestion. Jev can recommend a bounded next step:

- inspect the environment or project setup;
- search for a different source of evidence;
- try a different, still-permitted approach;
- ask the user a specific question; or
- stop and report the blocker.

For example, if `python3` is unavailable, the agent should check known interpreter
commands or report the missing runtime instead of repeatedly running the same
test.

**Jev's role:** Choose among the recovery options using a compact summary of
recent actions and results. It can recommend asking the user when a decision
depends on their environment or intent.

**Application's role:** Detect repetition from structured events, cap retries,
enforce deadlines, and ensure each suggested action goes through the existing
tool and permission system. Keep cancellation available.

**How to evaluate it:** Track repeated failed calls per task, recovery success,
time to a useful next action, and unnecessary user interruptions. Compare with a
deterministic repetition detector alone before enabling Jev steering.

### 3. Scope drift warnings

**User problem:** A narrow request can expand into unrelated files or subsystems.
The user may only notice the expansion during diff review.

**Experience:** When the plan or changed-file set starts to extend beyond the
request, JevCode shows a small notice such as “This task is now touching provider
authentication, which was not part of the requested toolbar change.” The user
can continue, narrow the scope, or ask JevCode to explain why the extra work is
needed.

**Jev's role:** Compare a concise objective with candidate work items or changed
areas and score how directly each supports the request. Treat necessary adjacent
work differently from unrelated cleanup where possible.

**Application's role:** Keep the objective and actual paths/actions, show the
warning at a useful point, and require user approval before any action that
already requires approval. A scope warning itself does not grant or deny tool
access.

**How to evaluate it:** Measure unrelated changed lines caught before review,
warnings dismissed as false alarms, and user-approved scope expansions. Avoid
optimizing for “fewest files changed” when a correct fix genuinely spans several
areas.

### 4. Smarter test selection

**User problem:** Running every test after every small edit can be slow, while
running too few tests can miss regressions.

**Experience:** JevCode first uses local dependency and project metadata to build
a shortlist of tests likely to cover the changed files. Jev can rank that
shortlist or identify which test area best matches the change. The task shows
which tests ran and why. A final or release check can still run the full required
suite.

**Jev's role:** Rank or classify the candidate tests from compact descriptions
of the change and test purpose. It should be able to say that the shortlist is
insufficient or ambiguous.

**Application's role:** Discover candidates using deterministic project data,
run the selected tests, capture their real results, and broaden the run when
coverage is unclear, the change is high risk, or a selected test fails. Mandatory
checks are never skipped because Jev says they are unnecessary.

**How to evaluate it:** Compare test time saved with regressions missed against
the full-suite baseline. Include flaky tests and dependency changes in the
evaluation. Do not assume results from another codebase transfer to JevCode.

### 5. Better file and context selection

**User problem:** Long tasks can repeatedly load irrelevant files and bury
important project instructions in a large context.

**Experience:** Search and project structure produce a local shortlist of likely
relevant files, symbols, and prior task facts. Jev helps rank what the coding
agent should inspect next. The agent can fetch file contents through the normal
workspace-scoped tools when needed rather than loading the entire repository.

**Jev's role:** Rank a bounded list using short descriptions and the current
objective. It may recommend “inspect the project instructions” or “search for
callers” as a next step.

**Application's role:** Preserve user and project instructions, perform actual
searches locally, enforce workspace boundaries, and decide what content is sent
to each provider. Existing context compaction must keep critical decisions and
instructions intact.

**How to evaluate it:** Measure repeated file reads, irrelevant context, tokens
sent to the coding model, time to first useful edit, and task success. A smaller
prompt is not a success if it drops a required instruction.

### 6. Project memory with source checks

**User problem:** Useful conventions and decisions are rediscovered across tasks,
but a memory system can become stale or silently preserve a mistaken assumption.

**Experience:** JevCode can offer durable project notes such as “run the
frontend checks with `npm run check`” or “provider requests are implemented in
the native adapter.” Each note shows where it came from and when it was last
checked. Users can review, edit, pin, or delete notes. Notes whose source has
changed are visibly stale and are not treated as verified facts.

**Jev's role:** Suggest whether a concise fact or decision is likely to help
future tasks, and help categorize it. Jev should not create durable memory
without the app's configured user approval policy.

**Application's role:** Store notes locally with project scope, source reference,
creation/update time, and a source fingerprint where available. Re-check the
source before reusing a note and provide straightforward controls to inspect or
remove it. Keep instructions supplied by the user separate from inferred memory.

**Privacy:** Do not treat memory as model training or fine-tuning. Avoid storing
secrets, credentials, large code blocks, or unreviewed tool output. Any sharing
with Jev or another provider must follow the user's configured content policy.

**How to evaluate it:** Measure whether notes are reused, stale notes caught,
repeated discovery reduced, and user edits/deletions. Audit a sample for
incorrect or sensitive memories.

### 7. More relevant tools and MCP servers

**User problem:** As projects connect more MCP servers and tools, the agent may
see a long list of irrelevant actions, making tool choice slower and less clear.

**Experience:** JevCode filters the visible list to tools already enabled and
trusted for the current project, then orders a small relevant subset. The UI
continues to show which tools are connected and why. JevCode can explicitly
recommend no external tool when built-in project tools are sufficient.

**Jev's role:** Rank short tool descriptions against the task or choose a
category such as repository search, issue lookup, or documentation lookup. A
two-step shortlist and verification can reduce unnecessary full descriptions.

**Application's role:** Enforce server enablement, trust, per-project scope,
permissions, and audit logging. Jev cannot enable an MCP server, authorize a
tool, or bypass its approval prompt. The existing permission flow remains the
same for native and external tools.

**How to evaluate it:** Track irrelevant tool calls, user-denied calls, task
completion, and time spent discovering tools. Include adversarial or misleading
tool descriptions in security tests.

### 8. Optional model budget manager

**User problem:** Some tasks need stronger reasoning while routine inspections
or summaries may not. Users may want a choice between speed, quality, and cost.

**Experience:** The user defines an allowed model pool and an optional task
budget. JevCode may recommend a model for a phase such as planning, implementation,
or verification. The interface shows the chosen provider, model, and
authentication/billing route before a request. Users can lock a model or approve
a proposed switch.

**Jev's role:** Classify task difficulty or recommend one option from the
user-approved pool. It may suggest escalating after a bounded failure or for a
high-complexity task.

**Application's role:** Enforce the pool and budget, record actual model usage,
and follow existing model/provider selection behavior. It must not silently
switch between ChatGPT OAuth, API-key billing, Google Account, Gemini API key,
Vertex AI, or another billing route. No route changes unless the user configured
and approved them.

**How to evaluate it:** Compare cost, latency, completion quality, retry rate,
and user overrides against a fixed-model baseline. Do not promise savings or
quality improvements until measurements on JevCode workloads support them.

### 9. Coordination between simultaneous tasks

**User problem:** Two tasks can work in separate sessions but still touch the
same files, duplicate work, or conflict when their changes are applied.

**Experience:** Before edits, JevCode detects direct path overlap and shows
possible semantic overlap such as two tasks both changing provider selection.
It can suggest sequencing the work, using separate worktrees, or asking the user
which task should own the area. Active worktrees remain visible under the
project.

**Jev's role:** Compare concise task objectives and planned areas to identify
possible overlap that file paths alone miss. It can recommend coordination, not
declare a merge conflict impossible.

**Application's role:** Detect actual file collisions, preserve worktree
isolation, show Git status, and handle conflicts when applying changes. Any
locking or task ownership should be deterministic and visible. The user retains
control over merge, apply, and deletion.

**How to evaluate it:** Track overlapping edits caught before they occur,
conflicts during apply, duplicate work, and false overlap warnings.

### 10. Compare two approaches before choosing

**User problem:** A design or implementation choice may have multiple plausible
solutions, but comparing them from descriptions alone can miss compatibility or
test failures.

**Experience:** As an explicit, higher-cost option, JevCode runs two approaches
in isolated worktrees. It runs the same relevant checks for each and presents
the diffs, test outcomes, and a short comparison. The user chooses which result
to keep or asks for another iteration.

**Jev's role:** Help define a small rubric before implementation, such as scope,
compatibility, maintainability, and test evidence. After deterministic checks,
it can summarize how each candidate maps to the rubric and identify unresolved
tradeoffs.

**Application's role:** Create isolated worktrees, enforce separate task budgets,
run the checks, preserve both diffs, and require an explicit user choice before
applying or merging one. A higher Jev score is not permission to accept a
candidate.

**How to evaluate it:** Measure whether users select the better-supported
solution, added time/cost, quality of comparison, and how often neither option
is acceptable. Keep the feature opt-in because it can use substantially more
compute.

## Suggested implementation order

| Stage | Work | Why it comes here | Gate to continue |
| --- | --- | --- | --- |
| 0. Measure | Add a replayable decision-evaluation set, record resolved Jev model versions, and run new decisions in shadow mode. | New guidance should be compared against actual task outcomes before it changes behavior. | Reproducible baseline and privacy review for every new field sent to Jev. |
| 1. Trustworthy task completion | Build the completion checklist with evidence, then stuck recovery and scope warnings. | These improve user trust and reduce obvious wasted work without granting Jev new authority. | Better requirement coverage and fewer repeated failures without a rise in false alerts. |
| 2. Focus | Add file/context ranking, test selection, and tool/MCP relevance. | These use existing local search, test, and permission infrastructure. | Demonstrated reduction in irrelevant reads or test time with no meaningful regression increase. |
| 3. Reuse | Add source-checked project memory. | Memory quality depends on reliable task evidence and clear provenance. | Stale, incorrect, and sensitive memory tests pass; users can inspect and delete every item. |
| 4. Orchestration | Add user-configured model budgets, task overlap guidance, and competing approaches. | These can affect cost and parallel work, so they need strong evaluation and explicit controls. | Clear opt-in, route visibility, budget enforcement, isolation, and measurable benefit. |

## Evaluation and rollout requirements

Before a feature changes agent behavior, evaluate it on a small, versioned set
of representative JevCode tasks. Include successful tasks, failed tests,
missing tools, ambiguous requests, pre-existing user changes, permission
prompts, and project instructions. Compare at least:

- the current JevCode behavior with Jev disabled;
- Jev recommendations recorded but not acted upon (shadow mode); and
- the proposed assisted behavior with the same task inputs.

For each evaluation, record the Jev model identifier/version actually resolved,
the structured question and answer, the app's deterministic outcome, user
corrections, and the final task result. Do not store secrets or full repository
contents in an evaluation record. Keep Jev decision usage separate from coding
provider usage.

Confidence needs careful interpretation. For Choice questions it is derived
from the answer distribution; it is not a calibrated probability that code is
correct. Tune thresholds against JevCode's own data and keep a fallback for
ambiguous results. For Score questions, perform arithmetic and exact threshold
checks in Rust rather than relying on Jev for precise numeric work.

Jev model aliases can move over time, so log the resolved version and rerun the
evaluation set after model changes. Keep decision prompts compact and options
precise. The published Jev 1.13 jaggedness notes describe version-specific
limitations; they are useful reasons to test literal, numeric, long-context,
and adversarial cases, not proof that every limitation applies unchanged to
later Jev versions.

### Stage 0 foundation status

Stage 0 is implemented in the application:

- A versioned, synthetic four-case evaluation suite can be replayed from
  Settings → Decision assistance. Results are stored locally with expected and
  actual answer labels, match fields, usage, and the requested and resolved Jev
  model IDs.
- A Shadow decision mode records the full bounded suggestion set without
  injecting suggestions into the coding-model request or changing tool order.
- Jev usage history records the provider-returned model ID when present, making
  mutable aliases visible in the Usage page.
- The evaluation path accepts no prompt or project path from the renderer. Its
  fixtures contain no user or repository content. The privacy fields and limits
  are documented in [Jev evaluation data](jev-evaluation.md).

The feature is opt-in and does not include a benchmark claim. A user must run
the evaluation with a connected Jev account to produce live results; each run
uses four real Jev requests and may count toward TypeSafe usage. No live run was
performed as part of implementation.

## Research references

These sources informed the patterns above. Vendor cookbook results are examples
from their own experiments; they are not JevCode benchmark results.

- [TypeSafe: Jev for coding agents](https://docs.typesafe.ai/introduction/coding-agents) — Jev's role as a structured decision model within a coding agent.
- [TypeSafe: Jev primitives](https://docs.typesafe.ai/primitives) — Choice, Score, and Noul decision types.
- [TypeSafe: Confidence](https://docs.typesafe.ai/confidence) — interpreting confidence from answer distributions.
- [TypeSafe: Patterns](https://docs.typesafe.ai/patterns) — routing, fan-out, confidence handling, and composite decisions.
- [TypeSafe: Model catalog](https://docs.typesafe.ai/models) — model capabilities, context, and versioning information.
- [TypeSafe: Jev 1.13 jaggedness](https://docs.typesafe.ai/model-jaggedness/jev-1.13) — version-specific failure modes worth testing.
- [TypeSafe cookbook: Skill suggestion](https://docs.typesafe.ai/cookbooks/skill_suggestion) — two-stage tool/skill discovery and “none fit” handling.
- [TypeSafe cookbook: Reranking](https://docs.typesafe.ai/cookbooks/rerank_typesafe) — local shortlist followed by semantic ranking.
- [TypeSafe cookbook: Basic memory](https://docs.typesafe.ai/cookbooks/basic_memory) — deciding whether a fact is useful to retain.
- [TypeSafe cookbook: Citation check](https://docs.typesafe.ai/cookbooks/citation_check) — checking claims against candidate evidence.
- [Anthropic: Effective harnesses for long-running agents](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents) — structured progress, tests, and end-to-end verification.
- [Anthropic: Effective context engineering for AI agents](https://www.anthropic.com/research/effective-context-engineering-for-ai-agents) — keeping context focused and preserving useful state.
- [Meta Engineering: Predictive test selection](https://engineering.fb.com/2018/11/21/developer-tools/predictive-test-selection/) — test ranking based on historical change and test outcomes; results need local validation.
- [RouteLLM](https://github.com/lm-sys/RouteLLM) — evaluating model routing as a cost/quality tradeoff.
- [LangChain: Time travel](https://docs.langchain.com/oss/python/langgraph/frontend/time-travel) — checkpoint branching as inspiration for comparing alternatives; JevCode does not need to adopt LangGraph.

## First recommendation

Start with **completion evidence**, **stuck recovery**, and **scope drift
warnings**, all in shadow mode first. They improve confidence in the existing
workflow while leaving file access, permissions, commands, billing, and final
acceptance under deterministic application and user control.
