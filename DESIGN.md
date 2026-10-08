---
name: JevCode
description: A restrained desktop workspace for local projects and agent conversations.
colors:
  accent: "#37634c"
  accent-hover: "#2c513c"
  canvas: "#fafaf8"
  sidebar: "#f0f0ed"
  surface: "#ffffff"
  ink: "#222522"
  muted: "#656a65"
  line: "#dedfda"
  active: "#e0e7df"
  active-ink: "#234c34"
  navigation-hover: "#e7e8e2"
  field-line: "#c9cec7"
  field-focus: "#839d86"
  quiet-surface: "#f0f2ec"
  stop-surface: "#e9eae4"
  disabled-surface: "#e4e7e0"
  danger: "#9c3535"
typography:
  headline:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "clamp(26px, 2.6vw, 34px)"
    fontWeight: 680
    lineHeight: 1.25
    letterSpacing: "-0.035em"
  page-title:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "28px"
    fontWeight: 650
    letterSpacing: "-0.03em"
  title:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "16px"
    fontWeight: 600
  body:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 400
  conversation:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.8
  label:
    fontFamily: "Inter Variable, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 550
rounded:
  compact: "4px"
  navigation: "5px"
  control: "6px"
  row: "7px"
  panel: "10px"
  composer: "14px"
spacing:
  tight: "4px"
  small: "8px"
  control: "12px"
  section: "16px"
  panel: "20px"
  region: "24px"
  block: "28px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.surface}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "9px 13px"
  button-primary-hover:
    backgroundColor: "{colors.accent-hover}"
  button-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "9px 13px"
  button-secondary-hover:
    backgroundColor: "{colors.sidebar}"
  button-new-session:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.surface}"
    rounded: "{rounded.row}"
    padding: "11px 12px"
  button-send:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.surface}"
    rounded: "{rounded.control}"
    padding: "9px 11px"
  button-send-disabled:
    backgroundColor: "{colors.disabled-surface}"
    textColor: "{colors.muted}"
  button-stop:
    backgroundColor: "{colors.stop-surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "9px 11px"
  navigation-row:
    textColor: "{colors.ink}"
    rounded: "{rounded.navigation}"
    padding: "10px 11px"
  navigation-row-active:
    backgroundColor: "{colors.active}"
    textColor: "{colors.active-ink}"
  field:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "11px 12px"
  select:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.navigation}"
    padding: "7px 8px 7px 32px"
  composer:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.composer}"
    padding: "15px 17px 12px"
  tool-disclosure:
    backgroundColor: "{colors.quiet-surface}"
    textColor: "{colors.muted}"
    rounded: "{rounded.row}"
---

# Design System: JevCode

## Overview

**Creative North Star: "Restrained desktop development workspace"**

JevCode uses a restrained desktop workspace: warm neutral surfaces, a single forest green accent, self-hosted Inter, and Lucide line icons. Project navigation stays distinct from the conversation, while controls use familiar native buttons, selects, and disclosure elements.

The system is compact and flat. Borders, changes in surface tone, and spacing establish hierarchy; green identifies primary actions, selected navigation, focus, and live status. Desktop width and available height change density independently. The approved mockup establishes direction, while the implemented source supplies the reusable rules below.

**Key Characteristics:**

- Warm canvas and quiet sidebar with forest green controls.
- Self-hosted Inter with compact, sentence-case labels.
- Flat surfaces defined by fine borders and tonal changes.
- Persistent project navigation, conversation, composer, and status regions.
- Lucide SVG icons paired with semantic native controls.

## Colors

The palette combines warm near-white and gray surfaces with a subdued forest green accent. The frontmatter is the normative color vocabulary; sidecar tonal ramps are visualization metadata.

### Primary

- **Forest Green** (`accent`): primary actions, brand mark, focus outlines, caret, and status dots.
- **Deep Forest** (`accent-hover`): hover state for primary actions.
- **Selected Sage / Forest Ink** (`active`, `active-ink`): paired background and text for current navigation.
- **Sage Focus** (`field-focus`): the composer border when focus is inside.

### Neutral

- **Warm Canvas** (`canvas`): main application background.
- **Quiet Sidebar** (`sidebar`): persistent navigation and secondary-button hover.
- **White Surface** (`surface`): composer, credential fields, selects, and permission menu.
- **Charcoal Ink / Muted Gray** (`ink`, `muted`): primary text and supporting information.
- **Fine Divider / Field Border** (`line`, `field-line`): region boundaries and input outlines.
- **Navigation Hover** (`navigation-hover`): subdued row feedback.
- **Quiet Surface** (`quiet-surface`): suggestion hover and tool output.
- **Stop Surface / Disabled Surface** (`stop-surface`, `disabled-surface`): neutral stop action and unavailable send state.

The existing danger color is reserved for errors; it is a semantic signal rather than a second brand accent.

### Named Rules

**The Action Accent Rule.** Use forest green for primary actions, selected context, focus, and live status; keep broad reading surfaces neutral.

## Typography

**Display and Body Font:** Inter Variable, self-hosted from the Latin variable WOFF2 file, with Segoe UI and sans-serif fallbacks.

**Code Font:** The browser's native monospace face in code and preformatted tool output.

**Character:** A single sans-serif family keeps the workspace familiar and direct. Medium weights distinguish labels and controls; larger headings use tighter tracking.

### Hierarchy

- **Headline:** welcome heading using the frontmatter `headline` role.
- **Page title:** settings headings using `page-title`.
- **Title:** credential section headings using `title`.
- **Body:** default interface text using `body`.
- **Conversation:** wrapped message content using `conversation`.
- **Label:** compact controls and section labels using `label`; labels remain in sentence case.

Wide desktop and short-height overrides are described in Layout. Numeric totals and token counts use tabular figures. Credential explanatory copy is limited to approximately (65ch). Small incidental metadata in the current source does not define a reusable type role.

### Named Rules

**The One Family Rule.** Use self-hosted Inter for interface text. Reserve native monospace for code and tool output.

## Layout

The supported product is a desktop Tauri window with a minimum of (900 × 640), as stated in the direction contract. The shell fills (100dvh), clips outer overflow, and uses a persistent sidebar plus a flexible main column. The default sidebar is (244px); the toolbar is (57px) high. Conversation and settings content are centered at a maximum of (750px), with (64px) total horizontal inset. The welcome region is capped at (680px), with (72px) total inset. The composer spans the main region with (48px) total inset.

The main column stacks the toolbar, independently scrolling conversation or settings region, composer, and status bar. Sidebar sessions scroll independently between project navigation and lower utilities. Use flex and grid minimum-size constraints so long text cannot force either column wider.

At desktop widths of (1100px) or less, the sidebar narrows to (216px), the project-context label hides its text, and native selects cap at (190px). At widths of (1400px) or more, the sidebar expands to (332px), the toolbar to (69px), and the welcome region to (852px). This wide rule increases brand and control sizes: welcome heading (40px), introductory text (23px), sidebar navigation (17px), suggestion and new-session text (18px), composer/select/send text (16px), and composer hint/status text (14px). Session rows retain their compact override.

At heights of (760px) or less, the welcome region uses (20px) vertical padding, heading (28px), introductory text (14px), suggestion top gap (24px), and suggestion rows with (14px 10px) padding. This rule follows the wide rule and wins for those properties even on a wide, short window.

A narrower fallback exists at widths of (760px) or less: sidebar (180px), reduced horizontal insets, wrapped composer controls, and hidden security status. It is a defensive browser-preview fallback; the product contract does not establish a mobile surface.

The reused spacing steps are in the frontmatter. Components also use the measured asymmetric padding shown there; avoid forcing every value into a fabricated uniform scale.

## Elevation & Depth

The workspace is flat. A shaded sidebar, white fields, subtle selected rows, and fine borders convey depth. The permission menu is positioned above content with a border and stacking order, without a shadow. Permission requests use a pale green surface and border; errors use a pale warm surface and danger text.

### Named Rules

**The Tonal Depth Rule.** Establish separation with surface color, fine borders, and spacing. The current system has no box-shadow vocabulary.

## Shapes

Corners are gently rounded. Compact icon and menu controls use `compact`; navigation and selects use `navigation`; standard buttons and credential fields use `control`; new-session and tool-output rows use `row`; permission surfaces use `panel`; the composer uses the larger `composer` silhouette. Fine (1px) borders define surfaces and dividers. Status dots are circular.

Lucide line icons provide the action and navigation vocabulary, usually (14–20px) in the compact interface. Wide suggestions use (23px) icons. The JevCode letter mark is a brand element rather than a replacement for action icons.

## Components

### Buttons

Primary buttons use forest green and white text; secondary buttons use white, ink, and a fine divider border. Standard actions share compact label typography and the `control` radius. New session is a wider, left-aligned sidebar action; send and stop are compact composer actions with both icon and label.

Primary hover darkens the accent; secondary hover uses the sidebar surface. Button color transitions last (140ms) with ease timing. Focus uses an accent outline (2px) with (3px) offset. Disabled buttons and selects use reduced opacity, except send, whose disabled state uses its own neutral surface and keeps full opacity.

### Inputs / Fields

Credential inputs are white, bordered, and gently rounded. The prompt textarea is visually contained by the composer rather than by its own outline; the composer changes border color on focus-within. The caret uses the accent. Provider and model controls remain native selects with visible labels for assistive technology and a Lucide icon inset. Native checkbox accent follows the primary color.

### Navigation

Project, session, and utility rows pair a Lucide icon with a label. Current rows use the selected sage surface and forest text; hover uses the quiet navigation surface. Long names truncate with an ellipsis; icons do not shrink. Current project semantics use `aria-current`, while utilities and sessions retain button behavior. The sidebar uses fine dividers between project, utility, and workspace-footer regions.

### Composer

A white, bordered container with the largest recurring corner radius groups the prompt and controls. The textarea uses (1.7) line height. Provider/model controls sit together; project context follows; send or stop aligns to the end. Enter submits when available, Shift+Enter creates a newline, and composition input is preserved. The hint underneath reports the actual project/provider state.

### Containers / Disclosures

Tool output is a flat tonal disclosure with the `row` radius, wrapping preformatted content, and a bounded scrolling output area. Permission requests are bordered, softly rounded panels with an accent icon and primary/secondary actions. The permission policy menu is a native details/summary control.

Tool output and the permission menu are functional containers. Suggestion and provider lists use divider-separated rows, without individual raised cards. The system currently has no chip or tag primitive.

### Status

The footer uses muted text, a green status dot, and tabular token counts aligned at the end. Working status pulses over (1.6s) with ease-in-out timing. The reduced-motion query disables animations and transitions and restores automatic scroll behavior.

## Do's and Don'ts

### Do:

- **Do** use the forest green accent for actions, selection, focus, and status.
- **Do** keep project navigation distinct from the main conversation and settings region.
- **Do** use Lucide SVG icons and retain visible labels or accessible names on controls.
- **Do** preserve native button, select, checkbox, and disclosure semantics.
- **Do** retain the accent focus outline; inside the composer, use its focus-within border treatment.
- **Do** apply the short-height density rule even when the wide-screen rule is active.
- **Do** keep long navigation labels truncated and conversation/tool text able to wrap.
- **Do** honor reduced-motion preferences.

### Don't:

- **Don't** expand the palette with additional decorative accents.
- **Don't** introduce raised-card shadows into the flat surface vocabulary.
- **Don't** promote small incidental timestamps, version text, or tool metadata into the reusable interface type scale.
- **Don't** treat the narrow CSS fallback as a supported mobile product.
- **Don't** treat the reference mockup as an application asset or as evidence of verified pixel fidelity.
