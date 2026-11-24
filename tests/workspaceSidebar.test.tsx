import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { WorkspaceSidebar } from '../src/features/workspaces/WorkspaceSidebar';

const noop = () => undefined;

function renderSidebar(collapsed: boolean) {
  return renderToStaticMarkup(<WorkspaceSidebar projects={[]} sessions={[]} projectId="" sessionId={null} view="overview" collapsed={collapsed} opening={false} showArchived={false} interfaceMode="maximum" onShowArchived={noop} onCollapse={noop} onProject={noop} onSession={noop} onRenameSession={noop} onArchiveSession={noop} onDeleteSession={noop} onDuplicateSession={noop} onForkSession={noop} onView={noop} onNew={noop} onOpen={noop} onCreate={noop} onRemoveRecent={noop} onReveal={noop} onSearch={noop} />);
}

function headerOf(html: string) {
  return html.match(/<div class="sidebar-header">(.*?)<\/div>/)?.[1] ?? '';
}

describe('workspace sidebar header', () => {
  it('shows the collapse control in place of the app title', () => {
    const header = headerOf(renderSidebar(false));

    expect(header).toContain('aria-label="Collapse sidebar"');
    expect(header).not.toContain('Octran Code');
  });

  it('offers the expand control when the sidebar is collapsed', () => {
    expect(headerOf(renderSidebar(true))).toContain('aria-label="Expand sidebar"');
  });
});
