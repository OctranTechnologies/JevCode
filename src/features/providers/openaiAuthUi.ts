import type { OpenAIAccount, OpenAIAuthAttemptEvent, OpenAIAuthFailureReason } from '../../types/domain';

export type OpenAIAuthFlowPhase = 'idle' | 'starting' | 'waiting' | 'cancelling';
export type OpenAIAuthFeedbackTone = 'success' | 'info' | 'error';

export interface OpenAIAuthFeedback {
  tone: OpenAIAuthFeedbackTone;
  message: string;
}

export interface OpenAIAuthFlow {
  phase: OpenAIAuthFlowPhase;
  attemptId: string | null;
  feedback: OpenAIAuthFeedback | null;
}

export type OpenAIAuthFlowAction =
  | { type: 'begin' }
  | { type: 'started'; attemptId: string }
  | { type: 'begin-cancel' }
  | { type: 'cancel-failed'; message: string }
  | { type: 'start-failed'; message: string }
  | { type: 'attempt-finished'; event: OpenAIAuthAttemptEvent };

export const initialOpenAIAuthFlow: OpenAIAuthFlow = {
  phase: 'idle',
  attemptId: null,
  feedback: null,
};

export function openAIAuthFailureMessage(reason: OpenAIAuthFailureReason | null): string {
  switch (reason) {
    case 'browser_unavailable':
      return 'JevCode could not open your browser. Check your default browser and try again.';
    case 'permission_denied':
      return 'OpenAI sign-in was denied. Allow JevCode’s requested access to connect this account.';
    case 'network_error':
      return 'JevCode could not reach OpenAI. Check your connection and try sign-in again.';
    case 'provider_outage':
      return 'OpenAI sign-in is temporarily unavailable. Your account is unchanged; try again later.';
    default:
      return 'OpenAI could not verify this sign-in. Start a new attempt and try again.';
  }
}

export function openAIAuthEventFeedback(event: OpenAIAuthAttemptEvent): OpenAIAuthFeedback {
  switch (event.state) {
    case 'callback_received':
      return { tone: 'success', message: 'OpenAI account connected. Credentials stay in your OS credential manager.' };
    case 'cancelled':
      return { tone: 'info', message: 'OpenAI sign-in cancelled.' };
    case 'timed_out':
      return { tone: 'error', message: 'OpenAI sign-in timed out. Start again when you’re ready.' };
    case 'denied':
      return { tone: 'error', message: openAIAuthFailureMessage('permission_denied') };
    case 'failed':
      return { tone: 'error', message: openAIAuthFailureMessage(event.failureReason) };
  }
}

export function openAIAuthStartErrorMessage(code: string): string {
  switch (code) {
    case 'system_browser_unavailable':
      return openAIAuthFailureMessage('browser_unavailable');
    case 'auth_callback_unavailable':
      return 'JevCode could not start the local sign-in listener. Restart the app and try again.';
    case 'network_error':
      return openAIAuthFailureMessage('network_error');
    case 'provider_outage':
      return openAIAuthFailureMessage('provider_outage');
    case 'openai_reconnect_required':
      return 'This ChatGPT session needs to be reconnected. Choose Reconnect and sign in again.';
    case 'desktop_required':
      return 'Run JevCode as a desktop app to connect an OpenAI account.';
    default:
      return 'JevCode could not start OpenAI sign-in. Check your connection and try again.';
  }
}

export function openAIAuthFlowReducer(state: OpenAIAuthFlow, action: OpenAIAuthFlowAction): OpenAIAuthFlow {
  switch (action.type) {
    case 'begin':
      return { phase: 'starting', attemptId: null, feedback: null };
    case 'started':
      if (state.phase !== 'starting') return state;
      return {
        phase: 'waiting',
        attemptId: action.attemptId,
        feedback: { tone: 'info', message: 'Your browser was opened. Complete sign-in there; JevCode will verify the result.' },
      };
    case 'begin-cancel':
      if (state.phase !== 'waiting') return state;
      return { ...state, phase: 'cancelling', feedback: { tone: 'info', message: 'Cancelling OpenAI sign-in…' } };
    case 'cancel-failed':
      if (state.phase !== 'cancelling') return state;
      return { ...state, phase: 'waiting', feedback: { tone: 'error', message: action.message } };
    case 'start-failed':
      return { phase: 'idle', attemptId: null, feedback: { tone: 'error', message: action.message } };
    case 'attempt-finished':
      if (!state.attemptId || state.attemptId !== action.event.attemptId) return state;
      return { phase: 'idle', attemptId: null, feedback: openAIAuthEventFeedback(action.event) };
  }
}

export function openAIAccountNeedsReconnect(account: OpenAIAccount): boolean {
  return account.requiresReconnect || !account.connected;
}

export function openAIAccountStatus(account: OpenAIAccount): { label: 'Connected' | 'Reconnect needed'; tone: 'connected' | 'needs_attention'; detail: string | null } {
  if (openAIAccountNeedsReconnect(account)) {
    return { label: 'Reconnect needed', tone: 'needs_attention', detail: 'Reconnect this ChatGPT account to keep using it.' };
  }
  return {
    label: 'Connected',
    tone: 'connected',
    detail: account.accessTokenValid ? null : 'Token refreshes automatically before an OpenAI request.',
  };
}
