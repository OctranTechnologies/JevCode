import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import type { GoogleAuthAttemptEvent, GoogleAuthStatus } from '../src/types/domain';
import {
  googleAccountNeedsReconnect,
  googleAuthEventFeedback,
  googleAuthFailureMessage,
  googleAuthFlowReducer,
  googleAuthStartErrorMessage,
  googleAuthStatusSummary,
  googleAuthUnavailableMessage,
  googleBillingRouteLabel,
  googleProviderErrorMessage,
  initialGoogleAuthFlow,
} from '../src/features/providers/googleAuthUi';

const attemptId = '5b5b0e1a-8127-4e62-9fb7-f3a3dd9d09ce';

function event(state: GoogleAuthAttemptEvent['state'], failureReason: GoogleAuthAttemptEvent['failureReason'] = null): GoogleAuthAttemptEvent {
  return { attemptId, state, failureReason };
}

function status(overrides: Partial<GoogleAuthStatus> = {}): GoogleAuthStatus {
  return {
    account: null,
    accounts: [],
    selectedAccountId: null,
    codeAssist: {
      accountType: 'unknown',
      availability: 'not_connected',
      requiresCloudProject: false,
      cloudProjectId: null,
      userTier: null,
      inferenceAvailable: false,
    },
    selectedAuthType: null,
    apiKeyConnected: false,
    oauthAvailable: true,
    unavailableReason: null,
    ...overrides,
  };
}

describe('Google Gemini auth settings states', () => {
  it('shows browser-opened progress and supports cancellation', () => {
    const starting = googleAuthFlowReducer(initialGoogleAuthFlow, { type: 'begin' });
    expect(starting.phase).toBe('starting');
    const waiting = googleAuthFlowReducer(starting, { type: 'started', attemptId });
    expect(waiting).toMatchObject({ phase: 'waiting', attemptId });
    expect(waiting.feedback?.message).toContain('browser has been opened');
    const cancelling = googleAuthFlowReducer(waiting, { type: 'begin-cancel' });
    expect(cancelling.phase).toBe('cancelling');
    expect(googleAuthFlowReducer(cancelling, { type: 'cancel-failed', message: 'Retry cancel.' })).toMatchObject({ phase: 'waiting', feedback: { tone: 'error', message: 'Retry cancel.' } });
    expect(googleAuthFlowReducer(starting, { type: 'start-failed', message: 'Browser unavailable.' })).toMatchObject({ phase: 'idle', attemptId: null, feedback: { tone: 'error' } });
  });

  it.each([
    ['connected', 'success', 'connected'],
    ['cancelled', 'info', 'cancelled'],
    ['timed_out', 'error', 'timed out'],
    ['denied', 'error', 'denied'],
    ['failed', 'error', 'complete this sign-in'],
  ] as const)('maps %s to useful safe UI feedback', (state, tone, phrase) => {
    const feedback = googleAuthEventFeedback(event(state));
    expect(feedback.tone).toBe(tone);
    expect(feedback.message.toLowerCase()).toContain(phrase);
  });

  it('maps browser, callback timeout, consent, OAuth, network and outage failures', () => {
    expect(googleAuthFailureMessage('browser_unavailable')).toContain('could not open your browser');
    expect(googleAuthEventFeedback(event('denied', 'permission_denied')).message).toContain('Workspace administrator');
    expect(googleAuthEventFeedback(event('failed', 'network_error')).message).toContain('could not reach Google');
    expect(googleAuthEventFeedback(event('failed', 'provider_outage')).message).toContain('temporarily unavailable');
    expect(googleAuthEventFeedback(event('failed', 'oauth_failure')).message).toContain('could not complete this sign-in');
    expect(googleAuthEventFeedback(event('timed_out')).message).toContain('timed out');
    expect(googleAuthStartErrorMessage('auth_callback_unavailable')).toContain('local sign-in listener');
  });

  it('explains unsupported accounts, Workspace restrictions and Cloud project setup', () => {
    expect(googleAuthFailureMessage('permission_denied')).toContain('unsupported');
    expect(googleAuthFailureMessage('permission_denied')).toContain('Workspace');
    expect(googleAuthUnavailableMessage('Google Cloud project ID is required')).toContain('Cloud project');
    expect(googleAuthStartErrorMessage('google_gemini_project_required')).toContain('Cloud project');
  });

  it('finishes only the active attempt and clears its handle', () => {
    const starting = googleAuthFlowReducer(initialGoogleAuthFlow, { type: 'begin' });
    const waiting = googleAuthFlowReducer(starting, { type: 'started', attemptId });
    expect(googleAuthFlowReducer(waiting, { type: 'attempt-finished', event: event('cancelled') })).toMatchObject({
      phase: 'idle', attemptId: null, feedback: { tone: 'info', message: 'Google sign-in cancelled.' },
    });
    expect(googleAuthFlowReducer(waiting, { type: 'attempt-finished', event: { ...event('connected'), attemptId: 'other' } })).toBe(waiting);
  });

  it('distinguishes Google Account, Gemini API Key, Vertex AI and reconnect status', () => {
    expect(googleAuthStatusSummary(status({ selectedAuthType: 'google_oauth', account: { id: 'google-id', email: 'user@gmail.com', displayName: 'User', authType: 'google_oauth', accountType: 'individual', cloudProjectId: 'personal-project', connected: true, requiresReconnect: false, expiresAt: null, grantedScopes: [] } }))).toEqual({ label: 'Connected', tone: 'connected' });
    expect(googleAuthStatusSummary(status({ selectedAuthType: 'gemini_api_key', apiKeyConnected: true }))).toEqual({ label: 'Connected', tone: 'connected' });
    expect(googleAuthStatusSummary(status())).toEqual({ label: 'Not connected', tone: 'not_connected' });
    expect(googleAuthStatusSummary(status({ selectedAuthType: 'google_oauth', account: { id: 'google-id', email: 'user@gmail.com', displayName: 'User', authType: 'google_oauth', accountType: 'individual', cloudProjectId: null, connected: true, requiresReconnect: false, expiresAt: null, grantedScopes: [] } }))).toEqual({ label: 'Cloud project required', tone: 'needs_attention' });
    expect(googleAuthStatusSummary(status({ selectedAuthType: 'google_oauth' })).tone).toBe('needs_attention');
    expect(googleAccountNeedsReconnect({ id: 'google-id', email: null, displayName: null, authType: 'google_oauth', accountType: 'unknown', cloudProjectId: null, connected: false, requiresReconnect: true, expiresAt: null, grantedScopes: [] })).toBe(true);
    expect(googleProviderErrorMessage('google_auth_method_unavailable', 'Vertex AI authentication is not available yet.')).toContain('Vertex AI authentication is not available yet');
    expect(googleBillingRouteLabel('google_oauth')).toBe('Google Account · Cloud project billing');
    expect(googleBillingRouteLabel('api_key')).toBe('Gemini API key billing');
    expect(googleBillingRouteLabel(null)).toBeNull();
  });

  it('turns provider errors into friendly messages without forwarding raw OAuth data', () => {
    expect(googleProviderErrorMessage('google_reconnect_required', 'private detail')).toContain('needs reconnecting');
    expect(googleProviderErrorMessage('google_gemini_project_required', 'Choose a Google Cloud project for Gemini API usage.')).toContain('Cloud project');
    expect(googleProviderErrorMessage('google_gemini_api_scope_required', 'Reconnect Google and grant Gemini API access.')).toContain('Reconnect Google');
    expect(googleProviderErrorMessage('invalid_credential', 'private detail')).toContain('rejected');
    expect(googleProviderErrorMessage('network_error', 'private detail')).toContain('connection');
    expect(googleProviderErrorMessage('provider_outage', 'private detail')).toContain('temporarily unavailable');
  });

  it('uses the shared theme tokens so the provider card adapts to dark and warm-light themes', () => {
    const css = readFileSync(new URL('../src/workbench.css', import.meta.url), 'utf8');
    expect(css).toContain(":root[data-theme='light']");
    expect(css).toContain('.account-row { min-width: 0; padding: 14px; background: var(--wb-surface); border: 1px solid var(--wb-border);');
    expect(css).toContain('.google-provider-card { grid-column: 1 / -1; padding: 17px; }');
    expect(css).not.toContain('.google-provider-card { background: #');
  });
});
