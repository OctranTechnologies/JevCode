import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { WindowTitleBar, WindowTitleBarView } from '../src/components/WindowTitleBar';

const noop = () => undefined;

describe('window title bar', () => {
  it('labels the window controls and marks the bar as a drag region', () => {
    const html = renderToStaticMarkup(<WindowTitleBarView maximized={false} onMinimize={noop} onToggleMaximize={noop} onClose={noop} />);

    expect(html).toContain('data-tauri-drag-region');
    expect(html).toContain('aria-label="Minimize"');
    expect(html).toContain('aria-label="Maximize"');
    expect(html).toContain('aria-label="Close"');
    expect(html.match(/type="button"/g)).toHaveLength(3);
  });

  it('offers restore when the window is already maximized', () => {
    const html = renderToStaticMarkup(<WindowTitleBarView maximized onMinimize={noop} onToggleMaximize={noop} onClose={noop} />);

    expect(html).toContain('aria-label="Restore"');
    expect(html).not.toContain('aria-label="Maximize"');
  });

  it('renders nothing outside the Windows desktop app', () => {
    expect(renderToStaticMarkup(<WindowTitleBar />)).toBe('');
  });
});
