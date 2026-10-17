import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ReleaseNotes } from '../src/features/settings/ReleaseNotes';

describe('release notes', () => {
  it('renders updater markdown as structured text without interpreting HTML', () => {
    const html = renderToStaticMarkup(<ReleaseNotes
      version="0.2.0"
      publishedAt="2026-10-17T00:00:00Z"
      body={'## Improvements\n\n- **Faster search** across large projects\n- Run `npm test`\n\n<script>alert(1)</script> [unsafe](javascript:alert(1))'}
    />);

    expect(html).toContain("What's New in v0.2.0");
    expect(html).toContain('<strong>Faster search</strong>');
    expect(html).toContain('<code>npm test</code>');
    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt;');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('href=');
    expect(html).toContain('unsafe</span>');
  });

  it('shows a clear empty state when the release has no notes', () => {
    const html = renderToStaticMarkup(<ReleaseNotes version="0.2.0" body="  " />);
    expect(html).toContain('No release notes were provided.');
  });
});
