import { describe, expect, it } from 'vitest';
import type { OpenAIAccount, OpenAIAuthAttemptEvent } from '../src/types/domain';
import {
  initialOpenAIAuthFlow,
  openAIAccountNeedsReconnect,
  openAIAccountStatus,
  openAIAuthEventFeedback,
  openAIAuthFailureMessage,
  openAIAuthFlowReducer,
  openAIAuthStartErrorMessage,
} from '../src/features/providers/openaiAuthUi';

const attemptId = 'd8f8a8ef-4df0-4c58-b20d-61cda4482f60';

function event(state: OpenAIAuthAttemptEvent['state'], failureReason: OpenAIAuthAttemptEvent['failureReason'] = null): OpenAIAuthAttemptEvent {
  return { attemptId, state, failureReason };
}

function account(overrides: Partial<OpenAIAccount> = {}): OpenAIAccount {
  return {
    id: 'account-id',
    connected: true,
    accessTokenValid: false,
    requiresReconnect: false,
    displayName: 'Example',
    email: 'user@example.com',
    organization: null,
    clientId: 'client-id',
    grantedScopes: ['openid'],
    connectedAt: '2026-10-14T00:00:00Z',
    expiresAt: null,
    ...overrides,
  };
}

describe('OpenAI auth settings states', () => {
  it('shows the browser-opened waiting state and keeps cancellation available', () => {
    const starting = openAIAuthFlowReducer(initialOpenAIAuthFlow, { type: 'begin' });
    expect(starting.phase).toBe('starting');
    const waiting = openAIAuthFlowReducer(starting, { type: 'started', attemptId });
    expect(waiting).toMatchObject({ phase: 'waiting', attemptId });
    expect(waiting.feedback?.message).toContain('Your browser was opened');
    expect(openAIAuthFlowReducer(waiting, { type: 'begin-cancel' }).phase).toBe('cancelling');
  });

  it.each([
    ['callback_received', 'success', 'connected'],
    ['cancelled', 'info', 'cancelled'],
    ['timed_out', 'error', 'timed out'],
    ['denied', 'error', 'denied'],
    ['failed', 'error', 'verify'],
  ] as const)('maps %s to a friendly UI result', (state, tone, phrase) => {
    const feedback = openAIAuthEventFeedback(event(state));
    expect(feedback.tone).toBe(tone);
    expect(feedback.message.toLowerCase()).toContain(phrase);
  });

  it('distinguishes browser, permission, network, outage, and OAuth failures', () => {
    expect(openAIAuthFailureMessage('browser_unavailable')).toContain('could not open your browser');
    expect(openAIAuthFailureMessage('permission_denied')).toContain('was denied');
    expect(openAIAuthEventFeedback(event('failed', 'network_error')).message).toContain('could not reach OpenAI');
    expect(openAIAuthEventFeedback(event('failed', 'provider_outage')).message).toContain('temporarily unavailable');
    expect(openAIAuthEventFeedback(event('failed', 'oauth_failure')).message).toContain('could not verify');
    expect(openAIAuthStartErrorMessage('system_browser_unavailable')).toContain('could not open your browser');
    expect(openAIAuthStartErrorMessage('auth_callback_unavailable')).toContain('local sign-in listener');
    expect(openAIAuthStartErrorMessage('network_error')).toContain('Check your connection');
  });

  it('finishes only the active attempt and returns to idle after a result', () => {
    const waiting = openAIAuthFlowReducer(initialOpenAIAuthFlow, { type: 'begin' });
    const active = openAIAuthFlowReducer(waiting, { type: 'started', attemptId });
    expect(openAIAuthFlowReducer(active, { type: 'attempt-finished', event: event('cancelled', null) })).toMatchObject({
      phase: 'idle',
      attemptId: null,
      feedback: { tone: 'info', message: 'OpenAI sign-in cancelled.' },
    });
    expect(openAIAuthFlowReducer(active, { type: 'attempt-finished', event: { ...event('callback_received'), attemptId: 'another-attempt' } })).toBe(active);
  });

  it('treats an expired access token as connected when refresh remains available', () => {
    expect(openAIAccountNeedsReconnect(account({ accessTokenValid: false }))).toBe(false);
    expect(openAIAccountStatus(account({ accessTokenValid: false }))).toEqual({
      label: 'Connected',
      tone: 'connected',
      detail: 'Token refreshes automatically before an OpenAI request.',
    });
    expect(openAIAccountNeedsReconnect(account({ requiresReconnect: true }))).toBe(true);
    expect(openAIAccountNeedsReconnect(account({ connected: false }))).toBe(true);
    expect(openAIAccountStatus(account({ requiresReconnect: true }))).toMatchObject({ label: 'Reconnect needed', tone: 'needs_attention' });
  });
});
