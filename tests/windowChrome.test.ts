import { describe, expect, it } from 'vitest';
import { usesCustomTitleBar } from '../src/lib/windowChrome';

const windowsAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0';
const macAgent = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15';
const linuxAgent = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36';

describe('custom title bar platform check', () => {
  it('draws the bar only inside the Windows desktop app', () => {
    expect(usesCustomTitleBar(true, windowsAgent)).toBe(true);
    expect(usesCustomTitleBar(false, windowsAgent)).toBe(false);
  });

  it('keeps the native frame on macOS and Linux', () => {
    expect(usesCustomTitleBar(true, macAgent)).toBe(false);
    expect(usesCustomTitleBar(true, linuxAgent)).toBe(false);
  });
});
