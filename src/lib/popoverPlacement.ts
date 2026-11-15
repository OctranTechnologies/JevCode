export type Placement = 'above' | 'below';

const viewportMargin = 12;

/**
 * Opens a popover below its anchor when it fits there, or when there is more room below than above.
 * `room` is the space left on the chosen side, so a popover can cap its height to it.
 */
export function choosePlacement(
  anchor: { top: number; bottom: number },
  height: number,
  viewportHeight: number,
  gap: number,
): { placement: Placement; room: number } {
  const below = viewportHeight - anchor.bottom - gap - viewportMargin;
  const above = anchor.top - gap - viewportMargin;
  return below >= height || below >= above ? { placement: 'below', room: below } : { placement: 'above', room: above };
}
