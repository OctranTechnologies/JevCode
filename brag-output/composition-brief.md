# Hyperframes Composition Brief: JevCode

## Objective
Create a short launch-style brag video for JevCode, emphasizing Jev, the separate decision model that checks the coding agent's work but cannot act.

## Output
- Composition directory: `brag-output/composition/`
- Rendered video: `brag-output/brag.mp4`
- Format: landscape — 1920x1080
- Duration: 23.5s

## Source Material
- Project root: repository root
- Primary files read: README.md, docs/jev-roadmap.md, docs/jev-evaluation.md, src/workbench.css, src/styles.css, public/favicon.svg, src/features/agent/Conversation.tsx, src-tauri/src/agent/mod.rs
- Product name: JevCode
- Tagline: "A local-first desktop coding agent with visible actions and reviewable changes."
- Key UI to recreate: the "Task completion checklist" card (Jev labels + Files changed + Test runs evidence) and task-timeline activity rows.
- Copy that must appear verbatim (or lightly trimmed):
  - "Task completed." (runtime activity string)
  - "Task completion checklist", "Jev: possible match", "Jev: no match", "Files changed", "Test runs", "passed (exit 0)"
  - "Jev's shadow labels are suggestions, not verification."
  - "Jev shadow recovery hint: inspect the environment or project setup after 3 repeated failed command actions." / "No action was taken automatically."
  - "Jev shadow warning: src/auth/oauth.rs may be outside this task's scope." / "No action was blocked or reverted."
  - "Possible task overlap: …These tasks share the same project checkout." / "This warning does not block the edit."
  - "Jev recommends. Rust decides." (from roadmap principle "Rust remains authoritative. Jev may recommend…")

## Creative Direction
- Tone preset: polished
- Creative direction: quiet flex for people who already use coding agents
- Angle: every agent says "Task completed." — JevCode asks a second model to check against real evidence, and that model has zero authority.
- Hook: "Task completed." types out → "Says who?"
- Outro: logo, "Visible actions. Reviewable changes.", feature row.
- Avoid: competitor names, generic SaaS language, abstract filler.

## Visual Identity
- Background `#0b0b0c`, surfaces `#131315`/`#16161a`, border `#2a2a30`
- Text `#ededee`, muted `#aeb2bb`
- Accent `#ff5e1a`, Jev indigo `#a5b4fc`, success `#6ee7b7`, warning `#fcd34d`, danger `#fda4af`
- Font: Inter (local woff2 from @fontsource-variable/inter); mono: generic monospace
- Logo: five-stroke radial asterisk from public/favicon.svg

## Storyboard
See `brag-plan.md`. Scenes: Hook 0–3.5 · Reveal 3.3–6.6 · Checklist 6.4–11.4 · Timeline signals 11.2–16.4 · Authority 16.2–20.0 · Outro 19.8–23.5.

## Audio
- Music: vol-12 at 0.32, lane fade-in 0–0.6s, fade-out 22.3–23.5s
- Cues (bundled preset): beat-locks at 8.74 (no-match catch), 17.47 (Rust decides), 20.19 (logo arrival); timeline rows on every other beat 11.95/13.05/14.15
- Audio-reactive: RMS drives logo/background glow, subtle
- SFX: keypresses on typing, soft impacts on "Says who?" / no-match / "Rust decides.", drops on checklist rows, clicks on timeline rows, bell on logo
