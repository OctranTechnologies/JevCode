import { useCallback, useEffect, useState } from 'react';
import { LoaderCircle, RefreshCw, ShieldAlert, ShieldCheck, Trash2 } from 'lucide-react';
import { command, desktopAvailable } from '../../lib/ipc';
import type { PermissionMode, PermissionRule, Project } from '../../types/domain';

const modeDetails: Record<PermissionMode, string> = {
  ask: 'Read project files and Git metadata. Ask before edits, commands, network access, or anything outside the workspace.',
  workspace_write: 'Allow project file edits. Ask before terminal commands, network access, or anything outside the workspace.',
  full_access: 'Allow project edits and ordinary commands, including network access. Destructive, credential, and outside-workspace actions still need fresh review.',
};

export function PermissionsSettings({
  mode, projects, onMode, onError,
}: {
  mode: PermissionMode;
  projects: Project[];
  onMode: (mode: PermissionMode) => void;
  onError: (error: string) => void;
}) {
  const [rules, setRules] = useState<PermissionRule[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [revoking, setRevoking] = useState<string | null>(null);

  const reload = useCallback(async (background = false) => {
    if (!desktopAvailable) { setLoading(false); return; }
    if (background) setRefreshing(true);
    else setLoading(true);
    try { setRules(await command('list_permission_rules', undefined)); }
    catch (error) { onError(error instanceof Error ? error.message : 'Permission rules could not be loaded.'); }
    finally { setLoading(false); setRefreshing(false); }
  }, [onError]);

  useEffect(() => { void reload(); }, [reload]);

  async function changeMode(value: PermissionMode) {
    if (!desktopAvailable || value === mode) return;
    try { onMode(await command('set_permission_mode', { mode: value })); }
    catch (error) { onError(error instanceof Error ? error.message : 'Permission mode could not be changed.'); }
  }

  async function revoke(rule: PermissionRule) {
    setRevoking(rule.id);
    try {
      await command('revoke_permission_rule', { ruleId: rule.id });
      setRules(current => current.filter(item => item.id !== rule.id));
    } catch (error) { onError(error instanceof Error ? error.message : 'Permission rule could not be revoked.'); }
    finally { setRevoking(null); }
  }

  return <section className="settings-content permissions-settings">
    <h1>Permissions</h1>
    <p className="page-description">Choose how JevCode handles routine work. Sensitive actions keep their own review step.</p>

    <section className="permission-mode-section" aria-labelledby="permission-mode-heading">
      <div className="permission-section-heading">
        <div><h2 id="permission-mode-heading">Default mode</h2><p>Applied to new tasks. Project rules can add tighter restrictions.</p></div>
        <ShieldCheck size={17} aria-hidden="true" />
      </div>
      <label className="permission-mode-control" htmlFor="permission-mode-select">
        <span>Permission mode</span>
        <select id="permission-mode-select" value={mode} disabled={!desktopAvailable} onChange={event => void changeMode(event.target.value as PermissionMode)}>
          <option value="ask">Ask</option>
          <option value="workspace_write">Workspace Write</option>
          <option value="full_access">Full Access</option>
        </select>
      </label>
      <p className="permission-mode-description">{modeDetails[mode]}</p>
      <div className="permission-safety-note"><ShieldAlert size={15} /><span>Commands that may delete data, access credentials, leave the workspace, or affect system settings always need fresh approval. Agent commands never elevate privileges.</span></div>
    </section>

    <section className="persistent-rules-section" aria-labelledby="permission-rules-heading">
      <div className="permission-section-heading">
        <div><h2 id="permission-rules-heading">Always allow for a project</h2><p>These exact operations can run without another prompt. Revoke a rule at any time.</p></div>
        <button className="quiet-icon-button" onClick={() => void reload(true)} disabled={!desktopAvailable || loading || refreshing} aria-label="Refresh permission rules" title="Refresh rules">
          {refreshing ? <LoaderCircle size={14} className="spin" /> : <RefreshCw size={14} />}
        </button>
      </div>
      {loading ? <div className="permission-rules-loading" role="status"><span /><span /><span /></div> : !desktopAvailable ?
        <div className="permission-rules-empty"><ShieldCheck size={17} /><p>Persistent project rules are available in the JevCode desktop app.</p></div> : rules.length === 0 ?
          <div className="permission-rules-empty"><ShieldCheck size={17} /><p>No persistent permission rules yet. Choose “Always allow for this project” from an approval request to add one.</p></div> :
          <ul className="permission-rule-list">{rules.map(rule => {
            const projectName = projects.find(project => project.id === rule.projectId)?.name ?? 'Unavailable project';
            return <li className="permission-rule-row" key={rule.id}>
              <div className="permission-rule-mark"><ShieldCheck size={14} /></div>
              <div className="permission-rule-copy">
                <strong title={rule.summary}>{rule.summary}</strong>
                <span>{rule.categories.map(formatCategory).join(' · ')} <i>in</i> {projectName}</span>
                <time dateTime={rule.createdAt}>Added {new Date(rule.createdAt).toLocaleDateString()}</time>
              </div>
              <button className="permission-revoke-button" onClick={() => void revoke(rule)} disabled={revoking === rule.id} aria-label={`Revoke permission for ${rule.summary}`}>
                {revoking === rule.id ? <LoaderCircle size={14} className="spin" /> : <Trash2 size={14} />}<span>Revoke</span>
              </button>
            </li>;
          })}</ul>}
    </section>
  </section>;
}

function formatCategory(category: PermissionRule['categories'][number]) {
  return category === 'read' ? 'Read' : category === 'write' ? 'Write' : category === 'command' ? 'Command' : category === 'network' ? 'Network' : 'Dangerous';
}
