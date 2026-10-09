import { useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { CornerDownLeft, Search } from 'lucide-react';

export interface PaletteAction {
  id: string;
  label: string;
  detail?: string;
  group: string;
  icon: ReactNode;
  shortcut?: string;
  run: () => void;
}

export function CommandPalette({ open, onClose, actions }: { open: boolean; onClose: () => void; actions: PaletteAction[] }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const filtered = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    return actions.filter(action => !normalized || `${action.label} ${action.detail ?? ''} ${action.group}`.toLocaleLowerCase().includes(normalized));
  }, [actions, query]);

  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    if (open && !element.open) {
      element.showModal();
      setQuery('');
      setActiveIndex(0);
      requestAnimationFrame(() => input.current?.focus());
    } else if (!open && element.open) element.close();
  }, [open]);

  function select(action: PaletteAction | undefined) {
    if (!action) return;
    onClose();
    action.run();
  }

  return <dialog className="command-palette" ref={dialog} aria-label="Search tasks and commands" onClose={onClose} onClick={event => { if (event.target === dialog.current) onClose(); }}>
    <div className="palette-search-row"><Search size={18} /><input ref={input} value={query} onChange={event => { setQuery(event.target.value); setActiveIndex(0); }} onKeyDown={event => {
      if (event.key === 'ArrowDown') { event.preventDefault(); setActiveIndex(index => (index + 1) % Math.max(filtered.length, 1)); }
      else if (event.key === 'ArrowUp') { event.preventDefault(); setActiveIndex(index => (index - 1 + Math.max(filtered.length, 1)) % Math.max(filtered.length, 1)); }
      else if (event.key === 'Enter') { event.preventDefault(); select(filtered[activeIndex]); }
      else if (event.key === 'Escape') { event.preventDefault(); onClose(); }
    }} placeholder="Search projects, tasks, and actions…" aria-label="Search projects, tasks, and actions" /><kbd>ESC</kbd></div>
    <div className="palette-results" role="listbox" aria-label="Search results">
      {filtered.length ? filtered.map((action, index) => <button key={action.id} className={`palette-action${activeIndex === index ? ' is-active' : ''}`} role="option" aria-selected={activeIndex === index} onMouseEnter={() => setActiveIndex(index)} onClick={() => select(action)}>
        <span className="palette-action-icon">{action.icon}</span><span className="palette-action-copy"><strong>{action.label}</strong>{action.detail && <small>{action.detail}</small>}</span><span className="palette-action-meta">{action.shortcut && <kbd>{action.shortcut}</kbd>}{activeIndex === index && <CornerDownLeft size={14} />}</span>
      </button>) : <p className="palette-no-results">No matching project, task, or action.</p>}
    </div>
    <div className="palette-footer"><span><kbd>↑</kbd><kbd>↓</kbd> to navigate</span><span><kbd>Enter</kbd> to open</span></div>
  </dialog>;
}
