import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { SourceRow } from '../src/features/agent/EffectiveContextView';
import type { EffectiveContextSource } from '../src/types/domain';

const included: EffectiveContextSource = {
  path: 'packages/app/AGENTS.md',
  scope: 'folder',
  status: 'included',
  summary: 'Included at task start.',
  bytes: 2048,
  secretLines: [],
  fingerprint: 'abc',
  change: 'changed',
  preview: 'Run focused tests.\n[possible secret hidden]',
};

describe('effective context source rows', () => {
  it('shows status, scope, size, and what changed since the task started', () => {
    const html = renderToStaticMarkup(<SourceRow item={included} />);

    expect(html).toContain('class="effective-context-status">Included<');
    expect(html).toContain('This folder');
    expect(html).toContain('2 KiB');
    expect(html).toContain('effective-context-change is-changed">Changed<');
  });

  it('offers the masked preview only when there is text to show', () => {
    expect(renderToStaticMarkup(<SourceRow item={included} />)).toContain('<pre>Run focused tests.');

    const skipped: EffectiveContextSource = {
      ...included,
      status: 'skipped',
      summary: 'Not UTF-8 text.',
      change: null,
      preview: null,
      fingerprint: null,
    };
    const html = renderToStaticMarkup(<SourceRow item={skipped} />);
    expect(html).toContain('Skipped');
    expect(html).not.toContain('<details>');
    expect(html).not.toContain('effective-context-change');
  });
});
