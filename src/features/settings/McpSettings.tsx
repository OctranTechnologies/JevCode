import { useCallback, useEffect, useState } from 'react';
import { Globe2, LoaderCircle, PlugZap, Plus, RefreshCw, ShieldAlert, ShieldCheck, Terminal, Trash2, X } from 'lucide-react';
import { command, desktopAvailable } from '../../lib/ipc';
import type { McpScope, McpServerConfig, McpServerView, McpTransport, Project } from '../../types/domain';

type SecretField = { name: string; value: string };
type Draft = {
  id: string; name: string; scope: McpScope; type: 'stdio' | 'http'; command: string; args: string; url: string;
  env: SecretField[]; token: string; hasAuthToken: boolean;
};

const emptyDraft = (scope: McpScope = 'user'): Draft => ({
  id: '', name: '', scope, type: 'stdio', command: '', args: '', url: '', env: [], token: '', hasAuthToken: false,
});

export function McpSettings({ project, onError }: { project?: Project; onError: (error: string) => void }) {
  const [servers, setServers] = useState<McpServerView[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);

  const reload = useCallback(async (background = false) => {
    if (!desktopAvailable) { setLoading(false); return; }
    if (background) setRefreshing(true); else setLoading(true);
    try { setServers(await command('list_mcp_servers', { projectId: project?.id ?? null })); }
    catch (error) { onError(error instanceof Error ? error.message : 'MCP configuration could not be loaded.'); }
    finally { setLoading(false); setRefreshing(false); }
  }, [onError, project?.id]);

  useEffect(() => { void reload(); }, [reload]);

  function editServer(view: McpServerView) {
    const config = view.config;
    const transport = config.transport;
    setDraft({
      id: config.id, name: config.name, scope: config.scope, type: transport.type,
      command: transport.type === 'stdio' ? transport.command : '',
      args: transport.type === 'stdio' ? transport.args.join('\n') : '',
      url: transport.type === 'http' ? transport.url : '',
      env: config.envNames.map(name => ({ name, value: '' })), token: '', hasAuthToken: config.hasAuthToken,
    });
  }

  async function save() {
    if (!draft) return;
    setSaving(true);
    try {
      const env = draft.type === 'stdio' ? draft.env.filter(item => item.name.trim()) : [];
      const transport: McpTransport = draft.type === 'stdio'
        ? { type: 'stdio', command: draft.command.trim(), args: draft.args.split('\n').map(value => value.trim()).filter(Boolean) }
        : { type: 'http', url: draft.url.trim() };
      const config: McpServerConfig = {
        id: draft.id.trim(), name: draft.name.trim(), scope: draft.scope,
        projectId: draft.scope === 'project' ? project?.id ?? null : null,
        transport, envNames: env.map(item => item.name.trim()), hasAuthToken: draft.type === 'http' && draft.hasAuthToken, enabled: false,
      };
      const secrets = Object.fromEntries(env.filter(item => item.value).map(item => [item.name.trim(), item.value]));
      if (draft.token) secrets.__AUTH_TOKEN = draft.token;
      await command('save_mcp_server', { input: { config, secrets } });
      setDraft(null);
      await reload(true);
    } catch (error) { onError(error instanceof Error ? error.message : 'MCP server could not be saved.'); }
    finally { setSaving(false); }
  }

  async function setEnabled(server: McpServerView, enabled: boolean) {
    const key = getKey(server);
    setBusy(key);
    try { setServers(await command('set_mcp_server_enabled', { input: actionInput(server), enabled })); }
    catch (error) { onError(error instanceof Error ? error.message : 'MCP server state could not be changed.'); }
    finally { setBusy(null); }
  }

  async function connect(server: McpServerView) {
    if (!desktopAvailable) return;
    if (!server.trusted) {
      const endpoint = server.config.transport.type === 'stdio'
        ? `${server.config.transport.command}\n${server.config.transport.args.join(' ')}`
        : server.config.transport.url;
      const confirmed = window.confirm(`Trust and start “${server.config.name}”?\n\n${endpoint}\n\nThis external server can execute its own tools. JevCode will still ask for task permissions.`);
      if (!confirmed) return;
    }
    const key = getKey(server);
    setBusy(key);
    try {
      let target = server;
      if (!target.config.enabled) {
        const updated = await command('set_mcp_server_enabled', { input: actionInput(target), enabled: true });
        setServers(updated);
        target = updated.find(item => getKey(item) === key) ?? { ...target, config: { ...target.config, enabled: true } };
      }
      const connected = await command('connect_mcp_server', { input: actionInput(target), trust: !target.trusted });
      setServers(current => current.map(item => getKey(item) === key ? connected : item));
    } catch (error) {
      onError(error instanceof Error ? error.message : 'MCP server could not connect.');
      await reload(true);
    } finally { setBusy(null); }
  }

  async function disconnect(server: McpServerView) {
    setBusy(getKey(server));
    try { setServers(await command('disconnect_mcp_server', { input: actionInput(server) })); }
    catch (error) { onError(error instanceof Error ? error.message : 'MCP server could not disconnect.'); }
    finally { setBusy(null); }
  }

  async function remove(server: McpServerView) {
    setBusy(getKey(server));
    try {
      await command('delete_mcp_server', { input: actionInput(server) });
      setServers(current => current.filter(item => getKey(item) !== getKey(server)));
    } catch (error) { onError(error instanceof Error ? error.message : 'MCP server could not be removed.'); }
    finally { setBusy(null); }
  }

  return <section className="settings-content mcp-settings">
    <div className="mcp-page-heading">
      <div><h1>MCP</h1><p className="page-description">Connect external tools through the Model Context Protocol. Configured servers stay stopped until you explicitly trust and connect them.</p></div>
      <button className="mcp-quiet-action" onClick={() => void reload(true)} disabled={refreshing} title="Refresh MCP status">{refreshing ? <LoaderCircle size={14} className="spin" /> : <RefreshCw size={14} />}Refresh</button>
    </div>

    <div className="mcp-security-note"><ShieldAlert size={15} /><span>MCP tools run outside JevCode's built-in tool sandbox. Calls still require the active task's permission checks and appear in its timeline. Server output is treated as untrusted.</span></div>

    <section className="mcp-server-section" aria-labelledby="mcp-server-heading">
      <div className="mcp-section-title"><div><h2 id="mcp-server-heading">Servers</h2><p>User configuration is private to this app. Project configuration lives in <code>.jevcode/mcp.json</code>.</p></div>
        <button className="mcp-add-button" onClick={() => setDraft(emptyDraft())}><Plus size={14} />Add server</button>
      </div>
      <div className="mcp-table-wrap"><table className="mcp-table">
        <thead><tr><th>Server</th><th>Status</th><th>Available tools</th><th>Scope</th><th>Permissions</th><th aria-label="Actions" /></tr></thead>
        <tbody>
          {loading ? <tr><td colSpan={6}><div className="mcp-loading"><span /><span /><span /></div></td></tr> : servers.length === 0 ? <tr><td colSpan={6}><div className="mcp-empty"><PlugZap size={18} /><span>No MCP servers configured.</span><small>Add a user or project server to discover external tools.</small></div></td></tr> : servers.map(server => {
            const key = getKey(server);
            const pending = busy === key;
            return <tr key={key}>
              <td><div className="mcp-server-name"><span className="mcp-transport-icon">{server.config.transport.type === 'stdio' ? <Terminal size={14} /> : <Globe2 size={14} />}</span><span><strong>{server.config.name}</strong><small>{server.config.transport.type === 'stdio' ? server.config.transport.command : server.config.transport.url}</small></span></div></td>
              <td><span className={`mcp-status mcp-status-${server.status}`}><i />{formatStatus(server.status)}</span></td>
              <td><span className="mcp-tool-count">{server.availableTools.length ? `${server.availableTools.length} ${server.availableTools.length === 1 ? 'tool' : 'tools'}` : '—'}</span>{server.lastError && <small className="mcp-inline-error" title={server.lastError}>{server.lastError}</small>}</td>
              <td><span className={`mcp-scope mcp-scope-${server.config.scope}`}>{server.config.scope === 'user' ? 'User' : 'Project'}</span></td>
              <td><span className="mcp-permission-cell">{server.permissionSummary}</span></td>
              <td><div className="mcp-row-actions">
                {!server.config.enabled
                  ? <button className="mcp-small-action" onClick={() => void setEnabled(server, true)} disabled={pending}>{pending ? <LoaderCircle size={13} className="spin" /> : <Plus size={13} />}Enable</button>
                  : server.status === 'connected'
                  ? <button className="mcp-small-action" onClick={() => void disconnect(server)} disabled={pending}>{pending ? <LoaderCircle size={13} className="spin" /> : <X size={13} />}Disconnect</button>
                  : <button className="mcp-small-action mcp-connect-action" onClick={() => void connect(server)} disabled={pending}>{pending ? <LoaderCircle size={13} className="spin" /> : server.trusted ? <PlugZap size={13} /> : <ShieldCheck size={13} />}{server.trusted ? 'Connect' : 'Trust & Connect'}</button>}
                {server.config.enabled && <button className="mcp-icon-action" onClick={() => void setEnabled(server, false)} disabled={pending} aria-label={`Disable ${server.config.name}`} title="Disable server"><X size={13} /></button>}
                <button className="mcp-icon-action" onClick={() => editServer(server)} disabled={pending} aria-label={`Edit ${server.config.name}`} title="Edit server"><RefreshCw size={13} /></button>
                <button className="mcp-icon-action mcp-delete-action" onClick={() => void remove(server)} disabled={pending} aria-label={`Delete ${server.config.name}`} title="Delete server"><Trash2 size={13} /></button>
              </div></td>
            </tr>;
          })}
        </tbody>
      </table></div>
    </section>

    {draft && <div className="mcp-form-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) setDraft(null); }}>
      <section className="mcp-form" role="dialog" aria-modal="true" aria-labelledby="mcp-form-title">
        <header><div><span className="mcp-form-kicker">External tool integration</span><h2 id="mcp-form-title">{draft.id ? 'Edit server' : 'Add MCP server'}</h2></div><button className="mcp-icon-action" onClick={() => setDraft(null)} aria-label="Close"><X size={15} /></button></header>
        <label>Display name<input value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value, id: draft.id || slug(event.target.value) })} placeholder="Documentation search" /></label>
        <div className="mcp-form-two"><label>Server ID<input value={draft.id} onChange={event => setDraft({ ...draft, id: slug(event.target.value) })} placeholder="docs-search" /></label><label>Configuration scope<select value={draft.scope} onChange={event => setDraft({ ...draft, scope: event.target.value as McpScope })}><option value="user">User</option><option value="project" disabled={!project}>Project {project ? `· ${project.name}` : '· choose a project first'}</option></select></label></div>
        <label>Transport<select value={draft.type} onChange={event => setDraft({ ...draft, type: event.target.value as Draft['type'] })}><option value="stdio">Local process · stdio</option><option value="http">Remote endpoint · Streamable HTTP</option></select></label>
        {draft.type === 'stdio' ? <><label>Program<input value={draft.command} onChange={event => setDraft({ ...draft, command: event.target.value })} placeholder="npx" autoComplete="off" /></label><label>Arguments <small>One argument per line; no shell is used.</small><textarea rows={3} value={draft.args} onChange={event => setDraft({ ...draft, args: event.target.value })} placeholder="-y&#10;@example/mcp-server" /></label></> : <><label>HTTPS endpoint<input value={draft.url} onChange={event => setDraft({ ...draft, url: event.target.value })} placeholder="https://tools.example.com/mcp" autoComplete="off" /></label><label className="mcp-checkbox-label"><input type="checkbox" checked={draft.hasAuthToken} onChange={event => setDraft({ ...draft, hasAuthToken: event.target.checked })} />Use bearer token from OS credential store</label>{draft.hasAuthToken && <label>Bearer token<input type="password" value={draft.token} onChange={event => setDraft({ ...draft, token: event.target.value })} autoComplete="new-password" placeholder="Leave blank to keep the saved token" /></label>}</>}
        {draft.type === 'stdio' && <><div className="mcp-env-heading"><div><strong>Environment secrets</strong><small>Values are stored in the OS credential store, never in project config.</small></div><button className="mcp-icon-action" onClick={() => setDraft({ ...draft, env: [...draft.env, { name: '', value: '' }] })} aria-label="Add environment variable"><Plus size={14} /></button></div>
        {draft.env.map((item, index) => <div className="mcp-env-row" key={`${index}-${item.name}`}><input aria-label="Environment variable name" value={item.name} onChange={event => updateEnv(draft, setDraft, index, 'name', event.target.value)} placeholder="API_TOKEN" autoComplete="off" /><input aria-label={`Secret value for ${item.name || 'environment variable'}`} type="password" value={item.value} onChange={event => updateEnv(draft, setDraft, index, 'value', event.target.value)} placeholder="Secret value" autoComplete="new-password" /><button className="mcp-icon-action" onClick={() => setDraft({ ...draft, env: draft.env.filter((_, row) => row !== index) })} aria-label="Remove environment variable"><X size={13} /></button></div>)}</>}
        <footer><span><ShieldAlert size={13} />Saving disables the server and clears its previous trust.</span><button className="mcp-cancel-button" onClick={() => setDraft(null)}>Cancel</button><button className="mcp-save-button" onClick={() => void save()} disabled={saving || !draft.name.trim() || !draft.id.trim()}>{saving ? <LoaderCircle size={14} className="spin" /> : <ShieldCheck size={14} />}Save configuration</button></footer>
      </section>
    </div>}
    {!desktopAvailable && <div className="mcp-browser-note">MCP server management is available in the JevCode desktop application.</div>}
  </section>;
}

function updateEnv(draft: Draft, setDraft: (value: Draft) => void, index: number, field: keyof SecretField, value: string) {
  const env = [...draft.env];
  env[index] = { ...env[index], [field]: field === 'name' ? value.replace(/[^A-Za-z0-9_]/g, '').toUpperCase() : value };
  setDraft({ ...draft, env });
}

function slug(value: string) { return value.toLowerCase().trim().replace(/[^a-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 64); }
function getKey(server: McpServerView) { return `${server.config.scope}:${server.config.projectId ?? ''}:${server.config.id}`; }
function actionInput(server: McpServerView) { return { scope: server.config.scope, projectId: server.config.projectId, serverId: server.config.id }; }
function formatStatus(status: McpServerView['status']) { return status === 'connected' ? 'Connected' : status === 'untrusted' ? 'Needs trust' : status === 'disabled' ? 'Disabled' : status === 'error' ? 'Error' : 'Disconnected'; }
