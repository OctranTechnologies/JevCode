import { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Copy, Minus, Square, X } from 'lucide-react';
import { Brand } from './Brand';
import { customTitleBar } from '../lib/windowChrome';

export function WindowTitleBar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!customTitleBar) return;
    const appWindow = getCurrentWindow();
    let active = true;
    let unlisten: (() => void) | undefined;
    const sync = () => void appWindow.isMaximized().then(value => { if (active) setMaximized(value); }).catch(() => undefined);
    sync();
    void appWindow.onResized(sync).then(stop => { if (active) unlisten = stop; else stop(); }).catch(() => undefined);
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  if (!customTitleBar) return null;

  return (
    <WindowTitleBarView
      maximized={maximized}
      onMinimize={() => void getCurrentWindow().minimize().catch(() => undefined)}
      onToggleMaximize={() => void getCurrentWindow().toggleMaximize().catch(() => undefined)}
      onClose={() => void getCurrentWindow().close().catch(() => undefined)}
    />
  );
}

export function WindowTitleBarView({ maximized, onMinimize, onToggleMaximize, onClose }: { maximized: boolean; onMinimize: () => void; onToggleMaximize: () => void; onClose: () => void }) {
  const maximizeLabel = maximized ? 'Restore' : 'Maximize';
  return (
    <header className="window-titlebar" data-tauri-drag-region>
      <Brand />
      <span className="window-titlebar-title">Octran Code</span>
      <div className="window-controls">
        <button type="button" className="window-control" aria-label="Minimize" title="Minimize" onClick={onMinimize}><Minus size={14} /></button>
        <button type="button" className="window-control" aria-label={maximizeLabel} title={maximizeLabel} onClick={onToggleMaximize}>{maximized ? <Copy size={14} /> : <Square size={14} />}</button>
        <button type="button" className="window-control is-close" aria-label="Close" title="Close" onClick={onClose}><X size={14} /></button>
      </div>
    </header>
  );
}
