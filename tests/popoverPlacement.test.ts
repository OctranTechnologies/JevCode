import { describe, expect, it } from 'vitest';
import { choosePlacement } from '../src/lib/popoverPlacement';

describe('choosePlacement', () => {
  it('opens below an anchor near the top when the popover fits there', () => {
    expect(choosePlacement({ top: 40, bottom: 60 }, 200, 800, 6)).toEqual({ placement: 'below', room: 800 - 60 - 6 - 12 });
  });

  it('opens above an anchor at the bottom of the window when there is no room below', () => {
    const result = choosePlacement({ top: 800, bottom: 830 }, 520, 850, 6);
    expect(result.placement).toBe('above');
    expect(result.room).toBe(800 - 6 - 12);
  });

  it('keeps the side with more room when the popover fits on neither side', () => {
    expect(choosePlacement({ top: 100, bottom: 130 }, 400, 300, 6).placement).toBe('below');
  });
});
