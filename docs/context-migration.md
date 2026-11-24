# Context from other agents

People arriving from another coding agent usually bring instruction files,
rules, and skills with them. This page records what Octran Code honors, how the
**Context from other agents** panel on a project's overview works, and what is
still missing. Competitor behavior was checked against first-party
documentation on 15 November 2026; links are listed at the end.

## What Octran Code loads

When a task starts, Octran Code adds three sources to the system prompt:

1. The project instructions saved in the project's settings (SQLite, this device).
2. The root `AGENTS.md`, if it is a regular, UTF-8 file of 64 KiB or less.
3. The root `CLAUDE.md`, under the same conditions.

Nothing else is loaded. Symbolic links and larger files are skipped. A root
`AGENTS.md` or `CLAUDE.md` that is not UTF-8 stops new tasks from starting.

## The panel

The panel runs a read-only native scan (`scan_project_context`) and groups what
it finds:

| Group | Meaning |
| --- | --- |
| Loaded by Octran Code | Root `AGENTS.md`, root `CLAUDE.md`, and `.jevcode/mcp.json`. |
| Ready to add | Instruction and rule files Octran Code does not load, plus a skill index. |
| Added to project instructions | Copied earlier; flagged when the source file has changed since. |
| Not supported yet | Skills, custom commands, workflows, and agent settings files. |
| Skipped | Symbolic links, files over 64 KiB, non-UTF-8 text, and unsupported names. |

Detected sources:

| Agent | Files |
| --- | --- |
| Codex | `AGENTS.md`, `AGENTS.override.md`, nested `AGENTS.md`, `.agents/skills/*/SKILL.md`, `.codex/config.toml` (name only) |
| Claude Code | `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md`, nested `CLAUDE.md`, `.claude/rules/**/*.md`, `.claude/skills/*/SKILL.md`, `.claude/commands`, `.claude/settings*.json` and `.mcp.json` (names only) |
| Cursor | `.cursor/rules/**/*.mdc`, legacy `.cursorrules`, `AGENTS.md`, `.cursor/mcp.json` (name only) |
| Gemini CLI | `GEMINI.md`, nested `GEMINI.md`, `.gemini/skills`, `.agents/skills`, `.gemini/commands`, `.gemini/settings.json` (name only) |
| Antigravity | `GEMINI.md`, `.agents/AGENTS.md`, `.agents/GEMINI.md`, `.agents/rules/*.md`, legacy `.agent/rules`, `.agents/skills`, legacy `.agent/skills`, workflow folders |
| GitHub Copilot | `.github/copilot-instructions.md`, `.github/instructions/**/*.instructions.md`, `.vscode/mcp.json` (name only) |

Adding files (`import_project_context`) works like this:

- The renderer sends only item IDs. Rust scans again and takes the text from the
  file itself.
- The text is added to the project instructions inside
  `<!-- jevcode:import source="…" sha256="…" -->` markers. Adding the same
  source again replaces its block. Project files are never written.
- Frontmatter is removed. Rules that are scoped to some files or applied on
  demand in their own tool are marked with their original scope and a warning,
  because Octran Code applies them to every task.
- Files with a likely credential cannot be added. The panel shows the line
  numbers but not the values, and previews hide those lines. A loaded
  `AGENTS.md` or `CLAUDE.md` with a likely credential gets a warning, because
  it is sent to the provider with every task.
- Settings and MCP files from other agents are reported by name and never read.
- The skill index lists each skill's name, description, and `SKILL.md` path,
  and tells the agent to read a skill with the normal file tools when a task
  matches it. Skill scripts still need command approval, and `allowed-tools`
  metadata is not honored.
- Adding is blocked while the project settings have unsaved edits, and the
  result must stay within the 50,000-character instruction limit.
- `@path` imports in Claude Code or Gemini CLI files are listed as warnings.
  Octran Code does not expand them.

## Remaining gaps

| Gap | Impact | Notes |
| --- | --- | --- |
| Nested and directory-scoped instructions are not loaded natively | Monorepo guidance applies only when copied, and then to the whole project | Codex, Claude Code, Cursor, Copilot, and Gemini CLI all scope by directory |
| No file-scoped rules (`globs`, `paths`, `applyTo`) | Copied rules add context to every task | Needs a context-assembly hook based on the paths a task touches |
| No native skills | The index is a workaround that relies on the model reading files | Agent Skills is a shared format across Codex, Claude Code, Gemini CLI, and Antigravity |
| No `@path` import expansion | Claude Code and Gemini CLI users lose the text they import | Needs workspace-bounded resolution, a depth limit, and approval for external paths |
| No global (user-level) instructions | Personal preferences must be repeated per project | `~/.codex/AGENTS.md`, `~/.claude/CLAUDE.md`, `~/.gemini/GEMINI.md` |
| No MCP import from `.mcp.json` and similar files | MCP servers must be configured again by hand | Must keep the trust flow and route secrets to the OS credential store |
| No custom commands or workflows | Repeatable procedures do not carry over | Could map to skills once skills exist |
| No "show effective context" view at task start | Users cannot see the exact prompt text | The inspector lists sources, not their text |

## Proposed phases

**Phase 1: done in this change.** Discovery, honest status, safe copy into
project instructions, secret blocking, and a skill index.

**Phase 2: native loading options.** Per-project settings for which root files
to load (`GEMINI.md`, `AGENTS.override.md`, `.github/copilot-instructions.md`)
and how to handle both `AGENTS.md` and `CLAUDE.md` (load both, or prefer one,
as Claude Code does). Done when the panel's "Ready to add" items can be switched
to "Loaded" without copying, the session's loaded-file list matches, and
changes apply to new tasks only.

**Phase 3: scoped context.** Load nested `AGENTS.md` / `CLAUDE.md` / `GEMINI.md`
and glob-scoped rules when a task first reads or edits a path under them, record
each load as a task activity event, and keep them protected from compaction.
Done when a rule with `globs: src/**/*.ts` is added only after a matching file
is touched and the event shows which file triggered it.

**Phase 4: native Agent Skills.** Discover `.agents/skills` and `.claude/skills`,
put names and descriptions in context, load the body when the model asks for it,
and run scripts only through `run_command` with normal approval. Done when the
skill index is no longer needed and explicit invocation works from the composer.

**Phase 5: configuration import.** Preview MCP servers from `.mcp.json`,
`.cursor/mcp.json`, `.vscode/mcp.json`, and `.gemini/settings.json`, write them
to `.jevcode/mcp.json` or user scope only after confirmation, move secrets into
the credential store, and keep servers untrusted until trusted.

## Sources

- Codex AGENTS.md discovery: https://learn.chatgpt.com/docs/agent-configuration/agents-md
- Codex skills: https://learn.chatgpt.com/docs/build-skills
- Claude Code memory, rules, imports, and AGENTS.md: https://code.claude.com/docs/en/memory
- Claude Code skills: https://code.claude.com/docs/en/skills
- Cursor rules: https://cursor.com/docs/context/rules
- Gemini CLI context files: https://geminicli.com/docs/cli/gemini-md/
- Gemini CLI skills: https://geminicli.com/docs/cli/skills/
- Antigravity rules: https://antigravity.google/docs/rules
- Antigravity skills: https://antigravity.google/docs/skills
- GitHub Copilot repository instructions: https://docs.github.com/en/copilot/how-tos/configure-custom-instructions/add-repository-instructions
- T3 Code: https://github.com/pingdotgg/t3code
