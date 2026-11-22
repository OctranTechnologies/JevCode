import { useEffect, useState } from 'react';
import { LoaderCircle, ShieldAlert } from 'lucide-react';
import { getContainmentStatus, setReducedSafetyAllowed } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import type { ContainmentStatus } from '../../types/domain';

/** The machine switch is pure so its checked and disabled states can be tested directly. */
export function MachineContainmentSwitch({ allowed, busy, disabled, onChange }: {
  allowed: boolean;
  busy: boolean;
  disabled: boolean;
  onChange: (enabled: boolean) => void;
}) {
  return <label className="update-preference-row containment-machine-switch">
    <span><strong>Allow reduced safety on this machine</strong><small>When off, no project can turn on reduced safety. Every agent command stays blocked.</small></span>
    <input
      type="checkbox"
      role="switch"
      aria-label="Allow reduced safety on this machine"
      checked={allowed}
      disabled={disabled || busy}
      onChange={event => onChange(event.currentTarget.checked)}
    />
  </label>;
}

export function ContainmentSettings({ projectId, onError }: { projectId: string | null; onError: (message: string) => void }) {
  const [status, setStatus] = useState<ContainmentStatus | null>(null);
  const [loading, setLoading] = useState(projectId !== null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let active = true;
    setStatus(null);
    if (!projectId) { setLoading(false); return () => { active = false; }; }
    setLoading(true);
    void getContainmentStatus(projectId)
      .then(next => { if (active) setStatus(next); })
      .catch(cause => { if (active) onError(normalizeError(cause).message); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [onError, projectId]);

  async function change(allowed: boolean) {
    if (busy) return;
    setBusy(true);
    try {
      const next = await setReducedSafetyAllowed(allowed);
      setStatus(current => current ? { ...current, machineAllowed: next.machineAllowed } : current);
    } catch (cause) {
      onError(normalizeError(cause).message);
    } finally {
      setBusy(false);
    }
  }

  return <section className="permission-mode-section" aria-labelledby="containment-machine-heading">
    <div className="permission-section-heading">
      <div><h2 id="containment-machine-heading">Reduced safety</h2><p>The Windows fallback for agent commands. It limits processes and memory but does not restrict files or the network.</p></div>
      <ShieldAlert size={17} aria-hidden="true" />
    </div>
    {loading ? <p className="permission-mode-description"><LoaderCircle size={13} className="spin" /> Loading the machine setting…</p>
      : status ? <MachineContainmentSwitch allowed={status.machineAllowed} busy={busy} disabled={false} onChange={enabled => void change(enabled)} />
        : <p className="permission-mode-description">Open a project to read the machine setting before changing it.</p>}
    <p className="permission-mode-description">Reduced safety is opt-in per project and needs a typed confirmation there. Turning this off blocks agent commands, Git, and search in every project; file edits still work.</p>
  </section>;
}
