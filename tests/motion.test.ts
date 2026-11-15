import { afterEach, describe, expect, it } from 'vitest';
import { scrollBehavior } from '../src/lib/motion';

function stubReducedMotion(reduce: boolean) {
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { matchMedia: (query: string) => ({ matches: reduce && query.includes('prefers-reduced-motion: reduce') }) },
  });
}

describe('scrollBehavior', () => {
  afterEach(() => {
    delete (globalThis as { window?: unknown }).window;
  });

  it('keeps the requested behavior when motion is allowed', () => {
    stubReducedMotion(false);
    expect(scrollBehavior('smooth')).toBe('smooth');
  });

  it('jumps instantly when the user prefers reduced motion', () => {
    stubReducedMotion(true);
    expect(scrollBehavior('smooth')).toBe('auto');
  });

  it('falls back to the requested behavior where matchMedia is unavailable', () => {
    expect(scrollBehavior('smooth')).toBe('smooth');
  });
});
