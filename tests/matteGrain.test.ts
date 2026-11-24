import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const css = readFileSync(new URL('../src/workbench.css', import.meta.url), 'utf8');
const lightStart = css.indexOf(":root[data-theme='light'] {");
const darkBlock = css.slice(0, lightStart);
const lightBlock = css.slice(lightStart, css.indexOf('}', lightStart));

describe('matte background grain', () => {
  it('declares a capped grain tile in both theme blocks', () => {
    for (const [block, cap] of [[darkBlock, 0.05], [lightBlock, 0.04]] as const) {
      const slope = Number(/--wb-grain: url\(.*slope='([\d.]+)'/.exec(block)?.[1]);
      expect(slope).toBeGreaterThan(0);
      expect(slope).toBeLessThanOrEqual(cap);
    }
  });

  it('keeps an opaque surface color under every grain layer', () => {
    const layers = css.match(/background: var\(--wb-grain[^;]*;/g) ?? [];
    expect(layers.length).toBeGreaterThanOrEqual(11);
    for (const layer of layers) expect(layer).toMatch(/^background: var\(--wb-grain, none\), var\(--wb-(canvas|sidebar)\);$/);
  });
});
