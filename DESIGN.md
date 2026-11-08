---
name: JevCode
description: A focused desktop workspace for reviewing and directing coding agents.
colors:
  graphite: "#131315"
  graphite-soft: "#1A1A1D"
  panel-line: "#2A2A2F"
  panel-line-strong: "#3A3A40"
  paper: "#FFFFFF"
  paper-soft: "#F4F4F5"
  warm-canvas: "#FAF9F6"
  warm-sidebar: "#F4F2EE"
  ink: "#EDEDEE"
  muted-dark: "#8A8F98"
  muted-light: "#55575E"
  signal-orange: "#FF5E1A"
  deep-orange: "#B9480F"
  apricot: "#FDC9A5"
  success: "#6EE7B7"
  warning: "#FCD34D"
  danger: "#FDA4AF"
typography:
  title:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "26px"
    fontWeight: 520
    lineHeight: 1.2
    letterSpacing: "-0.025em"
  body:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.65
  conversation:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.75
  label:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 550
  code:
    fontFamily: "JetBrains Mono, Cascadia Code, Consolas, monospace"
    fontSize: "11px"
    fontWeight: 400
rounded:
  none: "0px"
spacing:
  tight: "4px"
  small: "8px"
  control: "12px"
  section: "16px"
  panel: "20px"
  region: "24px"
components:
  primary-action:
    backgroundColor: "{colors.signal-orange}"
    textColor: "{colors.graphite}"
    typography: "{typography.label}"
    rounded: "{rounded.none}"
    height: "44px"
  secondary-action:
    backgroundColor: "{colors.graphite-soft}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.none}"
    height: "44px"
  text-field:
    backgroundColor: "{colors.graphite}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.none}"
    height: "44px"
  workspace-panel:
    backgroundColor: "{colors.graphite}"
    textColor: "{colors.ink}"
    rounded: "{rounded.none}"
---

# Design System: JevCode

## Overview

**Creative North Star: Precise engineering workspace.** JevCode is a desktop coding-agent workbench. The repository, conversation, permission decisions, and reviewable file changes remain easy to distinguish. A single orange signal marks the current action, selected context, or active work.

The system uses the Octran guide's graphite and paper palette, square geometry, fine rules, Inter interface text, and mono text for code and measured values. The JevCode asterisk remains the brand mark. Simple and Maximum are density modes over the same visual system.

## Colors

Dark mode uses graphite `#131315`, soft graphite `#1A1A1D`, panel lines `#2A2A2F` / `#3A3A40`, and light text `#EDEDEE`. Slate `#8A8F98` carries secondary text. Orange `#FF5E1A` identifies primary actions, focus, selection, and active work. Apricot is reserved for approval and callout states; green, yellow, and rose communicate success, warning, and failure.

Light mode keeps its warm canvas preference with paper surfaces and dividers. Use deep orange `#B9480F` for small accent text on light backgrounds, where the brighter signal orange needs more contrast. Orange never communicates a state by color alone; pair it with a label, icon, position, or shape.

Keep body text and controls at least 4.5:1 contrast, large text at least 3:1, and UI boundaries at least 3:1. Verify pairs in both themes.

## Typography

Inter Variable is the interface and conversation face. Use 24–28px regular-weight page headings, 14px conversation text, and 12–13px control labels. Keep prose around 65–72 characters per line. Mono is for code, paths, commands, token values, and compact technical labels; use JetBrains Mono when available, followed by Cascadia Code, Consolas, and system monospace. Use tabular numerals for counts, durations, and usage.

## Layout

The application is a Tauri desktop window. At wide sizes, project navigation, work area, and optional task inspector form distinct panes. The sidebar and inspector remain resizable. Below the existing responsive breakpoints, the sidebar and inspector become overlays; keep the composer and primary navigation usable.

The workspace places the project and branch in the toolbar, the conversation in a centered reading column, the composer at the bottom, and task details in the inspector. Settings use a readable centered measure; data tables may scroll within their own region. Simple hides or groups secondary controls while preserving approvals, errors, agent status, and change review. Maximum exposes the fuller set of controls.

Use 4–24px spacing steps. Establish hierarchy with whitespace and 1px rules. Avoid boxed paragraphs and repeated decorative cards; keep cards for functional groupings such as diffs, permission prompts, dialogs, and usage summaries.

## Elevation & Depth

The interface is flat. Use surface changes and 1px borders to separate panes, fields, disclosures, and dialogs. Do not add decorative shadows, blur, or gradients. Focus is a visible 2px orange outline with 4px offset.

## Shapes

Application controls and panels use square corners. Borders are 1px except for the 2px focus outline. Status dots and diagram marks may retain their meaningful geometry. Icons use the existing Lucide line system and should remain consistent in stroke and scale.

## Components

### Navigation and toolbars

Sidebar groups and settings sections use open rows and divider rules. The active project or task receives a muted orange surface and a clear label. Keep project names and paths truncatable without hiding their action controls. Toolbar controls align on one baseline and retain accessible labels.

### Conversation and composer

Conversation messages use a comfortable reading width and whitespace rather than chat bubbles. Markdown headings, lists, and code blocks keep their semantic hierarchy. The composer is a bordered work surface with an obvious focus state. Attach, context, mode, model, send, and stop controls remain distinct and keyboard accessible.

### Activity, approval, and review

Routine tool activity may collapse in Simple mode; errors, permission prompts, current task status, and changed-file review stay visible. Use short summaries with expandable structured details. Approval cards state the action and risk before the user chooses. Diffs use separate semantic colors for additions and deletions, with file paths and line numbers in mono text.

### Settings and data

Settings pages use direct headings and divider-separated sections. Forms have visible labels, square 44px fields, and readable help text. Tables and usage charts prioritize legibility and honest status; do not imply that local usage equals a provider's account totals.

## Do's and Don'ts

- Use orange for action and current state, with a non-color cue alongside it.
- Keep working controls at least 44px tall where the layout allows; keep form fields 44–48px.
- Preserve keyboard focus, reduced-motion behavior, and native button/select/disclosure semantics.
- Keep errors, approvals, streaming status, and file review visible in every density mode.
- Use mono type for code and measured data, not as decoration.
- Do not add rounded cards, decorative grids, blur, gradients, or shadows.
- Do not present conceptual blueprint artwork as a live application screen.
