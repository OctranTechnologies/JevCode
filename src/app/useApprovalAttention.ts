import { useEffect } from 'react';
import { getCurrentWindow, UserAttentionType } from '@tauri-apps/api/window';
import { desktopAvailable } from '../lib/ipc';

const windowTitle = 'Octran Code';

/**
 * The agent is blocked until the user answers a request. While that is pending, the window title
 * says so, and the taskbar button flashes when Octran Code is in the background.
 */
export function useApprovalAttention(waiting: boolean) {
  useEffect(() => {
    if (!desktopAvailable || !waiting) return;
    const current = getCurrentWindow();
    void current.setTitle(`Action needed · ${windowTitle}`);
    if (!document.hasFocus()) void current.requestUserAttention(UserAttentionType.Informational);
    return () => { void current.setTitle(windowTitle); };
  }, [waiting]);
}
