import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ImportRow } from '../src/features/settings/McpSettings';
import type { McpImportServer } from '../src/types/domain';

const importable: McpImportServer = {
  key: '.mcp.json#github',
  name: 'github',
  serverId: 'github',
  transport: 'Command: npx -y @example/github',
  envNames: ['DEBUG', 'GITHUB_TOKEN'],
  storedEnvNames: ['GITHUB_TOKEN'],
  warnings: ['Add values for DEBUG in Octran Code before connecting.'],
  blocked: null,
};
const blocked: McpImportServer = { ...importable, key: '.cursor/mcp.json#legacy', name: 'legacy', serverId: null, transport: null, envNames: [], storedEnvNames: [], warnings: [], blocked: 'SSE servers are not supported yet.' };
const noop = () => {};

describe('MCP import rows', () => {
  it('shows where a server would connect and which values move to the credential store', () => {
    const html = renderToStaticMarkup(<ImportRow server={importable} checked={false} disabled={false} onToggle={noop} />);

    expect(html).toContain('Command: npx -y @example/github');
    expect(html).toContain('Imported as <code>github</code>');
    expect(html).toContain('Environment: DEBUG, GITHUB_TOKEN · 1 stored in the OS credential store');
    expect(html).toContain('Add values for DEBUG in Octran Code before connecting.');
    expect(html).not.toContain('disabled=""');
  });

  it('cannot be selected when the server is blocked, and says why', () => {
    const html = renderToStaticMarkup(<ImportRow server={blocked} checked={false} disabled={false} onToggle={noop} />);

    expect(html).toContain('disabled=""');
    expect(html).toContain('SSE servers are not supported yet.');
    expect(html).toContain('is-blocked');
  });
});
