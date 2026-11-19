import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { MarkdownMessage } from '../src/features/agent/MarkdownMessage';

const root = fileURLToPath(new URL('..', import.meta.url));
const css = readFileSync(join(root, 'src/workbench.css'), 'utf8');

function rule(selector: string) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`).exec(css);
  expect(match, `missing CSS rule for ${selector}`).not.toBeNull();
  return match?.[1] ?? '';
}

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(ts|tsx)$/.test(entry.name) ? [path] : [];
  });
}

describe('typography rules', () => {
  it('offsets every underlined link by .2em', () => {
    const offsets = [...css.matchAll(/text-underline-offset:\s*([^;]+);/g)].map(match => match[1].trim());

    expect(offsets.length).toBeGreaterThanOrEqual(4);
    for (const offset of offsets) expect(offset).toBe('.2em');
  });

  it('shrinks the folder first, then the base name, and never the extension', () => {
    expect(rule('.file-label-ext')).toContain('flex: 0 0 auto');
    expect(rule('.file-label-dir')).toContain('flex: 0 1000 auto');
    expect(rule('.file-label-stem')).toContain('overflow: hidden');
  });

  it('keeps footnote superscripts out of the line height', () => {
    const sup = rule('.message-markdown sup');

    expect(sup).toContain('position: relative');
    expect(sup).toContain('line-height: 0');
    expect(sup).toContain('vertical-align: baseline');
  });

  it('clamps both multi-line titles to two lines', () => {
    for (const selector of ['.task-detail-panel h3', '.project-memory-title h3']) {
      const body = rule(selector);
      expect(body).toContain('display: -webkit-box');
      expect(body).toContain('-webkit-line-clamp: 2');
      expect(body).toContain('overflow: hidden');
    }
  });

  it('never hyphenates paths, code or identifiers', () => {
    const line = css.split('\n').find(item => item.includes('hyphens: auto'));
    expect(line, 'missing hyphens rule').toBeDefined();
    const selector = (line ?? '').slice(0, (line ?? '').indexOf('{'));

    expect(selector).not.toMatch(/\b(code|pre|path|kbd)\b/);
    expect(readFileSync(join(root, 'index.html'), 'utf8')).toContain('lang="en"');
  });

  it('keeps straight apostrophes out of interface copy', () => {
    const offenders: string[] = [];
    for (const file of sourceFiles(join(root, 'src'))) {
      const lines = readFileSync(file, 'utf8').split(/\r?\n/);
      lines.forEach((line, index) => {
        const trimmed = line.trim();
        if (trimmed.startsWith('//') || trimmed.startsWith('/*') || trimmed.startsWith('*')) return;
        if (/[A-Za-z]'(?:s|t|re|ll|ve|d|m)\b/.test(trimmed)) offenders.push(`${relative(root, file)}:${index + 1}`);
      });
    }

    expect(offenders).toEqual([]);
  });

  it('renders footnote references as superscripts', () => {
    const markup = renderToStaticMarkup(createElement(MarkdownMessage, { content: 'A note.[^1]\n\n[^1]: The detail.' }));

    expect(markup).toContain('<sup>');
  });
});
