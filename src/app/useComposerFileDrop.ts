import { useEffect, useRef, useState } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { desktopAvailable } from '../lib/ipc';

/**
 * Reports files dropped onto the task composer. Tauri delivers native drop events, which carry
 * file paths; HTML drop events do not. Returns whether files are currently hovering the composer.
 */
export function useComposerFileDrop(enabled: boolean, onDrop: (paths: string[]) => void) {
  const [dragging, setDragging] = useState(false);
  const latest = useRef({ enabled, onDrop });
  useEffect(() => { latest.current = { enabled, onDrop }; });

  useEffect(() => {
    if (!desktopAvailable) return;
    let stop: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (payload.type === 'leave') { setDragging(false); return; }
      // Native positions are physical pixels relative to the webview; hit-test in CSS pixels.
      const scale = window.devicePixelRatio || 1;
      const target = document.elementFromPoint(payload.position.x / scale, payload.position.y / scale);
      const overComposer = latest.current.enabled && !!target?.closest('.composer-dock');
      if (payload.type === 'drop') {
        setDragging(false);
        if (overComposer) latest.current.onDrop(payload.paths);
        return;
      }
      setDragging(overComposer);
    }).then(unlisten => {
      if (cancelled) unlisten();
      else stop = unlisten;
    });
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  return dragging;
}
