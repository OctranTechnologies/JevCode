import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { choosePlacement } from '../lib/popoverPlacement';
import { usePresence } from '../lib/presence';

const closeMs = 120;

/**
 * A disclosure menu. Its panel stays mounted while it fades out, opens on the side with room,
 * grows from the edge nearest its trigger, and closes on Escape or an outside press.
 */
export function DisclosureMenu({ className, summary, summaryTitle, children }: {
  className: string;
  summary: ReactNode;
  summaryTitle?: string;
  children: ReactNode;
}) {
  const details = useRef<HTMLDetailsElement>(null);
  const [open, setOpen] = useState(false);
  const rendered = usePresence(open, closeMs);

  useLayoutEffect(() => {
    const element = details.current;
    const anchor = element?.querySelector<HTMLElement>(':scope > summary');
    const panel = element?.querySelector<HTMLElement>(':scope > :not(summary)');
    if (!element || !anchor || !panel) return;
    // A closing panel is inert at once, so it cannot be clicked or tabbed into while it fades.
    panel.inert = !open;
    if (!open) return;
    const anchorRect = anchor.getBoundingClientRect();
    const { placement } = choosePlacement(anchorRect, panel.offsetHeight, window.innerHeight, 7);
    const panelRect = panel.getBoundingClientRect();
    const side = Math.abs(panelRect.left - anchorRect.left) < 2 ? 'left' : Math.abs(panelRect.right - anchorRect.right) < 2 ? 'right' : 'center';
    element.dataset.placement = placement;
    panel.style.transformOrigin = `${placement === 'above' ? 'bottom' : 'top'} ${side}`;
  }, [open]);

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: PointerEvent) {
      if (!details.current?.contains(event.target as Node)) setOpen(false);
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== 'Escape') return;
      setOpen(false);
      details.current?.querySelector<HTMLElement>(':scope > summary')?.focus();
    }
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  return <details ref={details} className={`disclosure-menu ${className}`} open={rendered} data-state={open ? 'open' : 'closed'}>
    <summary title={summaryTitle} onClick={event => { event.preventDefault(); setOpen(value => !value); }}>{summary}</summary>
    {children}
  </details>;
}
