# Brag Plan: JevCode

## What is this app?
A local-first desktop coding agent (Tauri + Rust + React) that keeps every action visible and every change reviewable — plus **Jev**, a separate decision model that judges the agent's work but cannot execute tools or grant permissions.

## The angle
The audience already uses AI coding agents. They've all seen an agent cheerfully announce "Task completed." JevCode's flex is that it doesn't take the agent's word for it: a second model checks requirements against real evidence, notices when the agent is stuck, drifting, or colliding with another task — and has zero authority to touch anything. "Jev recommends. Rust decides." The video treats this as quiet, earned confidence, not hype.

No competitor names. The contrast is implied by "Task completed." → "Says who?"

## Hook (first 2-3 seconds)
Dark canvas. An agent message types out the exact string JevCode's runtime writes: **"Task completed."** A beat. Then, in JevCode orange, large: **"Says who?"**

## Key moments (the middle)
- The **Task completion checklist** card (real `CompletionReviewCard` UI): three request items reveal one by one with real Jev labels — `Jev: possible match`, `Jev: possible match`, `Jev: no match` — then the evidence rows: *Files changed* and *Test runs: passed (exit 0)*. The "no match" row is the catch.
- Three real task-timeline signals stack up, each with a one-word chip: **STUCK** (recovery hint), **DRIFTING** (scope warning), **COLLIDING** (task overlap). Real runtime copy, trimmed.
- The authority line: **"Jev recommends. Rust decides."** with chips *Can't execute tools · Can't grant permissions · Shadow mode first*.

## Outro / punchline
JevCode mark + wordmark, tagline "Visible actions. Reviewable changes.", and a quiet row: *5 providers · Per-file keep or revert · Isolated Git worktrees*. Lands on the 22.93s strong cue.

## User flow worth showing
Agent says done → completion checklist appears with Jev's labels and local evidence → user spots the unmatched item. Timeline warnings are the mid-task flow (agent loops / edits off-scope / another task touches the same file).

## Tone
- Preset: polished
- Creative direction: "quiet flex for people who already use coding agents — the product that checks its own homework"
- Interpretation: Restrained motion, long-enough holds, mixed case, dark UI-native look. Confidence comes from showing the real UI and real copy, not from big claims. One sharp line ("Says who?") carries the attitude.

## Format: landscape — 1920x1080
## Duration: 23.5s

## Visual identity (from the project)
- Background: `#0b0b0c` (--wb-canvas); surfaces `#131315` / `#16161a`; borders `#2a2a30`
- Accent: `#ff5e1a` (--wb-accent) — JevCode orange
- Jev / governance: `#a5b4fc` (--wb-governance) — indigo used for Jev signals
- Text: `#ededee` (ink), `#aeb2bb` (muted)
- Status: success `#6ee7b7`, warning `#fcd34d`, danger `#fda4af`
- Display/body font: Inter (Inter Variable in app); mono: Cascadia Code / Consolas fallback
- Strongest visual element: the orange radial asterisk logo (favicon.svg path) and the dark workbench cards with mono tool labels

## Share copy (draft)
Every coding agent says "Task completed." JevCode asks a second model to check — against real diffs and test results — and that model can't touch a single file.

## Audio direction
- Role: cinematic support / polished bed
- Music: `happy-beats-business-moves-vol-12-by-ende-dot-app.mp3` (steady, clean)
- Music treatment: start at 0, ~0.3 volume, fade in over first 0.6s, fade out over last ~1s after logo lands
- Music cue guidance: bundled preset `assets/music/cues/happy-beats-business-moves-vol-12-by-ende-dot-app.music-cues.json`, ~110 BPM. Strong cues: 8.74s (checklist "no match" catch), 17.47s ("Rust decides."), 22.93s (final logo). Beat grid for timeline rows: use every other beat (~1.1s spacing) around 11.46 / 12.55 / 13.64.
- Audio-reactive treatment: subtle; music RMS breathes the orange logo glow and a faint card-border glow. No waveforms.
- SFX posture: sparse, motion-matched
- Audio-coupled moments: hook typing (soft keypresses), "Says who?" (soft impact), checklist rows (soft drops), timeline rows (soft clicks), "Rust decides." (soft impact), logo (bell)
- Restraint rule: never more than one SFX per visual event; nothing bright or glitchy.

## Storyboard

### Scene 1 — Hook — 3.3s (0.0–3.3)
Agent message row (orange asterisk avatar, "JevCode" label) types "Task completed." in body text. At ~1.6s "Says who?" lands large in orange below/center.
Sequential/interaction: yes — typed text, then slam-in line. "Says who?" held ≥1.2s.
Audio intent: curiosity, then a small sting.
Audio-coupled idea: keypress per character; soft impact on "Says who?".
Transition mood: clean → Scene 2

### Scene 2 — Reveal — 3.1s (3.3–6.4)
Orange asterisk logo draws its five strokes, "JevCode" wordmark, then: "A local-first desktop coding agent" and second line "with a second model that checks its work." (first line verbatim README; second line is framing grounded in Jev docs).
Sequential/interaction: logo stroke draw.
Audio intent: confident arrival.
Transition mood: soft slide → Scene 3

### Scene 3 — Completion checklist — 4.8s (6.4–11.2)
Recreated "Task completion checklist" card (sample data). Three items reveal ~0.6s apart then hold:
1. `Jev: possible match` — Move the workspace selector to the header
2. `Jev: possible match` — Remove the old composer selector
3. `Jev: no match` — Show the selected mode in the status bar  ← lands on 8.74s, highlighted
Evidence block: Files changed `src/features/workspaces/WorkspaceHeader.tsx` · Test runs `npm test: passed (exit 0)`.
Side kicker: "Done means evidence."
Footer disclaimer (small, real copy): "Jev's shadow labels are suggestions, not verification."
Sequential/interaction: yes — rows one by one, then hold full card ≥1.8s.
Audio intent: methodical, then the catch.
Transition mood: clean wipe → Scene 4

### Scene 4 — Timeline signals — 5.0s (11.2–16.2)
Header: "Watching the run, not just the result." Three activity rows with indigo dot + chip:
- STUCK — "Jev shadow recovery hint: inspect the environment or project setup after 3 repeated failed command actions."
- DRIFTING — "Jev shadow warning: src/auth/oauth.rs may be outside this task's scope."
- COLLIDING (warning-colored triangle) — "Possible task overlap: these tasks share the same project checkout."
Each row ends with a muted "No action was taken automatically." style tag on the first only (real copy). Rows reveal ~1.1s apart; full set holds ≥1.5s.
Audio intent: alert but calm.
Transition mood: soft → Scene 5

### Scene 5 — Authority — 3.6s (16.2–19.8)
"Jev recommends." (indigo) then "Rust decides." (orange) at 17.47s. Chips below: Can't execute tools · Can't grant permissions · Shadow mode first.
Audio intent: the thesis — a single soft hit.
Transition mood: dramatic crossfade → Scene 6

### Scene 6 — Outro — 3.7s (19.8–23.5)
Logo + "JevCode", tagline "Visible actions. Reviewable changes.", feature row "5 providers · Per-file keep or revert · Isolated Git worktrees · Windows & Linux". Logo glow peaks at 22.93s. Music fades.
Audio intent: resolved, premium.

**Music mood for this video:** polished/cinematic-light
**Audio summary:** A steady clean bed with sparse UI-native accents, peaking on the "no match" catch, the "Rust decides" thesis, and the final logo.

Note: all task content (file names, request items) is fictional sample data mirroring the app's own demo; no secrets or user data.
