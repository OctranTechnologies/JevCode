import { ArrowUp, Folder, Laptop, Paperclip, Square } from 'lucide-react';
import type { Provider } from '../../types/domain';
import { desktopAvailable } from '../../lib/ipc';

export function Composer({ draft, setDraft, providers, providerId, modelId, onProvider, onModel, onSubmit, onStop, busy, sending, hasProject, sessionLocked }: {
  draft: string; setDraft: (value: string) => void; providers: Provider[]; providerId: string; modelId: string;
  onProvider: (id: string) => void; onModel: (id: string) => void; onSubmit: () => void; onStop: () => void;
  busy: boolean; sending: boolean; hasProject: boolean; sessionLocked: boolean;
}) {
  const provider = providers.find(provider => provider.id === providerId);
  const unavailable = !desktopAvailable || !hasProject || !provider?.connected;
  return <div className="composer-region"><form className="composer" onSubmit={event => { event.preventDefault(); onSubmit(); }}>
    <label className="sr-only" htmlFor="prompt">Message JevCode</label>
    <textarea id="prompt" value={draft} onChange={event => setDraft(event.target.value)} placeholder={hasProject ? 'Ask JevCode to explore your project…' : 'Open a project folder to get started…'} rows={2} disabled={busy || sending} onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (!unavailable && !busy && !sending && draft.trim()) onSubmit(); } }} />
    <div className="composer-controls"><div className="model-selectors"><label className="sr-only" htmlFor="provider">Provider</label><span className="select-control"><Laptop size={16} /><select id="provider" value={providerId} onChange={event => onProvider(event.target.value)} disabled={sessionLocked || sending}>{providers.map(provider => <option key={provider.id} value={provider.id}>{provider.name}{provider.connected ? '' : ' · Connect'}</option>)}</select></span><label className="sr-only" htmlFor="model">Model</label><span className="select-control"><Folder size={16} /><select id="model" value={modelId} onChange={event => onModel(event.target.value)} disabled={sessionLocked || sending}>{provider?.models.map(model => <option key={model.id} value={model.id}>{model.name}</option>)}</select></span></div>
      <span className="context-label"><Paperclip size={17} /><span>Project context</span></span>
      {busy ? <button type="button" className="stop-button" onClick={onStop} aria-label="Stop agent run"><Square size={14} />Stop</button> : <button type="submit" className="send-button" disabled={unavailable || sending || !draft.trim()} aria-label="Send message"><ArrowUp size={18} /><span>{sending ? 'Sending' : 'Send'}</span></button>}
    </div>
  </form><p className="composer-hint">{!desktopAvailable ? 'Browser preview · Open the desktop app to use local projects.' : !hasProject ? 'Choose a local folder using Open folder.' : !provider?.connected ? 'Add an API key in Providers to connect this provider.' : provider?.protocol === 'preview' ? 'Local preview uses project tools without sending data to a provider.' : 'Messages and tool results are sent to your selected provider.'}</p></div>;
}
