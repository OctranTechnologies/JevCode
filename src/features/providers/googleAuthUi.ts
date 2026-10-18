import type { GoogleAccount, GoogleAuthAttemptEvent, GoogleAuthFailureReason, GoogleAuthStatus } from '../../types/domain';

export type GoogleAuthFlowPhase = 'idle' | 'starting' | 'waiting' | 'cancelling';
export type GoogleAuthFeedbackTone = 'success' | 'info' | 'error';

export interface GoogleAuthFeedback {
  tone: GoogleAuthFeedbackTone;
  message: string;
}

export interface GoogleAuthFlow {
  phase: GoogleAuthFlowPhase;
  attemptId: string | null;
  feedback: GoogleAuthFeedback | null;
}

export type GoogleAuthFlowAction =
  | { type: 'begin' }
  | { type: 'started'; attemptId: string }
  | { type: 'begin-cancel' }
  | { type: 'cancel-failed'; message: string }
  | { type: 'start-failed'; message: string }
  | { type: 'attempt-finished'; event: GoogleAuthAttemptEvent };

export const initialGoogleAuthFlow: GoogleAuthFlow = {
  phase: 'idle',
  attemptId: null,
  feedback: null,
};

export function googleAuthFailureMessage(reason: GoogleAuthFailureReason | null): string {
  switch (reason) {
    case 'browser_unavailable':
      return 'JevCode could not open your browser. Check your default browser and try again.';
    case 'permission_denied':
      return 'Google sign-in was denied. This account may be unsupported or restricted by your Workspace administrator; try another account or ask your administrator.';
    case 'network_error':
      return 'JevCode could not reach Google. Check your connection and try sign-in again.';
    case 'provider_outage':
      return 'Google sign-in is temporarily unavailable. Your account is unchanged; try again later.';
    default:
      return 'Google could not complete this sign-in. Start a new attempt and try again.';
  }
}

export function googleAuthEventFeedback(event: GoogleAuthAttemptEvent): GoogleAuthFeedback {
  switch (event.state) {
    case 'connected':
      return { tone: 'success', message: 'Google account connected. Credentials stay in your OS credential manager.' };
    case 'cancelled':
      return { tone: 'info', message: 'Google sign-in cancelled.' };
    case 'timed_out':
      return { tone: 'error', message: 'Google sign-in timed out. Start again when you’re ready.' };
    case 'denied':
      return { tone: 'error', message: googleAuthFailureMessage('permission_denied') };
    case 'failed':
      return { tone: 'error', message: googleAuthFailureMessage(event.failureReason) };
  }
}

export function googleAuthStartErrorMessage(code: string): string {
  switch (code) {
    case 'system_browser_unavailable':
      return googleAuthFailureMessage('browser_unavailable');
    case 'auth_callback_unavailable':
      return 'JevCode could not start the local sign-in listener. Restart the app and try again.';
    case 'google_oauth_not_configured':
      return 'Google account sign-in is not configured. Register a Desktop OAuth client ID for JevCode, or use a Gemini API key for model requests.';
    case 'google_cloud_project_required':
      return 'This Workspace account needs a Google Cloud project ID for Code Assist setup. JevCode cannot use Code Assist models through a supported third-party API yet.';
    case 'google_reconnect_required':
      return 'This Google account needs reconnecting. Start sign-in again to restore access.';
    case 'network_error':
      return googleAuthFailureMessage('network_error');
    case 'provider_outage':
      return googleAuthFailureMessage('provider_outage');
    case 'desktop_required':
      return 'Run JevCode as a desktop app to connect a Google account.';
    default:
      return 'JevCode could not start Google sign-in. Check your connection and try again.';
  }
}

export function googleAuthFlowReducer(state: GoogleAuthFlow, action: GoogleAuthFlowAction): GoogleAuthFlow {
  switch (action.type) {
    case 'begin':
      return { phase: 'starting', attemptId: null, feedback: null };
    case 'started':
      if (state.phase !== 'starting') return state;
      return {
        phase: 'waiting',
        attemptId: action.attemptId,
        feedback: { tone: 'info', message: 'Your browser has been opened. Complete sign-in there; JevCode is waiting for authentication.' },
      };
    case 'begin-cancel':
      if (state.phase !== 'waiting') return state;
      return { ...state, phase: 'cancelling', feedback: { tone: 'info', message: 'Cancelling Google sign-in…' } };
    case 'cancel-failed':
      if (state.phase !== 'cancelling') return state;
      return { ...state, phase: 'waiting', feedback: { tone: 'error', message: action.message } };
    case 'start-failed':
      return { phase: 'idle', attemptId: null, feedback: { tone: 'error', message: action.message } };
    case 'attempt-finished':
      if (!state.attemptId || state.attemptId !== action.event.attemptId) return state;
      return { phase: 'idle', attemptId: null, feedback: googleAuthEventFeedback(action.event) };
  }
}

export function googleAccountNeedsReconnect(account: GoogleAccount): boolean {
  return account.requiresReconnect || !account.connected;
}

export function googleAuthStatusSummary(status: GoogleAuthStatus): { label: string; tone: 'connected' | 'needs_attention' | 'not_connected' } {
  if (status.selectedAuthType === 'google_oauth') {
    if (status.account && !googleAccountNeedsReconnect(status.account)) return { label: 'Account linked', tone: 'needs_attention' };
    return { label: 'Reconnect needed', tone: 'needs_attention' };
  }
  if (status.selectedAuthType === 'gemini_api_key') return { label: status.apiKeyConnected ? 'Connected' : 'Reconnect needed', tone: status.apiKeyConnected ? 'connected' : 'needs_attention' };
  if (status.account?.requiresReconnect) return { label: 'Reconnect needed', tone: 'needs_attention' };
  if (status.account?.connected) return { label: 'Account linked', tone: 'needs_attention' };
  return { label: 'Not connected', tone: 'not_connected' };
}

export function googleAuthUnavailableMessage(reason: string | null): string {
  if (!reason) return 'Google sign-in is unavailable. Use a Gemini API key or try again later.';
  return reason;
}

export function googleProviderErrorMessage(code: string, message: string): string {
  switch (code) {
    case 'google_reconnect_required':
    case 'expired_authentication':
      return 'Google account needs reconnecting. Sign in again to restore Gemini access.';
    case 'google_cloud_project_required':
    case 'google_oauth_not_configured':
    case 'google_auth_method_unavailable':
      return googleAuthUnavailableMessage(message);
    case 'google_code_assist_unsupported':
      return 'Google sign-in identifies your account, but Google has not documented a supported third-party Code Assist inference API. Use a Gemini API key for model requests.';
    case 'google_invalid_cloud_project':
      return 'Enter a valid Google Cloud project ID.';
    case 'invalid_credential':
      return 'Google rejected this credential. Check the account or API key and try again.';
    case 'quota_exhausted':
      return 'The Gemini account has reached its usage limit.';
    case 'network_error':
      return 'JevCode could not reach Google. Check your connection and try again.';
    case 'provider_outage':
      return 'Google is temporarily unavailable. Try again later.';
    case 'permission_denied':
      return googleAuthFailureMessage('permission_denied');
    default:
      return message;
  }
}
