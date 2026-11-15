import { useEffect, useRef, useState } from 'react';

/** Keeps a surface mounted for `exitMs` after `open` turns false, so it can animate out before it is removed. */
export function usePresence(open: boolean, exitMs: number): boolean {
  const [exiting, setExiting] = useState(false);
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open) {
      wasOpen.current = true;
      setExiting(false);
      return;
    }
    if (!wasOpen.current) return;
    wasOpen.current = false;
    setExiting(true);
    const timer = window.setTimeout(() => setExiting(false), exitMs);
    return () => window.clearTimeout(timer);
  }, [open, exitMs]);
  return open || exiting;
}
