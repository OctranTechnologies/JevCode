import { useState } from 'react';
import { Check, KeyRound, LockKeyhole, Trash2 } from 'lucide-react';
import type { Provider } from '../../types/domain';
import { command, desktopAvailable } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';

export function ProviderSettings({ providers, onUpdate }: { providers: Provider[]; onUpdate: (providers: Provider[]) => void }) {
  const [selected, setSelected] = useState(providers.find(provider => provider.protocol !== 'preview')?.id ?? '');
  const [secret, setSecret] = useState('');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const provider = providers.find(provider => provider.id === selected);
  async function update(remove: boolean) {
    setBusy(true); setError(null); setNotice(null);
    try {
      const updated = remove ? await command('delete_credential', { providerId: selected }) : await command('save_credential', { providerId: selected, secret });
      setSecret(''); onUpdate(updated); setNotice(remove ? 'API key removed from your OS keychain.' : 'API key saved to your OS keychain. Start a session to verify provider access.');
    } catch (error) { setError(normalizeError(error).message); }
    finally { setBusy(false); }
  }
  return <section className="settings-content"><h1>Providers</h1><p className="page-description">Choose your models. Keep your credentials on your computer.</p>
    <div className="provider-list">{providers.map(provider => <button key={provider.id} className={selected === provider.id ? 'provider-row selected' : 'provider-row'} disabled={provider.protocol === 'preview' || busy} onClick={() => { setSelected(provider.id); setSecret(''); setNotice(null); setError(null); }}><span className="provider-initial">{provider.name.charAt(0)}</span><span>{provider.name}<small>{provider.models.map(model => model.name).join(', ')}</small></span><span className={provider.connected ? 'connection connected' : 'connection'}>{provider.connected ? <><Check size={13} />{provider.protocol === 'preview' ? 'Available' : 'Key saved'}</> : 'Not connected'}</span></button>)}</div>
    {provider && <form className="credential-form" onSubmit={event => { event.preventDefault(); void update(false); }}><h2><KeyRound size={18} />Connect {provider.name}</h2><p>Your API key is stored in the OS credential manager. It is used by the desktop service when you send a message.</p><label htmlFor="api-key">API key</label><input id="api-key" type="password" autoComplete="off" spellCheck={false} value={secret} onChange={event => setSecret(event.target.value)} placeholder="Enter your API key" disabled={busy || !desktopAvailable} /><div className="credential-actions"><button type="submit" className="primary-button" disabled={!desktopAvailable || busy || !secret.trim()}>{busy ? 'Saving…' : 'Save API key'}</button>{provider.connected && <button type="button" className="secondary-button" disabled={!desktopAvailable || busy} onClick={() => void update(true)}><Trash2 size={14} />Remove key</button>}</div>{notice && <p className="notice" role="status">{notice}</p>}{error && <p className="inline-error" role="alert">{error}</p>}</form>}
    <p className="privacy-note"><LockKeyhole size={15} />Provider access is verified on your first request. Model catalogs can be extended in configuration.</p>
  </section>;
}
