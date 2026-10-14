import { useEffect, useReducer, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Check, CircleAlert, CircleCheck, CircleX, ExternalLink, KeyRound, LoaderCircle, RotateCw, ShieldCheck, Trash2, Waypoints } from 'lucide-react';
import type { Model, OpenAIAuthAttemptEvent, OpenAIAuthStatus, Provider, ProviderAccount, ProviderAuthMethod } from '../../types/domain';
import { cancelOpenAIAuth, desktopAvailable, getOpenAIAuthStatus, logoutOpenAIAuth, openAIAuthAttemptEventSchema, providerAuthAdapter, startOpenAIAuth } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import { initialOpenAIAuthFlow, openAIAccountNeedsReconnect, openAIAccountStatus, openAIAuthFlowReducer, openAIAuthStartErrorMessage } from './openaiAuthUi';

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
  initialOpenAIStatus,
  onAccount,
  onModels,
}: {
  providers: Provider[];
  accounts: ProviderAccount[];
  initialOpenAIStatus?: OpenAIAuthStatus;
  onAccount: (account: ProviderAccount) => void;
  onModels: (providerId: string, models: Model[]) => void;
}) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [secret, setSecret] = useState('');
  const [busyId, setBusyId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [models, setModels] = useState<Record<string, string[]>>({});
  const [openAIStatus, setOpenAIStatus] = useState<OpenAIAuthStatus | null>(initialOpenAIStatus ?? null);
  const [openAIStatusLoading, setOpenAIStatusLoading] = useState(initialOpenAIStatus === undefined && desktopAvailable);
  const [openAIStatusError, setOpenAIStatusError] = useState<string | null>(null);
  const [openAIFlow, dispatchOpenAIFlow] = useReducer(openAIAuthFlowReducer, initialOpenAIAuthFlow);
  const [managedOpenAIAccountId, setManagedOpenAIAccountId] = useState<string | null>(null);

  async function refreshOpenAIStatus() {
    setOpenAIStatusLoading(true);
    setOpenAIStatusError(null);
    try { setOpenAIStatus(await getOpenAIAuthStatus()); }
    catch { setOpenAIStatusError('OpenAI account status could not be loaded. Check your connection and retry.'); }
    finally { setOpenAIStatusLoading(false); }
  }

  useEffect(() => {
    if (!desktopAvailable) return;
    let active = true;
    void getOpenAIAuthStatus().then(status => { if (active) { setOpenAIStatus(status); setOpenAIStatusError(null); } }).catch(() => { if (active) setOpenAIStatusError('OpenAI account status could not be loaded. Check your connection and retry.'); }).finally(() => { if (active) setOpenAIStatusLoading(false); });
    let unlisten: (() => void) | undefined;
    void listen<unknown>('openai:auth-attempt', event => {
      const parsed = openAIAuthAttemptEventSchema.safeParse(event.payload);
      if (!parsed.success || !active) return;
      const attempt = parsed.data as OpenAIAuthAttemptEvent;
      dispatchOpenAIFlow({ type: 'attempt-finished', event: attempt });
      if (attempt.state === 'callback_received') {
        void getOpenAIAuthStatus().then(status => { if (active) { setOpenAIStatus(status); setOpenAIStatusError(null); } }).catch(() => { if (active) setOpenAIStatusError('OpenAI connected, but the account details could not be refreshed. Retry status below.'); });
        void providerAuthAdapter('openai').getAvailableModels().then(available => {
          if (!active) return;
          setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
          onModels('openai', available);
          setNotice(`${available.length} models loaded from your ChatGPT account.`);
          setError(null);
        }).catch(cause => {
          if (active) setError(`ChatGPT sign-in succeeded, but OpenAI models could not be loaded. ${normalizeError(cause).message}`);
        });
      }
    }).then(stop => { if (active) unlisten = stop; else stop(); });
    return () => { active = false; unlisten?.(); };
  }, [onModels]);

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
      const chatgptRouteActive = provider.id === 'openai' && !!openAIStatus?.chatgpt.connectedAccounts.length;
      if (!chatgptRouteActive) {
        try {
          const available = await providerAuthAdapter(provider.id).getAvailableModels();
          if (available.length) onModels(provider.id, available);
        } catch { /* The validated key is connected; catalog refresh can be retried from Accounts. */ }
      }
      setSecret('');
      setEditingId(null);
      setNotice(chatgptRouteActive
        ? 'The OpenAI API key was tested and saved. ChatGPT OAuth remains the active model route until you disconnect it.'
        : `${provider.name} connection tested and saved in your OS credential manager.`);
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
    dispatchOpenAIFlow({ type: 'begin' });
    try {
      const started = await startOpenAIAuth(accountId);
      dispatchOpenAIFlow({ type: 'started', attemptId: started.attemptId });
    } catch (cause) {
      dispatchOpenAIFlow({ type: 'start-failed', message: openAIAuthStartErrorMessage(normalizeError(cause).code) });
    }
  }

  async function cancelOpenAIChatGPT() {
    const attemptId = openAIFlow.attemptId;
    if (!attemptId) return;
    dispatchOpenAIFlow({ type: 'begin-cancel' });
    try {
      await cancelOpenAIAuth(attemptId);
      dispatchOpenAIFlow({ type: 'attempt-finished', event: { attemptId, state: 'cancelled', failureReason: null } });
    } catch (cause) {
      dispatchOpenAIFlow({ type: 'cancel-failed', message: openAIAuthStartErrorMessage(normalizeError(cause).code) });
    }
  }

  async function disconnectOpenAIAccount(accountId: string) {
    resetFeedback();
    try {
      await logoutOpenAIAuth(accountId);
      await refreshOpenAIStatus();
      try {
        const available = await providerAuthAdapter('openai').getAvailableModels();
        setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
        onModels('openai', available);
      } catch { /* The account is disconnected; model refresh can be retried later. */ }
      setNotice('ChatGPT credentials removed from your OS credential manager.');
    } catch (cause) { setError(normalizeError(cause).message); }
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
        const editing = editingId === provider.id;
        const busy = busyId === provider.id;
        const keyOption = account.availableMethods.find(method => method.method === apiKeyMethod);
        const oauthOptions = account.availableMethods.filter(method => method.method !== apiKeyMethod);
        const connectedLabel = account.authMethod === 'api_key' ? 'API key' : account.accountLabel;

        if (provider.id === 'openai') {
          const chatgptAccounts = openAIStatus?.chatgpt.connectedAccounts ?? [];
          const activeChatgptAccount = [...chatgptAccounts].sort((left, right) => right.connectedAt.localeCompare(left.connectedAt))[0];
          const chatgptConnected = !!activeChatgptAccount && !openAIAccountNeedsReconnect(activeChatgptAccount);
          const chatgptNeedsReconnect = !!activeChatgptAccount && openAIAccountNeedsReconnect(activeChatgptAccount);
          const keyConnected = account.state === 'connected';
          const keyNeedsAttention = account.state === 'needs_attention';
          const connected = keyConnected || chatgptConnected;
          const needsAttention = !connected && (keyNeedsAttention || chatgptNeedsReconnect);
          const aggregateState = connected ? 'connected' : needsAttention ? 'needs_attention' : 'not_connected';
          const chatGPTAvailable = openAIStatus?.chatgpt.available ?? desktopAvailable;
          const showConnectionChoices = !openAIStatusLoading && !openAIStatusError && chatgptAccounts.length === 0 && !keyConnected && !keyNeedsAttention && !editing;
          const flowActive = openAIFlow.phase !== 'idle';

          return <article className={`account-row openai-provider-card${editing ? ' is-editing' : ''}`} key={provider.id}>
            <div className="openai-provider-heading">
              <span className="account-monogram" aria-hidden="true">O</span>
              <div className="openai-provider-copy">
                <div className="account-title-line">
                  <h2>OpenAI</h2>
                  <span className={`account-status ${aggregateState}`}>
                    {aggregateState === 'connected' ? <CircleCheck size={13} /> : aggregateState === 'needs_attention' ? <CircleAlert size={13} /> : <CircleX size={13} />}
                    {connected ? 'Connected' : needsAttention ? 'Reconnect needed' : 'Not connected'}
                  </span>
                </div>
                <p>{showConnectionChoices ? 'Use your ChatGPT/OpenAI account to connect OpenAI models.' : 'Choose how JevCode authenticates OpenAI model requests.'}</p>
              </div>
            </div>

            {openAIStatusLoading && <div className="openai-status-loading" role="status"><LoaderCircle size={14} className="spin" />Loading OpenAI account status…</div>}
            {openAIStatusError && <div className="openai-auth-feedback error" role="alert"><CircleAlert size={14} /><span>{openAIStatusError}</span><button className="account-button quiet" onClick={() => void refreshOpenAIStatus()} disabled={openAIStatusLoading}>Retry</button></div>}
            {openAIStatus?.chatgpt.unavailableReason && <div className="openai-auth-feedback error" role="alert"><CircleAlert size={14} /><span>{openAIStatus.chatgpt.unavailableReason}</span></div>}
            {chatgptAccounts.length > 0 && <div className="openai-auth-routing-note" role="status">
              {chatgptNeedsReconnect
                ? 'ChatGPT OAuth remains the selected route for OpenAI requests until it is disconnected. Requests will pause for reconnection; JevCode will not fall back to API-key billing.'
                : 'OpenAI model requests use ChatGPT OAuth while it is connected. A saved API key is not used as an automatic fallback.'}
            </div>}

            {openAIFlow.feedback && openAIFlow.phase === 'idle' && <div className={`openai-auth-feedback ${openAIFlow.feedback.tone}`} role={openAIFlow.feedback.tone === 'error' ? 'alert' : 'status'}>
              {openAIFlow.feedback.tone === 'success' ? <CircleCheck size={14} /> : openAIFlow.feedback.tone === 'error' ? <CircleAlert size={14} /> : <Check size={14} />}
              <span>{openAIFlow.feedback.message}</span>
            </div>}

            {openAIFlow.phase === 'starting' && <div className="openai-auth-waiting" role="status">
              <LoaderCircle size={15} className="spin" /><span>Opening your browser…</span>
            </div>}
            {(openAIFlow.phase === 'waiting' || openAIFlow.phase === 'cancelling') && <div className="openai-auth-waiting" role="status">
              {openAIFlow.phase === 'cancelling' ? <LoaderCircle size={15} className="spin" /> : <ExternalLink size={15} />}
              <div><strong>{openAIFlow.phase === 'cancelling' ? 'Cancelling sign-in…' : 'Waiting for login...'}</strong><span>Your browser was opened. Complete sign-in there; JevCode will verify the result.</span></div>
              <button className="account-button quiet" onClick={() => void cancelOpenAIChatGPT()} disabled={openAIFlow.phase === 'cancelling'}>
                {openAIFlow.phase === 'cancelling' ? 'Cancelling…' : 'Cancel'}
              </button>
            </div>}

            {showConnectionChoices && <div className="openai-connection-choices">
              <button className="openai-chatgpt-action" onClick={() => void connectOpenAIChatGPT()} disabled={!desktopAvailable || !chatGPTAvailable || flowActive}>
                {openAIFlow.phase === 'starting' ? <LoaderCircle size={15} className="spin" /> : <ShieldCheck size={15} />}
                Continue with ChatGPT
              </button>
              <span className="openai-choice-divider" aria-hidden="true">or</span>
              <button className="openai-apikey-action" onClick={() => { setEditingId(provider.id); setSecret(''); resetFeedback(); }} disabled={!desktopAvailable || flowActive}>
                <KeyRound size={15} />Use API Key
              </button>
            </div>}

            {!openAIStatusLoading && openAIStatus && !showConnectionChoices && <div className="openai-method-list" aria-label="OpenAI authentication methods">
              {chatgptAccounts.length > 0 ? chatgptAccounts.map(chatgpt => {
                const accountStatus = openAIAccountStatus(chatgpt);
                const reconnect = accountStatus.tone === 'needs_attention';
                const isManaged = managedOpenAIAccountId === chatgpt.id;
                return <div className="openai-method-group" key={chatgpt.id}>
                  <div className="openai-method-row">
                    <div className="openai-method-status">
                      <span>ChatGPT OAuth</span>
                      <strong className={`account-status ${accountStatus.tone}`}>
                        {reconnect ? <CircleAlert size={13} /> : <CircleCheck size={13} />}{accountStatus.label}
                      </strong>
                    </div>
                    <div className="openai-method-account">
                      <span>{chatgpt.displayName ?? 'ChatGPT account'}</span>
                      {chatgpt.email && <small>{chatgpt.email}</small>}
                      {accountStatus.detail && <small>{accountStatus.detail}</small>}
                    </div>
                    <div className="openai-method-actions">
                      {reconnect && <button className="account-button connect" onClick={() => void connectOpenAIChatGPT(chatgpt.id)} disabled={!desktopAvailable || flowActive}>Reconnect</button>}
                      <button className="account-button quiet" onClick={() => setManagedOpenAIAccountId(isManaged ? null : chatgpt.id)} aria-expanded={isManaged}>
                        {isManaged ? 'Hide account' : 'Manage Account'}
                      </button>
                      <button className="account-button remove" onClick={() => void disconnectOpenAIAccount(chatgpt.id)} disabled={!desktopAvailable || flowActive} aria-label={'Disconnect ' + (chatgpt.email ?? 'ChatGPT account')}>
                        <Trash2 size={13} />Disconnect
                      </button>
                    </div>
                  </div>
                  {isManaged && <div className="openai-account-details" aria-label="ChatGPT OAuth account details">
                    <span><small>Method</small><strong>ChatGPT OAuth</strong></span>
                    <span><small>Account</small><strong>{chatgpt.email ?? chatgpt.displayName ?? 'ChatGPT account'}</strong></span>
                    <span><small>Session</small><strong>{reconnect ? 'Needs reconnection' : 'Stored securely on this device'}</strong></span>
                  </div>}
                </div>;
              }) : <div className="openai-method-row">
                <div className="openai-method-status"><span>ChatGPT OAuth</span><strong className="account-status">Not connected</strong></div>
                <div className="openai-method-account"><span>Sign in using your ChatGPT account in a browser.</span></div>
                <div className="openai-method-actions"><button className="account-button connect" onClick={() => void connectOpenAIChatGPT()} disabled={!desktopAvailable || !chatGPTAvailable || flowActive}>Continue with ChatGPT</button></div>
              </div>}

              <div className="openai-method-row">
                <div className="openai-method-status"><span>API Key</span><strong className={`account-status ${keyConnected ? 'connected' : keyNeedsAttention ? 'needs_attention' : ''}`}>
                  {keyConnected ? <><CircleCheck size={13} />Connected</> : keyNeedsAttention ? <><CircleAlert size={13} />{attentionLabel(account.lastErrorCode)}</> : 'Not connected'}
                </strong></div>
                <div className="openai-method-account"><span>{keyConnected ? 'Key stored in your OS credential manager.' : keyNeedsAttention ? 'Check or replace the saved key to restore access.' : 'Use an OpenAI API key from your developer account.'}</span></div>
                <div className="openai-method-actions">
                  {keyConnected && <>
                    <button className="account-button quiet" onClick={() => void validate(provider)} disabled={!desktopAvailable || busy}>{busy ? <LoaderCircle size={13} className="spin" /> : <RotateCw size={13} />}Test</button>
                    <button className="account-button quiet" onClick={() => { setEditingId(provider.id); setSecret(''); resetFeedback(); }} disabled={!desktopAvailable || busy}><KeyRound size={13} />Change key</button>
                    <button className="account-button remove" onClick={() => void disconnect(provider)} disabled={!desktopAvailable || busy}><Trash2 size={13} />Disconnect</button>
                  </>}
                  {!keyConnected && !editing && <button className="account-button quiet" onClick={() => { setEditingId(provider.id); setSecret(''); resetFeedback(); }} disabled={!desktopAvailable || busy}><KeyRound size={13} />{keyNeedsAttention ? 'Replace key' : 'Use API Key'}</button>}
                </div>
              </div>
              {(chatgptConnected || keyConnected) && <div className="openai-catalog-actions">
                <span>{models.openai ? `${models.openai.length} OpenAI models loaded` : 'Load the models available to the active OpenAI credential.'}</span>
                <button className="account-button quiet" onClick={() => void loadModels(provider)} disabled={!desktopAvailable || busy}>
                  {busy ? <LoaderCircle size={13} className="spin" /> : <Waypoints size={13} />}Refresh models
                </button>
              </div>}
            </div>}

            {editing && <form className="account-connect-form openai-key-form" onSubmit={event => { event.preventDefault(); void connect(provider); }}>
              <div className="account-form-copy"><strong>{keyConnected ? 'Replace OpenAI API key' : keyNeedsAttention ? 'Update OpenAI API key' : 'Use an OpenAI API key'}</strong><span>The key is tested before it is saved to your OS credential manager.</span></div>
              <label className="sr-only" htmlFor="api-key-openai">OpenAI API key</label>
              <input id="api-key-openai" type="password" autoComplete="off" spellCheck={false} value={secret} onChange={event => setSecret(event.target.value)} placeholder="Paste an API key" disabled={busy} autoFocus />
              <div className="account-form-actions">
                <button type="submit" className="account-button connect" disabled={!desktopAvailable || busy || !secret.trim()}>{busy ? <LoaderCircle size={13} className="spin" /> : <ShieldCheck size={13} />}{busy ? 'Testing…' : 'Test and connect'}</button>
                <button type="button" className="account-button quiet" onClick={() => { setEditingId(null); setSecret(''); resetFeedback(); }} disabled={busy}>Cancel</button>
              </div>
            </form>}
          </article>;
        }

        const connected = account.state !== 'not_connected';

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

          {oauthOptions.map(option => <div className="account-method-note" key={option.method}>
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
