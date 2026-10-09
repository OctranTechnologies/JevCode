import { describe, expect, it } from 'vitest';
import { shortcutModifier } from '../src/lib/shortcuts';

describe('keyboard shortcut labels', () => {
  it('uses the Command modifier on Apple platforms', () => {
    expect(shortcutModifier('MacIntel')).toBe('⌘');
    expect(shortcutModifier('iPad')).toBe('⌘');
  });

  it('uses Ctrl on Windows and Linux', () => {
    expect(shortcutModifier('Win32')).toBe('Ctrl');
    expect(shortcutModifier('Linux x86_64')).toBe('Ctrl');
  });
});
