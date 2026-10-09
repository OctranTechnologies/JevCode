import { useEffect, useState, type FormEvent } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { FolderOpen, LoaderCircle, Plus, X } from 'lucide-react';
import { desktopAvailable } from '../../lib/ipc';

export function CreateProjectDialog({ open: visible, busy, onClose, onCreate, onOpenExisting }: {
  open: boolean;
  busy: boolean;
  onClose: () => void;
  onCreate: (name: string, parentPath: string) => Promise<void>;
  onOpenExisting: () => void;
}) {
  const [name, setName] = useState('');
  const [parentPath, setParentPath] = useState('');
  const [pickerBusy, setPickerBusy] = useState(false);

  useEffect(() => {
    if (visible) { setName(''); setParentPath(''); }
  }, [visible]);

  async function chooseParent() {
    if (!desktopAvailable || pickerBusy) return;
    setPickerBusy(true);
    try {
      const selected = await open({ directory: true, multiple: false, title: 'Choose a parent folder for the project' });
      if (typeof selected === 'string') setParentPath(selected);
    } finally { setPickerBusy(false); }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!name.trim() || !parentPath || busy) return;
    await onCreate(name.trim(), parentPath);
  }

  if (!visible) return null;
  return <div className="project-dialog-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
    <section className="project-create-dialog" role="dialog" aria-modal="true" aria-labelledby="create-project-title">
      <header><div><span className="project-dialog-icon"><Plus size={15} /></span><h2 id="create-project-title">Create project</h2></div><button className="project-icon-action" onClick={onClose} aria-label="Close dialog" disabled={busy}><X size={16} /></button></header>
      <form onSubmit={event => void submit(event)}>
        <label>Project name<input autoFocus value={name} onChange={event => setName(event.target.value)} maxLength={100} placeholder="e.g. Atlas" disabled={!desktopAvailable || busy} /></label>
        <div className="project-parent-picker"><div><span>Parent folder</span><p title={parentPath}>{parentPath || 'Choose where the new folder should live'}</p></div><button type="button" className="project-secondary-action" onClick={() => void chooseParent()} disabled={!desktopAvailable || pickerBusy || busy}>{pickerBusy ? <LoaderCircle size={14} className="spin" /> : <FolderOpen size={14} />}{parentPath ? 'Change' : 'Choose'}</button></div>
        <p className="project-create-note">JevCode creates one new folder inside the selected location. It won’t overwrite an existing folder.</p>
        <footer><button type="button" className="project-text-action" onClick={onOpenExisting} disabled={!desktopAvailable}>Open an existing folder or Git repository</button><span /><button type="button" className="project-secondary-action" onClick={onClose} disabled={busy}>Cancel</button><button className="project-primary-action" type="submit" disabled={!desktopAvailable || !name.trim() || !parentPath || busy}>{busy ? <LoaderCircle size={13} className="spin" /> : <Plus size={13} />}{busy ? 'Creating' : 'Create project'}</button></footer>
      </form>
    </section>
  </div>;
}
