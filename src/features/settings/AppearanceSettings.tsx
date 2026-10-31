import { Check, Eye, SlidersHorizontal } from 'lucide-react';
import type { InterfaceMode } from '../../types/domain';

export function AppearanceSettings({ mode, onMode }: { mode: InterfaceMode; onMode: (mode: InterfaceMode) => void }) {
  return <section className="settings-content appearance-settings" aria-labelledby="appearance-title">
    <div className="settings-heading-row"><div><h1 id="appearance-title">Appearance</h1><p className="page-description">Choose how much of JevCode’s workspace is visible at once.</p></div></div>
    <div className="appearance-mode-list" aria-label="Interface density">
      <button type="button" aria-pressed={mode === 'simple'} className={`appearance-mode-option${mode === 'simple' ? ' is-selected' : ''}`} onClick={() => onMode('simple')}>
        <span className="appearance-mode-icon"><Eye size={17} /></span><span className="appearance-mode-copy"><strong>Simple</strong><small>Keep the workspace breathable. Advanced controls are grouped under options.</small></span>{mode === 'simple' && <Check size={16} className="appearance-mode-check" />}
      </button>
      <button type="button" aria-pressed={mode === 'maximum'} className={`appearance-mode-option${mode === 'maximum' ? ' is-selected' : ''}`} onClick={() => onMode('maximum')}>
        <span className="appearance-mode-icon"><SlidersHorizontal size={17} /></span><span className="appearance-mode-copy"><strong>Maximum</strong><small>Show workspace, context, activity, and task controls directly.</small></span>{mode === 'maximum' && <Check size={16} className="appearance-mode-check" />}
      </button>
    </div>
    <p className="appearance-theme-note">Changing this setting does not change your light or dark theme.</p>
  </section>;
}
