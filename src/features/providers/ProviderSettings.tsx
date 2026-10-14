import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Check, CircleAlert, CircleCheck, CircleX, KeyRound, LoaderCircle, RotateCw, ShieldCheck, Trash2, Waypoints } from 'lucide-react';
import type { Model, OpenAIAuthAttemptEvent, OpenAIAuthStatus, Provider, ProviderAccount, ProviderAuthMethod } from '../../types/domain';
import { cancelOpenAIAuth, desktopAvailable, getOpenAIAuthStatus, logoutOpenAIAuth, openAIAuthAttemptEventSchema, providerAuthAdapter, startOpenAIAuth } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';

const apiKeyMethod: ProviderAuthMethod = 'api_key';

function accountFor(provider: Provider, accounts: ProviderAccount[]): ProviderAccount {
  return accounts.find(account => account.providerId === provider.id) ?? {
    providerId: provider.id,
    providerName: provider.name,
    state: 'not_connected',
    authMethod: null,
    accountLabel: null,
    connectedAt: null,
    lastValidatedAt: null,
    lastErrorCode: null,
    availableMethods: [],
  };
}

function statusLabel(account: ProviderAccount): string {
  if (account.state === 'connected') return 'Connected';
  if (account.state === 'needs_attention') return attentionLabel(account.lastErrorCode);
  return 'Not connected';
}

function attentionLabel(code: string | null): string {
  switch (code) {
    case 'expired_authentication': return 'Sign-in expired';
    case 'invalid_credential': return 'Key rejected';
    case 'quota_exhausted': return 'Usage limit reached';
    case 'subscription_unavailable': return 'Access unavailable';
    case 'network_error': return 'Offline';
    case 'provider_outage': return 'Provider unavailable';
    default: return 'Needs attention';
  }
}

export function ProviderSettings({
  providers,
  accounts,
  onAccount,
  onModels,
}: {
  providers: Provider[];
  accounts: ProviderAccount[];
  onAccount: (account: ProviderAccount) => void;
  onModels: (providerId: string, models: Model[]) => void;
}) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [secret, setSecret] = useState('');
  const [busyId, setBusyId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [models, setModels] = useState<Record<string, string[]>>({});
  const [openAIStatus, setOpenAIStatus] = useState<OpenAIAuthStatus | null>(null);
  const [openAIAttemptId, setOpenAIAttemptId] = useState<string | null>(null);
  const [openAIBusy, setOpenAIBusy] = useState(false);

  async function refreshOpenAIStatus() {
    try { setOpenAIStatus(await getOpenAIAuthStatus()); }
    catch (cause) { setError(normalizeError(cause).message); }
  }

  useEffect(() => {
    if (!desktopAvailable) return;
    let active = true;
    void getOpenAIAuthStatus().then(status => { if (active) setOpenAIStatus(status); }).catch(cause => { if (active) setError(normalizeError(cause).message); });
    let unlisten: (() => void) | undefined;
    void listen<unknown>('openai:auth-attempt', event => {
      const parsed = openAIAuthAttemptEventSchema.safeParse(event.payload);
      if (!parsed.success || !active) return;
      const attempt = parsed.data as OpenAIAuthAttemptEvent;
      setOpenAIAttemptId(current => current === attempt.attemptId ? null : current);
      setOpenAIBusy(false);
      if (attempt.state === 'callback_received') {
        void getOpenAIAuthStatus().then(status => { if (active) setOpenAIStatus(status); });
        setNotice('ChatGPT account connected. Credentials are stored in your OS credential manager.');
      } else if (attempt.state === 'denied') setError('OpenAI sign-in was cancelled before access was granted.');
      else if (attempt.state === 'timed_out') setError('OpenAI sign-in timed out. Start again when you’re ready.');
      else if (attempt.state === 'failed') setError('OpenAI sign-in could not be verified. Start a new attempt.');
      else if (attempt.state === 'cancelled') setNotice('OpenAI sign-in cancelled.');
    }).then(stop => { if (active) unlisten = stop; else stop(); });
    return () => { active = false; unlisten?.(); };
  }, []);

  function resetFeedback() {
    setNotice(null);
    setError(null);
  }

  async function connect(provider: Provider) {
    setBusyId(provider.id);
    resetFeedback();
    try {
      const account = await providerAuthAdapter(provider.id).connect(secret);
      onAccount(account);
      try {
        const available = await providerAuthAdapter(provider.id).getAvailableModels();
        if (available.length) onModels(provider.id, available);
      } catch { /* The validated key is connected; catalog refresh can be retried from Accounts. */ }
      setSecret('');
      setEditingId(null);
      setNotice(`${provider.name} connection tested and saved in your OS credential manager.`);
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setBusyId(null);
    }
  }

  async function validate(provider: Provider) {
    setBusyId(provider.id);
    resetFeedback();
    try {
      const account = await providerAuthAdapter(provider.id).validate();
      onAccount(account);
      setNotice(`${provider.name} connection is healthy.`);
    } catch (cause) {
      setError(normalizeError(cause).message);
      try { onAccount(await providerAuthAdapter(provider.id).getAuthStatus()); } catch { /* Keep the useful validation error visible. */ }
    } finally {
      setBusyId(null);
    }
  }

  async function disconnect(provider: Provider) {
    setBusyId(provider.id);
    resetFeedback();
    try {
      const account = await providerAuthAdapter(provider.id).disconnect();
      onAccount(account);
      setModels(current => { const next = { ...current }; delete next[provider.id]; return next; });
      setNotice(`${provider.name} key removed from your OS credential manager.`);
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setBusyId(null);
    }
  }

  async function loadModels(provider: Provider) {
    setBusyId(provider.id);
    resetFeedback();
    try {
      const available = await providerAuthAdapter(provider.id).getAvailableModels();
      setModels(current => ({ ...current, [provider.id]: available.map(model => model.displayName) }));
      onModels(provider.id, available);
      setNotice(`${available.length} models available from ${provider.name}.`);
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setBusyId(null);
    }
  }

  async function connectOpenAIChatGPT(accountId?: string) {
    setOpenAIBusy(true);
    resetFeedback();
    try {
      const started = await startOpenAIAuth(accountId);
      setOpenAIAttemptId(started.attemptId);
      setNotice(accountId
        ? 'Continue sign-in in your browser to reconnect this ChatGPT account.'
        : 'Continue sign-in in your browser. JevCode will verify your account before connecting it.');
    } catch (cause) {
      setOpenAIBusy(false);
      setError(normalizeError(cause).message);
    }
  }

  async function cancelOpenAIChatGPT() {
    if (!openAIAttemptId) return;
    setOpenAIBusy(true);
    try { await cancelOpenAIAuth(openAIAttemptId); setOpenAIAttemptId(null); setOpenAIBusy(false); }
    catch (cause) { setOpenAIBusy(false); setError(normalizeError(cause).message); }
  }

  async function disconnectOpenAIAccount(accountId: string) {
    setOpenAIBusy(true);
    resetFeedback();
    try {
      await logoutOpenAIAuth(accountId);
      await refreshOpenAIStatus();
      setNotice('ChatGPT credentials removed from your OS credential manager.');
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setOpenAIBusy(false); }
  }

  const remoteProviders = providers.filter(provider => provider.protocol !== 'preview');

  return <section className="settings-content accounts-settings">
    <div className="accounts-heading">
      <div>
        <h1>Accounts &amp; providers</h1>
        <p className="page-description">Connect the model providers you use. JevCode tests keys in the desktop service and stores them in your OS credential manager.</p>
      </div>
      <span className="accounts-security"><ShieldCheck size={14} />Local credentials</span>
    </div>

    {notice && <p className="accounts-feedback success" role="status"><Check size={14} />{notice}</p>}
    {error && <p className="accounts-feedback failure" role="alert"><CircleAlert size={14} />{error}</p>}

    <div className="account-list" aria-label="Model provider accounts">
      {remoteProviders.map(provider => {
        const account = accountFor(provider, accounts);
        const connected = account.state !== 'not_connected';
        const editing = editingId === provider.id;
        const busy = busyId === provider.id;
        const keyOption = account.availableMethods.find(method => method.method === apiKeyMethod);
        const oauthOptions = account.availableMethods.filter(method => method.method !== apiKeyMethod);
        const connectedLabel = account.authMethod === 'api_key' ? 'API key' : account.accountLabel;

        return <article className={`account-row${editing ? ' is-editing' : ''}`} key={provider.id}>
          <div className="account-main">
            <span className="account-monogram" aria-hidden="true">{provider.name.split(/\s+/).map(part => part[0]).join('').slice(0, 2)}</span>
            <div className="account-copy">
              <div className="account-title-line"><h2>{provider.name}</h2><span className={`account-status ${account.state}`}>
                {account.state === 'connected' ? <CircleCheck size={13} /> : account.state === 'needs_attention' ? <CircleAlert size={13} /> : <CircleX size={13} />}
                {statusLabel(account)}
              </span></div>
              <p>{connected ? `${connectedLabel ?? 'Provider account'}${account.lastValidatedAt ? ` · Checked ${new Date(account.lastValidatedAt).toLocaleString()}` : ''}` : keyOption?.label ?? 'Provider API key'}</p>
            </div>
            <div className="account-actions">
              {connected ? <>
                <button className="account-button quiet" onClick={() => void loadModels(provider)} disabled={!desktopAvailable || busy}>
                  {busy ? <LoaderCircle size={13} className="spin" /> : <Waypoints size={13} />}Models
                </button>
                <button className="account-button quiet" onClick={() => void validate(provider)} disabled={!desktopAvailable || busy}>
                  {busy ? <LoaderCircle size={13} className="spin" /> : <RotateCw size={13} />}Test
                </button>
                <button className="account-button remove" onClick={() => void disconnect(provider)} disabled={!desktopAvailable || busy} aria-label={`Disconnect ${provider.name}`}>
                  {busy ? <LoaderCircle size={13} className="spin" /> : <Trash2 size={13} />}Disconnect
                </button>
              </> : <button className="account-button connect" onClick={() => { setEditingId(editing ? null : provider.id); setSecret(''); resetFeedback(); }} disabled={!desktopAvailable || busy}>
                <KeyRound size={13} />Connect
              </button>}
            </div>
          </div>

          {editing && <form className="account-connect-form" onSubmit={event => { event.preventDefault(); void connect(provider); }}>
            <div className="account-form-copy">
              <strong>{keyOption?.label ?? 'API key'}</strong>
              <span>The key is checked first. JevCode saves it only after the provider accepts it.</span>
            </div>
            <label className="sr-only" htmlFor={`api-key-${provider.id}`}>API key for {provider.name}</label>
            <input id={`api-key-${provider.id}`} type="password" autoComplete="off" spellCheck={false} value={secret} onChange={event => setSecret(event.target.value)} placeholder="Paste an API key" disabled={busy} autoFocus />
            <div className="account-form-actions">
              <button type="submit" className="account-button connect" disabled={!desktopAvailable || busy || !secret.trim()}>
                {busy ? <LoaderCircle size={13} className="spin" /> : <ShieldCheck size={13} />}{busy ? 'Testing…' : 'Test and connect'}
              </button>
              <button type="button" className="account-button quiet" onClick={() => { setEditingId(null); setSecret(''); resetFeedback(); }} disabled={busy}>Cancel</button>
            </div>
          </form>}

          {provider.id === 'openai' && <div className="openai-oauth-section">
            <div className="openai-oauth-heading"><div><strong>Sign in with ChatGPT</strong><small>Official browser sign-in · tokens stay in native secure storage</small></div>
              {openAIAttemptId
                ? <button className="account-button quiet" onClick={() => void cancelOpenAIChatGPT()} disabled={openAIBusy}><CircleX size={13} />Cancel sign-in</button>
                : <button className="account-button connect" onClick={() => void connectOpenAIChatGPT()} disabled={!desktopAvailable || openAIBusy}><ShieldCheck size={13} />{openAIBusy ? 'Starting…' : 'Continue with ChatGPT'}</button>}
            </div>
            {openAIStatus?.chatgpt.connectedAccounts.map(chatgpt => <div className="openai-oauth-account" key={chatgpt.id}>
              <span className={'account-status ' + (chatgpt.connected ? 'connected' : 'needs_attention')}>{chatgpt.connected ? <CircleCheck size={13} /> : <CircleAlert size={13} />}{chatgpt.connected ? 'Connected' : 'Expired'}</span>
              <span className="openai-profile">{chatgpt.displayName ?? chatgpt.email ?? 'ChatGPT account'}{chatgpt.displayName && chatgpt.email ? <small>{chatgpt.email}</small> : null}</span>
              <span className="openai-scopes">{chatgpt.connected
                ? (chatgpt.grantedScopes.includes('chatgpt.tokens.use.direct') ? 'Plan access enabled' : 'Identity only')
                : <button className="account-button quiet" onClick={() => void connectOpenAIChatGPT(chatgpt.id)} disabled={openAIBusy}>Reconnect</button>}</span>
              <button className="account-button remove" onClick={() => void disconnectOpenAIAccount(chatgpt.id)} disabled={openAIBusy} aria-label={'Disconnect ' + (chatgpt.email ?? 'ChatGPT account')}><Trash2 size={13} />Disconnect</button>
            </div>)}
            {openAIStatus?.chatgpt.connectedAccounts.length === 0 && <p className="openai-oauth-empty">No ChatGPT accounts connected.</p>}
            {openAIStatus?.chatgpt.unavailableReason && <p className="openai-oauth-empty">{openAIStatus.chatgpt.unavailableReason}</p>}
          </div>}
          {provider.id !== 'openai' && oauthOptions.map(option => <div className="account-method-note" key={option.method}>
            <span>{option.label}</span><small>{option.available ? 'Available' : option.unavailableReason}</small>
          </div>)}

          {models[provider.id] && <div className="account-model-list" aria-live="polite">
            <span>Available models</span>
            <p>{models[provider.id].length ? models[provider.id].join(' · ') : 'No models were returned for this account.'}</p>
          </div>}
        </article>;
      })}
    </div>

    <div className="accounts-policy-note"><ShieldCheck size={15} /><p>Anthropic uses Console API keys here; JevCode does not reuse Claude Code subscription credentials. OAuth tokens and API keys never enter SQLite, browser storage, or application logs.</p></div>
    {!desktopAvailable && <p className="accounts-preview-note">Run JevCode as a desktop app to connect and test accounts.</p>}
  </section>;
}
