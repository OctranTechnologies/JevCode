import { command, desktopAvailable } from './ipc';
/** Log event identifiers only. No keys, prompts, tool output or stack traces. */
export function logEvent(event: string, level: 'info' | 'error' = 'info'): void {
  if (desktopAvailable) void command('frontend_log', { level, event }).catch(() => {});
  if (import.meta.env.DEV) console[level](JSON.stringify({ timestamp: new Date().toISOString(), level, event }));
}
