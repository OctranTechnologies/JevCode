import { desktopAvailable } from './ipc';

// Only Windows draws its own bar. macOS keeps its native frame, and Linux keeps the system frame
// because an undecorated window there may lose its resize edges.
export function usesCustomTitleBar(tauri: boolean, userAgent: string): boolean {
  return tauri && /Windows/i.test(userAgent);
}

export const customTitleBar = usesCustomTitleBar(desktopAvailable, typeof navigator === 'undefined' ? '' : navigator.userAgent);
