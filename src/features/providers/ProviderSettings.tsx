import { useEffect, useReducer, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Check, CircleAlert, CircleCheck, CircleX, Copy, ExternalLink, KeyRound, LoaderCircle, RotateCw, ShieldCheck, Trash2, Waypoints } from 'lucide-react';
import type { GoogleAuthAttemptEvent, GoogleAuthStatus, Model, OpenAIAuthAttemptEvent, OpenAIAuthStatus, Provider, ProviderAccount, ProviderAuthMethod } from '../../types/domain';
import { cancelGoogleAuth, cancelOpenAIAuth, copyOpenAIAuthLink, desktopAvailable, disconnectGoogleApiKey, getGoogleAuthStatus, getOpenAIAuthStatus, logoutGoogleAuth, logoutOpenAIAuth, openAIAuthAttemptEventSchema, providerAuthAdapter, selectGoogleAccount, selectGoogleAuthMethod, selectOpenAIAPIKey, selectOpenAIAccount, setGoogleCloudProject, startGoogleAuth, startOpenAIAuth, subscribeGoogleAuthAttempts } from '../../lib/ipc';
import { normalizeError } from '../../lib/errors';
import { initialOpenAIAuthFlow, openAIAccountNeedsReconnect, openAIAccountStatus, openAIAuthFlowReducer, openAIAuthStartErrorMessage } from './openaiAuthUi';
import { googleAccountNeedsReconnect, googleAuthFlowReducer, googleAuthStartErrorMessage, googleAuthStatusSummary, googleProviderErrorMessage, initialGoogleAuthFlow } from './googleAuthUi';

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
  const [copyingAuthLink, setCopyingAuthLink] = useState(false);
  const [authLinkCopied, setAuthLinkCopied] = useState(false);
  const [googleStatus, setGoogleStatus] = useState<GoogleAuthStatus | null>(null);
  const [googleStatusLoading, setGoogleStatusLoading] = useState(desktopAvailable);
  const [googleStatusError, setGoogleStatusError] = useState<string | null>(null);
  const [googleFlow, dispatchGoogleFlow] = useReducer(googleAuthFlowReducer, initialGoogleAuthFlow);
  const [googleAccountExpanded, setGoogleAccountExpanded] = useState(false);
  const [googleProjectId, setGoogleProjectId] = useState('');
  const googleAttemptIdRef = useRef<string | null>(null);
  const googleStartPendingRef = useRef(false);
  const pendingGoogleEventsRef = useRef(new Map<string, GoogleAuthAttemptEvent>());
  const googleCallbacksRef = useRef({ onAccount, onModels });
  useEffect(() => { googleCallbacksRef.current = { onAccount, onModels }; }, [onAccount, onModels]);
  useEffect(() => { setGoogleProjectId(googleStatus?.account?.cloudProjectId ?? ''); }, [googleStatus?.account?.cloudProjectId]);

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
        void (async () => {
          try {
            const status = await getOpenAIAuthStatus();
            if (!active) return;
            setOpenAIStatus(status);
            setOpenAIStatusError(null);
            if (status.chatgpt.selectedAuthMethod !== 'openai_chatgpt') {
              setNotice('ChatGPT account connected. Select it below to use its models.');
              return;
            }
            const available = await providerAuthAdapter('openai').getAvailableModels();
            if (!active) return;
            setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
            onModels('openai', available);
            setNotice(`${available.length} models loaded from the selected ChatGPT account.`);
            setError(null);
          } catch (cause) {
            if (active) setError(`ChatGPT sign-in succeeded, but OpenAI models could not be loaded. ${normalizeError(cause).message}`);
          }
        })();
      }
    }).then(stop => { if (active) unlisten = stop; else stop(); });
    return () => { active = false; unlisten?.(); };
  }, [onModels]);

  async function refreshGoogleStatus() {
    setGoogleStatusLoading(true);
    setGoogleStatusError(null);
    try { setGoogleStatus(await getGoogleAuthStatus()); }
    catch { setGoogleStatusError('Google Gemini account status could not be loaded. Check the app and retry.'); }
    finally { setGoogleStatusLoading(false); }
  }

  async function saveGoogleProjectId() {
    const projectId = googleProjectId.trim();
    if (!projectId) return;
    setBusyId('gemini');
    resetFeedback();
    try {
      const accountId = googleStatus?.selectedAccountId;
      if (!accountId) throw new Error('Select a Google account first.');
      await setGoogleCloudProject(projectId, accountId);
      const status = await getGoogleAuthStatus();
      setGoogleStatus(status);
      setGoogleProjectId(status.account?.cloudProjectId ?? projectId);
      if (status.selectedAuthType === 'google_oauth') {
        const available = await providerAuthAdapter('gemini').getAvailableModels();
        setModels(current => ({ ...current, gemini: available.map(model => model.displayName) }));
        onModels('gemini', available);
        setNotice(`Google Cloud project saved. ${available.length} Gemini API models loaded using this project's quota and billing.`);
      } else {
        setNotice('Google Cloud project saved with this account. Select “Use Google Account + Gemini API” to load models through the project. Requests use that project’s quota and billing.');
      }
    } catch (cause) {
      const normalized = normalizeError(cause);
      setError(googleProviderErrorMessage(normalized.code, normalized.message));
    } finally { setBusyId(null); }
  }

  useEffect(() => {
    if (!desktopAvailable) return;
    let active = true;
    void getGoogleAuthStatus().then(status => {
      if (active) { setGoogleStatus(status); setGoogleProjectId(status.account?.cloudProjectId ?? ''); setGoogleStatusError(null); }
    }).catch(() => {
      if (active) setGoogleStatusError('Google Gemini account status could not be loaded. Check the app and retry.');
    }).finally(() => { if (active) setGoogleStatusLoading(false); });
    let unlisten: (() => void) | undefined;
    void subscribeGoogleAuthAttempts(attempt => {
      if (!active) return;
      const event = attempt as GoogleAuthAttemptEvent;
      if (googleStartPendingRef.current) {
        pendingGoogleEventsRef.current.set(event.attemptId, event);
      } else {
        if (googleAttemptIdRef.current !== event.attemptId) return;
        googleAttemptIdRef.current = null;
        dispatchGoogleFlow({ type: 'attempt-finished', event });
      }
      if (event.state !== 'connected') return;
      void (async () => {
        try {
          const status = await getGoogleAuthStatus();
          if (!active) return;
          setGoogleStatus(status);
          setGoogleProjectId(status.account?.cloudProjectId ?? '');
          setGoogleStatusError(null);
          const providerAccount = await providerAuthAdapter('gemini').getAuthStatus();
          if (active) googleCallbacksRef.current.onAccount(providerAccount);
          if (status.selectedAuthType === 'google_oauth') {
            const available = await providerAuthAdapter('gemini').getAvailableModels();
            if (!active) return;
            setModels(current => ({ ...current, gemini: available.map(model => model.displayName) }));
            googleCallbacksRef.current.onModels('gemini', available);
            setNotice(`${available.length} Gemini API models loaded for the selected Google account and Cloud project.`);
            return;
          }
          if (status.selectedAuthType !== 'gemini_api_key') {
            if (active) setNotice('Google account connected. Choose a Cloud project, then select Google Account + Gemini API to use Gemini models. This uses Cloud project quota and billing, not Code Assist or AI subscription quota.');
            return;
          }
          const available = await providerAuthAdapter('gemini').getAvailableModels();
          if (!active) return;
          setModels(current => ({ ...current, gemini: available.map(model => model.displayName) }));
          googleCallbacksRef.current.onModels('gemini', available);
          setNotice(`${available.length} Gemini models loaded for the selected Gemini API key.`);
          setError(null);
        } catch (cause) {
          if (active) setError(`Google sign-in succeeded, but Gemini models could not be loaded. ${googleProviderErrorMessage(normalizeError(cause).code, normalizeError(cause).message)}`);
        }
      })();
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
      if (provider.id === 'openai') {
        const status = await getOpenAIAuthStatus();
        setOpenAIStatus(status);
        try {
          const available = await providerAuthAdapter(provider.id).getAvailableModels();
          setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
          onModels(provider.id, available);
        } catch { /* The validated key is connected; catalog refresh can be retried from Accounts. */ }
      } else {
        if (provider.id === 'gemini') setGoogleStatus(await getGoogleAuthStatus());
        try {
          const available = await providerAuthAdapter(provider.id).getAvailableModels();
          if (available.length) onModels(provider.id, available);
        } catch { /* The validated key is connected; catalog refresh can be retried from Accounts. */ }
      }
      setSecret('');
      setEditingId(null);
      setNotice(provider.id === 'openai'
        ? 'The OpenAI API key was tested and saved. OpenAI requests now use the API key because you connected it explicitly.'
        : provider.id === 'gemini'
          ? 'The Gemini API key was tested and saved in your OS credential manager. Gemini requests now use this key.'
          : `${provider.name} connection tested and saved in your OS credential manager.`);
    } catch (cause) {
      const normalized = normalizeError(cause);
      setError(provider.id === 'gemini' ? googleProviderErrorMessage(normalized.code, normalized.message) : normalized.message);
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
    setAuthLinkCopied(false);
    dispatchOpenAIFlow({ type: 'begin' });
    try {
      const started = await startOpenAIAuth(accountId);
      dispatchOpenAIFlow({ type: 'started', attemptId: started.attemptId });
    } catch (cause) {
      dispatchOpenAIFlow({ type: 'start-failed', message: openAIAuthStartErrorMessage(normalizeError(cause).code) });
    }
  }

  async function copyOpenAIChatGPTAuthLink() {
    const attemptId = openAIFlow.attemptId;
    if (!attemptId || copyingAuthLink) return;
    setCopyingAuthLink(true);
    setAuthLinkCopied(false);
    resetFeedback();
    try {
      await copyOpenAIAuthLink(attemptId);
      setAuthLinkCopied(true);
      setNotice('Sign-in link copied. Paste it into the browser you want to use, and keep JevCode open until sign-in finishes.');
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally {
      setCopyingAuthLink(false);
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
    setBusyId('openai');
    resetFeedback();
    try {
      const result = await logoutOpenAIAuth(accountId);
      const status = await getOpenAIAuthStatus();
      setOpenAIStatus(status);
      setOpenAIStatusError(null);
      onModels('openai', []);
      setModels(current => { const next = { ...current }; delete next.openai; return next; });
      if (status.chatgpt.selectedAuthMethod === 'openai_chatgpt' && status.chatgpt.connectedAccounts.length > 0) {
        try {
          const available = await providerAuthAdapter('openai').getAvailableModels();
          setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
          onModels('openai', available);
        } catch { /* The account is removed; the selected account can be reconnected. */ }
      } else if (status.chatgpt.selectedAuthMethod === 'api_key') {
        try {
          const available = await providerAuthAdapter('openai').getAvailableModels();
          setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
          onModels('openai', available);
        } catch { /* The API key route remains selected; its catalog can be refreshed later. */ }
      }
      setNotice(result.revocationConfirmed
        ? 'Account removed from this device and its ChatGPT session revoked.'
        : 'Account removed from this device. OpenAI could not confirm remote sign-out; review app access in your ChatGPT settings.');
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusyId(null); }
  }

  async function selectChatGPTAccount(accountId: string) {
    setBusyId('openai');
    resetFeedback();
    onModels('openai', []);
    setModels(current => { const next = { ...current }; delete next.openai; return next; });
    try {
      const status = await selectOpenAIAccount(accountId);
      setOpenAIStatus(status);
      const available = await providerAuthAdapter('openai').getAvailableModels();
      setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
      onModels('openai', available);
      const selected = status.chatgpt.connectedAccounts.find(item => item.id === accountId);
      setNotice(`${selected?.email ?? selected?.displayName ?? 'ChatGPT account'} is selected. ${available.length} models loaded.`);
    } catch (cause) {
      setError(normalizeError(cause).message);
    } finally { setBusyId(null); }
  }

  async function selectAPIKeyRoute() {
    setBusyId('openai');
    resetFeedback();
    onModels('openai', []);
    setModels(current => { const next = { ...current }; delete next.openai; return next; });
    try {
      const status = await selectOpenAIAPIKey();
      setOpenAIStatus(status);
      const available = await providerAuthAdapter('openai').getAvailableModels();
      setModels(current => ({ ...current, openai: available.map(model => model.displayName) }));
      onModels('openai', available);
      setNotice(`OpenAI requests now use the API key. ${available.length} models loaded.`);
    } catch (cause) { setError(normalizeError(cause).message); }
    finally { setBusyId(null); }
  }

  async function connectGoogleAccount(reconnectAccountId: string | null = null) {
    pendingGoogleEventsRef.current.clear();
    googleAttemptIdRef.current = null;
    googleStartPendingRef.current = true;
    dispatchGoogleFlow({ type: 'begin' });
    try {
      const started = await startGoogleAuth(reconnectAccountId);
      googleStartPendingRef.current = false;
      googleAttemptIdRef.current = started.attemptId;
      dispatchGoogleFlow({ type: 'started', attemptId: started.attemptId });
      const earlyEvent = pendingGoogleEventsRef.current.get(started.attemptId);
      if (earlyEvent) {
        pendingGoogleEventsRef.current.delete(started.attemptId);
        googleAttemptIdRef.current = null;
        dispatchGoogleFlow({ type: 'attempt-finished', event: earlyEvent });
      }
    } catch (cause) {
      googleStartPendingRef.current = false;
      pendingGoogleEventsRef.current.clear();
      googleAttemptIdRef.current = null;
      dispatchGoogleFlow({ type: 'start-failed', message: googleAuthStartErrorMessage(normalizeError(cause).code) });
    }
  }

  async function chooseGoogleAccount(accountId: string) {
    if (accountId === googleStatus?.selectedAccountId) return;
    setBusyId('gemini');
    resetFeedback();
    try {
      const status = await selectGoogleAccount(accountId);
      setGoogleStatus(status);
      setGoogleProjectId(status.account?.cloudProjectId ?? '');
      if (status.selectedAuthType === 'google_oauth' || status.selectedAuthType === 'gemini_api_key' && status.apiKeyConnected) {
        const available = await providerAuthAdapter('gemini').getAvailableModels();
        setModels(current => ({ ...current, gemini: available.map(model => model.displayName) }));
        onModels('gemini', available);
      } else {
        setModels(current => { const next = { ...current }; delete next.gemini; return next; });
        onModels('gemini', []);
      }
    } catch (cause) {
      const normalized = normalizeError(cause);
      setError(googleProviderErrorMessage(normalized.code, normalized.message));
    } finally { setBusyId(null); }
  }

  async function cancelGoogleAccount() {
    const attemptId = googleFlow.attemptId;
    if (!attemptId) return;
    dispatchGoogleFlow({ type: 'begin-cancel' });
    try {
      await cancelGoogleAuth(attemptId);
      dispatchGoogleFlow({ type: 'attempt-finished', event: { attemptId, state: 'cancelled', failureReason: null } });
    } catch (cause) {
      dispatchGoogleFlow({ type: 'cancel-failed', message: googleAuthStartErrorMessage(normalizeError(cause).code) });
    }
  }

  async function selectGoogleMethod(authType: 'google_oauth' | 'gemini_api_key') {
    setBusyId('gemini');
    resetFeedback();
    onModels('gemini', []);
    setModels(current => { const next = { ...current }; delete next.gemini; return next; });
    try {
      const status = await selectGoogleAuthMethod(authType);
      setGoogleStatus(status);
      const providerAccount = await providerAuthAdapter('gemini').getAuthStatus();
      onAccount(providerAccount);
      const available = await providerAuthAdapter('gemini').getAvailableModels();
      setModels(current => ({ ...current, gemini: available.map(model => model.displayName) }));
      onModels('gemini', available);
      const label = authType === 'google_oauth' ? 'Google Account + Gemini API' : 'Gemini API key';
      setNotice(`${label} selected. ${available.length} Gemini models loaded.`);
    } catch (cause) {
      const normalized = normalizeError(cause);
      setError(googleProviderErrorMessage(normalized.code, normalized.message));
    } finally { setBusyId(null); }
  }

  async function disconnectGoogleAccount(accountId: string) {
    setBusyId('gemini');
    resetFeedback();
    try {
      const result = await logoutGoogleAuth(accountId);
      const status = await getGoogleAuthStatus();
      setGoogleStatus(status);
      onAccount(await providerAuthAdapter('gemini').getAuthStatus());
      if (status.selectedAuthType === 'gemini_api_key') {
        const available = await providerAuthAdapter('gemini').getAvailableModels();
        setModels(current => ({ ...current, gemini: available.map(model => model.displayName) }));
        onModels('gemini', available);
      } else {
        setModels(current => { const next = { ...current }; delete next.gemini; return next; });
        onModels('gemini', []);
      }
      setNotice(result.revocationConfirmed
        ? 'Google account removed from this device and access revoked.'
        : 'Google account removed from this device. Google could not confirm remote sign-out; review app access in your Google Account settings.');
    } catch (cause) {
      const normalized = normalizeError(cause);
      setError(googleProviderErrorMessage(normalized.code, normalized.message));
    } finally { setBusyId(null); }
  }

  async function disconnectGeminiApiKey() {
    setBusyId('gemini');
    resetFeedback();
    try {
      const account = await disconnectGoogleApiKey();
      onAccount(account);
      const status = await getGoogleAuthStatus();
      setGoogleStatus(status);
      setModels(current => { const next = { ...current }; delete next.gemini; return next; });
      onModels('gemini', []);
      setNotice('Gemini API key removed from your OS credential manager.');
    } catch (cause) {
      const normalized = normalizeError(cause);
      setError(googleProviderErrorMessage(normalized.code, normalized.message));
    } finally { setBusyId(null); }
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
          const activeChatgptAccount = chatgptAccounts.find(account => account.id === openAIStatus?.chatgpt.selectedAccountId);
          const chatgptConnected = !!activeChatgptAccount && !openAIAccountNeedsReconnect(activeChatgptAccount);
          const chatgptNeedsReconnect = !!activeChatgptAccount && openAIAccountNeedsReconnect(activeChatgptAccount);
          const keyConnected = account.state === 'connected';
          const keyNeedsAttention = account.state === 'needs_attention';
          const selectedMethod = openAIStatus?.chatgpt.selectedAuthMethod;
          const connected = selectedMethod === 'openai_chatgpt' ? chatgptConnected : selectedMethod === 'api_key' ? keyConnected : keyConnected || chatgptConnected;
          const needsAttention = selectedMethod === 'openai_chatgpt' ? chatgptNeedsReconnect || (!activeChatgptAccount && chatgptAccounts.length === 0) : selectedMethod === 'api_key' ? keyNeedsAttention : !connected && (keyNeedsAttention || chatgptNeedsReconnect);
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
            {(chatgptAccounts.length > 0 || selectedMethod === 'openai_chatgpt') && <div className="openai-auth-routing-note" role="status">
              {selectedMethod === 'openai_chatgpt'
                ? chatgptNeedsReconnect || chatgptAccounts.length === 0
                  ? 'ChatGPT OAuth is selected and needs reconnection. Requests pause until you reconnect or explicitly choose the API key.'
                  : 'OpenAI requests use the selected ChatGPT account. JevCode will not switch to API-key billing automatically.'
                : 'OpenAI requests use the API key. Select a ChatGPT account to route requests through your ChatGPT account.'}
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
              <button className="account-button quiet" onClick={() => void copyOpenAIChatGPTAuthLink()} disabled={openAIFlow.phase === 'cancelling' || copyingAuthLink}>
                {authLinkCopied ? <Check size={13} /> : copyingAuthLink ? <LoaderCircle size={13} className="spin" /> : <Copy size={13} />}
                {copyingAuthLink ? 'Copying…' : authLinkCopied ? 'Copied' : 'Copy sign-in link'}
              </button>
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
                const isSelected = openAIStatus?.chatgpt.selectedAccountId === chatgpt.id;
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
                      <small>Registration · {chatgpt.id.slice(0, 8)}</small>
                      {accountStatus.detail && <small>{accountStatus.detail}</small>}
                    </div>
                    <div className="openai-method-actions">
                      <button className="account-button quiet" onClick={() => void selectChatGPTAccount(chatgpt.id)} disabled={!desktopAvailable || busyId === 'openai' || isSelected && selectedMethod === 'openai_chatgpt'} aria-pressed={isSelected && selectedMethod === 'openai_chatgpt'}>
                        {busyId === 'openai' && !isSelected ? <LoaderCircle size={13} className="spin" /> : isSelected && selectedMethod === 'openai_chatgpt' ? <CircleCheck size={13} /> : null}{isSelected && selectedMethod === 'openai_chatgpt' ? 'Selected' : 'Use account'}
                      </button>
                      {reconnect && <button className="account-button connect" onClick={() => void connectOpenAIChatGPT(chatgpt.id)} disabled={!desktopAvailable || flowActive}>Reconnect</button>}
                      <button className="account-button quiet" onClick={() => setManagedOpenAIAccountId(isManaged ? null : chatgpt.id)} aria-expanded={isManaged}>
                        {isManaged ? 'Hide account' : 'Manage Account'}
                      </button>
                      <button className="account-button remove" onClick={() => void disconnectOpenAIAccount(chatgpt.id)} disabled={!desktopAvailable || flowActive || busyId === 'openai'} aria-label={'Remove ' + (chatgpt.email ?? 'ChatGPT account')}>
                        <Trash2 size={13} />Remove account
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

              {chatgptAccounts.length > 0 && <div className="openai-add-account-row">
                <button className="account-button connect" onClick={() => void connectOpenAIChatGPT()} disabled={!desktopAvailable || !chatGPTAvailable || flowActive}>Add Account</button>
              </div>}

              <div className="openai-method-row">
                <div className="openai-method-status"><span>API Key</span><strong className={`account-status ${keyConnected ? 'connected' : keyNeedsAttention ? 'needs_attention' : ''}`}>
                  {keyConnected ? <><CircleCheck size={13} />Connected</> : keyNeedsAttention ? <><CircleAlert size={13} />{attentionLabel(account.lastErrorCode)}</> : 'Not connected'}
                </strong></div>
                <div className="openai-method-account"><span>{keyConnected ? 'Key stored in your OS credential manager.' : keyNeedsAttention ? 'Check or replace the saved key to restore access.' : 'Use an OpenAI API key from your developer account.'}</span></div>
                <div className="openai-method-actions">
                  {keyConnected && selectedMethod !== 'api_key' && <button className="account-button quiet" onClick={() => void selectAPIKeyRoute()} disabled={!desktopAvailable || busyId === 'openai'}>Use API key</button>}
                  {keyConnected && selectedMethod === 'api_key' && <strong className="account-status connected"><CircleCheck size={13} />Selected</strong>}
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

        if (provider.id === 'gemini') {
          const googleAccounts = googleStatus?.accounts ?? [];
          const googleAccount = googleAccounts.find(item => item.id === googleStatus?.selectedAccountId) ?? googleStatus?.account ?? null;
          const googleConnected = !!googleAccount && !googleAccountNeedsReconnect(googleAccount);
          const googleApiScopesReady = !!googleAccount
            && googleAccount.grantedScopes.includes('https://www.googleapis.com/auth/cloud-platform')
            && googleAccount.grantedScopes.includes('https://www.googleapis.com/auth/generative-language.retriever');
          const googleApiOAuthReady = googleConnected && googleApiScopesReady && !!googleAccount?.cloudProjectId;
          const keyNeedsAttention = account.authMethod === 'api_key' && account.state === 'needs_attention';
          const keyConnected = !keyNeedsAttention && (!!googleStatus?.apiKeyConnected || account.authMethod === 'api_key' && account.state === 'connected');
          const selectedMethod = googleStatus?.selectedAuthType;
          const baseGoogleSummary = googleStatus ? googleAuthStatusSummary(googleStatus) : { label: googleStatusLoading ? 'Loading' : 'Not connected', tone: 'not_connected' as const };
          const googleSummary = keyNeedsAttention && selectedMethod === 'gemini_api_key'
            ? { label: attentionLabel(account.lastErrorCode), tone: 'needs_attention' as const }
            : baseGoogleSummary;
          const selectedConnected = selectedMethod === 'google_oauth' ? googleApiOAuthReady : selectedMethod === 'gemini_api_key' && keyConnected;
          const googleShowChoices = !googleStatusLoading && !googleStatusError && googleAccounts.length === 0 && !keyConnected && !keyNeedsAttention && !editing;
          const googleFlowActive = googleFlow.phase !== 'idle';

          return <article className={`account-row google-provider-card${editing ? ' is-editing' : ''}`} key={provider.id}>
            <div className="openai-provider-heading">
              <span className="account-monogram google-provider-mark" aria-hidden="true">G</span>
              <div className="openai-provider-copy">
                <div className="account-title-line">
                  <h2>Google Gemini</h2>
                  <span className={`account-status ${googleSummary.tone}`}>
                    {googleSummary.tone === 'connected' ? <CircleCheck size={13} /> : googleSummary.tone === 'needs_attention' ? <CircleAlert size={13} /> : <CircleX size={13} />}
                    {googleSummary.label}
                  </span>
                </div>
                <p>{googleShowChoices ? 'Use a Google account with a Cloud project, or connect a Gemini API key.' : 'Google Account OAuth and API-key access use separate billing paths.'}</p>
              </div>
            </div>

            {googleStatusLoading && <div className="openai-status-loading" role="status"><LoaderCircle size={14} className="spin" />Loading Google Gemini account status…</div>}
            {googleStatusError && <div className="openai-auth-feedback error" role="alert"><CircleAlert size={14} /><span>{googleStatusError}</span><button className="account-button quiet" onClick={() => void refreshGoogleStatus()} disabled={googleStatusLoading}>Retry</button></div>}
            {!googleStatusError && googleStatus?.oauthAvailable === false && <div className="openai-auth-feedback info" role="status"><CircleAlert size={14} /><span>{googleAuthStartErrorMessage('google_oauth_not_configured')}</span></div>}
            {!googleStatusError && <div className="openai-auth-feedback info google-code-assist-note" role="status"><CircleAlert size={14} /><span>Google Account OAuth uses the documented Gemini Developer API with a Cloud project. Requests use that project's quota and billing; they do not use Gemini Code Assist or Google AI Pro/Ultra subscription quota.</span></div>}

            {googleFlow.feedback && googleFlow.phase === 'idle' && <div className={`openai-auth-feedback ${googleFlow.feedback.tone}`} role={googleFlow.feedback.tone === 'error' ? 'alert' : 'status'}>
              {googleFlow.feedback.tone === 'success' ? <CircleCheck size={14} /> : googleFlow.feedback.tone === 'error' ? <CircleAlert size={14} /> : <Check size={14} />}
              <span>{googleFlow.feedback.message}</span>
            </div>}
            {googleFlow.phase === 'starting' && <div className="openai-auth-waiting" role="status"><LoaderCircle size={15} className="spin" /><span>Opening your browser…</span></div>}
            {(googleFlow.phase === 'waiting' || googleFlow.phase === 'cancelling') && <div className="openai-auth-waiting" role="status">
              {googleFlow.phase === 'cancelling' ? <LoaderCircle size={15} className="spin" /> : <ExternalLink size={15} />}
              <div><strong>{googleFlow.phase === 'cancelling' ? 'Cancelling sign-in…' : 'Sign in with Google'}</strong><span>Your browser has been opened. Waiting for authentication…</span></div>
              <button className="account-button quiet" onClick={() => void cancelGoogleAccount()} disabled={googleFlow.phase === 'cancelling'}>{googleFlow.phase === 'cancelling' ? 'Cancelling…' : 'Cancel'}</button>
            </div>}

            {googleShowChoices && <div className="openai-connection-choices">
              <button className="openai-chatgpt-action" onClick={() => void connectGoogleAccount()} disabled={!desktopAvailable || googleStatus?.oauthAvailable === false || googleFlowActive}>
                {googleFlow.phase === 'starting' ? <LoaderCircle size={15} className="spin" /> : <ShieldCheck size={15} />}Continue with Google
              </button>
              <span className="openai-choice-divider" aria-hidden="true">or</span>
              <button className="openai-apikey-action" onClick={() => { setEditingId(provider.id); setSecret(''); resetFeedback(); }} disabled={!desktopAvailable || googleFlowActive}>
                <KeyRound size={15} />Use Gemini API Key
              </button>
            </div>}

            {!googleStatusLoading && googleStatus && !googleStatusError && <div className="openai-method-list google-method-list" aria-label="Google Gemini authentication methods">
              <div className="google-account-list" aria-label="Google accounts">
                {googleAccounts.map(item => {
                  const selected = item.id === googleStatus.selectedAccountId;
                  const connected = !googleAccountNeedsReconnect(item);
                  return <div className={`openai-method-row google-account-row${selected ? ' is-selected' : ''}`} key={item.id}>
                    <button className="google-account-select" onClick={() => void chooseGoogleAccount(item.id)} aria-pressed={selected} disabled={!desktopAvailable || busy}>
                      <span className={`google-account-radio${selected ? ' selected' : ''}`} aria-hidden="true">{selected ? '●' : '○'}</span>
                      <span className="google-account-copy"><strong>{item.email ?? item.displayName ?? 'Google account'}</strong><small>{connected ? 'Connected' : 'Needs reconnecting'}{item.accountType === 'workspace' ? ' · Workspace' : ''}{selected ? ' · Selected' : ''}</small></span>
                    </button>
                    <div className="openai-method-actions">
                      {!connected && <button className="account-button connect" onClick={() => void connectGoogleAccount(item.id)} disabled={!desktopAvailable || googleStatus.oauthAvailable === false || googleFlowActive}>Reconnect</button>}
                      {connected && <button className="account-button quiet" onClick={() => setGoogleAccountExpanded(value => !value)} aria-expanded={googleAccountExpanded && selected} disabled={!selected}>{googleAccountExpanded && selected ? 'Hide details' : 'Manage account'}</button>}
                      <button className="account-button remove" onClick={() => void disconnectGoogleAccount(item.id)} disabled={!desktopAvailable || busy}><Trash2 size={13} />Remove</button>
                    </div>
                  </div>;
                })}
                <div className="google-add-account-row"><button className="account-button quiet" onClick={() => void connectGoogleAccount()} disabled={!desktopAvailable || googleStatus.oauthAvailable === false || googleFlowActive}><ShieldCheck size={13} />Add Google Account</button></div>
              </div>
              {googleAccountExpanded && googleAccount && <div className="openai-account-details google-account-details" aria-label="Google account details">
                <span><small>Sign-in method</small><strong>Google Account</strong></span>
                <span><small>Account</small><strong>{googleAccount.email ?? googleAccount.displayName ?? 'Google account'}</strong></span>
                <span><small>Account type</small><strong>{googleAccount.accountType === 'workspace' ? 'Workspace / organization' : googleAccount.accountType === 'individual' ? 'Individual' : 'Unknown'}</strong></span>
                <span><small>Code Assist access</small><strong>{googleStatus?.codeAssist.userTier ?? 'Separate; not used for JevCode requests'}</strong></span>
                <span><small>Session</small><strong>{googleAccount.requiresReconnect ? 'Needs reconnection' : 'Stored securely on this device'}</strong></span>
              </div>}

              {googleConnected && googleAccount && <div className="google-project-setup">
                <div className="account-form-copy"><strong>Gemini API Cloud project</strong><span>Required for Google Account OAuth. Enable the Generative Language API and ensure this account can use the project. Model requests, quota, and billing use this project.</span></div>
                <label className="sr-only" htmlFor="google-gemini-api-project">Google Cloud project ID</label>
                <input id="google-gemini-api-project" type="text" autoComplete="off" spellCheck={false} value={googleProjectId} onChange={event => setGoogleProjectId(event.target.value)} placeholder="your-google-cloud-project" disabled={busy} />
                <button className="account-button quiet" onClick={() => void saveGoogleProjectId()} disabled={!desktopAvailable || busy || !googleAccount?.id || !googleProjectId.trim()}>{busy ? <LoaderCircle size={13} className="spin" /> : <Check size={13} />}Save project ID</button>
              </div>}

              <div className="openai-method-row google-oauth-method-row">
                <div className="openai-method-status"><span>Google Account · Gemini API</span><strong className={`account-status ${selectedMethod === 'google_oauth' && googleApiOAuthReady ? 'connected' : ''}`}>
                  {selectedMethod === 'google_oauth' && googleApiOAuthReady ? <><CircleCheck size={13} />Selected</> : !googleAccount ? 'Not connected' : !googleApiScopesReady ? 'Reconnect to grant API access' : !googleAccount.cloudProjectId ? 'Cloud project required' : 'Not selected'}
                </strong></div>
                <div className="openai-method-account"><span>Uses the selected Google account's Gemini API permission and Cloud project. Google AI subscription quota is not used.</span></div>
                <div className="openai-method-actions">
                  {googleAccount && !googleApiScopesReady && <button className="account-button connect" onClick={() => void connectGoogleAccount(googleAccount.id)} disabled={!desktopAvailable || googleFlowActive}>Grant Gemini API access</button>}
                  {googleAccount && googleApiScopesReady && selectedMethod !== 'google_oauth' && <button className="account-button quiet" onClick={() => void selectGoogleMethod('google_oauth')} disabled={!desktopAvailable || busy || !googleAccount.cloudProjectId}>Use Google Account</button>}
                  {selectedMethod === 'google_oauth' && googleApiOAuthReady && <strong className="account-status connected"><CircleCheck size={13} />Selected</strong>}
                </div>
              </div>

              <div className="openai-method-row">
                <div className="openai-method-status"><span>Gemini API Key</span><strong className={`account-status ${keyConnected ? 'connected' : keyNeedsAttention ? 'needs_attention' : ''}`}>
                  {keyConnected ? <><CircleCheck size={13} />Connected</> : keyNeedsAttention ? <><CircleAlert size={13} />{attentionLabel(account.lastErrorCode)}</> : 'Not connected'}
                </strong></div>
                <div className="openai-method-account"><span>{keyConnected ? 'Key stored in your OS credential manager.' : keyNeedsAttention ? 'Check or replace the saved key to restore access.' : 'Connect a key from Google AI Studio.'}</span></div>
                <div className="openai-method-actions">
                  {keyConnected && selectedMethod !== 'gemini_api_key' && <button className="account-button quiet" onClick={() => void selectGoogleMethod('gemini_api_key')} disabled={!desktopAvailable || busy}>Use API key</button>}
                  {keyConnected && selectedMethod === 'gemini_api_key' && <strong className="account-status connected"><CircleCheck size={13} />Selected</strong>}
                  {keyConnected && <button className="account-button remove" onClick={() => void disconnectGeminiApiKey()} disabled={!desktopAvailable || busy}><Trash2 size={13} />Disconnect</button>}
                  {!keyConnected && !editing && <button className="account-button quiet" onClick={() => { setEditingId(provider.id); setSecret(''); resetFeedback(); }} disabled={!desktopAvailable || busy}>{keyNeedsAttention ? 'Replace key' : 'Use Gemini API Key'}</button>}
                </div>
              </div>

              <div className="openai-method-row google-vertex-row">
                <div className="openai-method-status"><span>Vertex AI</span><strong className="account-status">Coming later</strong></div>
                <div className="openai-method-account"><span>Vertex AI / Application Default Credentials are not available in this release.</span></div>
                <div className="openai-method-actions"><button className="account-button quiet" disabled aria-label="Vertex AI authentication unavailable">Unavailable</button></div>
              </div>

              {selectedConnected && <div className="openai-catalog-actions">
                <span>{models.gemini ? `${models.gemini.length} Gemini models loaded` : selectedMethod === 'google_oauth' ? 'Load models available to this Google account and Cloud project.' : 'Load the models available to the selected Gemini API key.'}</span>
                <button className="account-button quiet" onClick={() => void loadModels(provider)} disabled={!desktopAvailable || busy}>{busy ? <LoaderCircle size={13} className="spin" /> : <Waypoints size={13} />}Refresh models</button>
              </div>}
            </div>}

            {editing && <form className="account-connect-form openai-key-form google-key-form" onSubmit={event => { event.preventDefault(); void connect(provider); }}>
              <div className="account-form-copy"><strong>{keyConnected ? 'Replace Gemini API key' : keyNeedsAttention ? 'Update Gemini API key' : 'Use a Gemini API key'}</strong><span>The key is tested before it is saved to your OS credential manager.</span></div>
              <label className="sr-only" htmlFor="api-key-gemini">Gemini API key</label>
              <input id="api-key-gemini" type="password" autoComplete="off" spellCheck={false} value={secret} onChange={event => setSecret(event.target.value)} placeholder="Paste a Gemini API key" disabled={busy} autoFocus />
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
