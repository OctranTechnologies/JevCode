import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Clock3, Eye, Radio, Search, Sparkles, Star, Wrench, BrainCircuit } from 'lucide-react';
import type { Model, ModelPreferences, ModelReference, Provider } from '../../types/domain';

type Entry = { provider: Provider; model: Model; key: string };
type Props = {
  providers: Provider[];
  preferences: ModelPreferences;
  providerId: string;
  modelId: string;
  locked?: boolean;
  compact?: boolean;
  onSelect: (reference: ModelReference) => void;
  onToggleFavorite: (reference: ModelReference) => void;
  onSetDefault: (reference: ModelReference) => void;
};

function reference(entry: Entry): ModelReference {
  return { providerId: entry.provider.id, modelId: entry.model.id };
}

function sameReference(left: ModelReference | null | undefined, right: ModelReference): boolean {
  return !!left && left.providerId === right.providerId && left.modelId === right.modelId;
}

function entryKey(providerId: string, modelId: string): string { return `${providerId}\u0000${modelId}`; }

export function ModelPicker({ providers, preferences, providerId, modelId, locked = false, compact = false, onSelect, onToggleFavorite, onSetDefault }: Props) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const [position, setPosition] = useState({ left: 0, top: 0, width: 352 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const flat = useMemo<Entry[]>(() => providers.flatMap(provider => provider.models
    .filter(model => model.status !== 'unavailable')
    .map(model => ({ provider, model, key: entryKey(provider.id, model.id) }))), [providers]);
  const selected = flat.find(entry => entry.provider.id === providerId && entry.model.id === modelId);
  const stale = !selected && !!modelId;
  const staleDefault = !!preferences.defaultModel && !flat.some(entry => sameReference(preferences.defaultModel, reference(entry)));
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const matches = (entry: Entry) => !normalizedQuery || `${entry.model.displayName} ${entry.model.id} ${entry.provider.name}`.toLocaleLowerCase().includes(normalizedQuery);
  const filtered = flat.filter(matches);
  const favoriteEntries = preferences.favorites.map(ref => flat.find(entry => sameReference(ref, reference(entry)))).filter((entry): entry is Entry => !!entry && matches(entry));
  const recentEntries = preferences.recent.map(ref => flat.find(entry => sameReference(ref, reference(entry)))).filter((entry): entry is Entry => !!entry && matches(entry));
  const visibleKeys = normalizedQuery ? filtered.map(entry => entry.key) : [
    ...favoriteEntries.map(entry => entry.key),
    ...recentEntries.map(entry => entry.key),
    ...filtered.map(entry => entry.key),
  ];
  const uniqueVisibleKeys = [...new Set(visibleKeys)];
  const activeKey = uniqueVisibleKeys[Math.min(activeIndex, Math.max(uniqueVisibleKeys.length - 1, 0))];
  const providerGroups = providers.map(provider => ({ provider, entries: filtered.filter(entry => entry.provider.id === provider.id) })).filter(group => group.entries.length > 0);

  function updatePosition() {
    const rect = triggerRef.current?.getBoundingClientRect();
    if (!rect) return;
    const width = Math.min(360, window.innerWidth - 24);
    setPosition({ left: Math.max(12, Math.min(rect.left, window.innerWidth - width - 12)), top: Math.min(rect.bottom + 6, window.innerHeight - 420), width });
  }

  useEffect(() => {
    if (!open) return;
    updatePosition();
    searchRef.current?.focus();
    function onPointerDown(event: PointerEvent) {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !panelRef.current?.contains(target)) setOpen(false);
    }
    function onWindowChange() { updatePosition(); }
    document.addEventListener('pointerdown', onPointerDown);
    window.addEventListener('resize', onWindowChange);
    window.addEventListener('scroll', onWindowChange, true);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      window.removeEventListener('resize', onWindowChange);
      window.removeEventListener('scroll', onWindowChange, true);
    };
  }, [open]);

  useEffect(() => { setActiveIndex(0); }, [query]);

  function choose(entry: Entry) {
    if (entry.model.status === 'unavailable') return;
    onSelect(reference(entry));
    setOpen(false);
    setQuery('');
  }

  function handleSearchKey(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (uniqueVisibleKeys.length) setActiveIndex(index => (index + (event.key === 'ArrowDown' ? 1 : -1) + uniqueVisibleKeys.length) % uniqueVisibleKeys.length);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      const entry = flat.find(item => item.key === activeKey);
      if (entry) choose(entry);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
    }
  }

  function renderEntry(entry: Entry, section: string) {
    const ref = reference(entry);
    const isFavorite = preferences.favorites.some(item => sameReference(item, ref));
    const isDefault = sameReference(preferences.defaultModel, ref);
    const isSelected = entry.provider.id === providerId && entry.model.id === modelId;
    const highlighted = activeKey === entry.key;
    return <div className={`model-picker-row${highlighted ? ' is-highlighted' : ''}${isSelected ? ' is-selected' : ''}`} key={`${section}:${entry.key}`} onMouseEnter={() => setActiveIndex(Math.max(0, uniqueVisibleKeys.indexOf(entry.key)))}>
      <button type="button" className="model-picker-choice" role="option" aria-selected={isSelected} onClick={() => choose(entry)}>
        <span className="model-picker-choice-copy"><strong>{entry.model.displayName}{entry.model.status === 'deprecated' && <em>Deprecated</em>}</strong><small>{entry.provider.name}{entry.provider.connected ? '' : ' · Not connected'} <span>·</span> {entry.model.id}</small></span>
        <span className="model-picker-signals" aria-label={capabilityText(entry.model)}>
          {entry.model.supportsTools && <Wrench size={12} aria-hidden="true" />}
          {entry.model.supportsVision && <Eye size={12} aria-hidden="true" />}
          {entry.model.supportsReasoning && <BrainCircuit size={12} aria-hidden="true" />}
          {entry.model.supportsStreaming && <Radio size={12} aria-hidden="true" />}
        </span>
        {isSelected && <Check size={14} className="model-picker-selected-mark" aria-label="Selected" />}
      </button>
      <button type="button" className={`model-picker-action${isFavorite ? ' is-on' : ''}`} aria-label={`${isFavorite ? 'Remove' : 'Add'} ${entry.model.displayName} ${isFavorite ? 'from' : 'to'} favorites`} title={isFavorite ? 'Remove favorite' : 'Add favorite'} onClick={() => onToggleFavorite(ref)}><Star size={13} fill={isFavorite ? 'currentColor' : 'none'} /></button>
      <button type="button" className={`model-picker-action model-default-action${isDefault ? ' is-on' : ''}`} aria-label={`Set ${entry.model.displayName} as default`} title={isDefault ? 'Workspace default' : 'Set as workspace default'} onClick={() => onSetDefault(ref)}>{isDefault ? <Check size={13} /> : <span>Default</span>}</button>
    </div>;
  }

  return <>
    <button ref={triggerRef} type="button" className={`model-picker-trigger${compact ? ' is-compact' : ''}`} aria-haspopup="dialog" aria-expanded={open} aria-label={`Model: ${selected ? `${selected.model.displayName}, ${selected.provider.name}` : stale ? `Unavailable model ${modelId}` : 'Choose a model'}`} disabled={locked} onClick={() => { if (!open) updatePosition(); setOpen(value => !value); }}>
      <Sparkles size={13} aria-hidden="true" /><span className="model-picker-trigger-label">{selected?.model.displayName ?? (stale ? `Unavailable · ${modelId}` : 'Choose model')}</span><ChevronDown size={12} aria-hidden="true" />
    </button>
    {open && createPortal(<div ref={panelRef} className="model-picker-popover" role="dialog" aria-label="Choose a model" style={{ position: 'fixed', left: position.left, top: Math.max(12, position.top), width: position.width }}>
      <div className="model-picker-search"><Search size={14} /><input ref={searchRef} value={query} onChange={event => setQuery(event.target.value)} onKeyDown={handleSearchKey} placeholder="Search models" aria-label="Search models" /><kbd>↑↓</kbd></div>
      {(stale || staleDefault) && <p className="model-picker-unavailable" role="status">{stale ? 'The selected model is no longer in the provider catalog. Choose an available model to continue.' : 'Your saved workspace default is no longer available. Set an available model as the new default.'}</p>}
      <div className="model-picker-results" role="listbox" aria-label="Available models">
        {!normalizedQuery && favoriteEntries.length > 0 && <section className="model-picker-group"><h3><Star size={12} />Favorites</h3>{favoriteEntries.map(entry => renderEntry(entry, 'favorite'))}</section>}
        {!normalizedQuery && recentEntries.length > 0 && <section className="model-picker-group"><h3><Clock3 size={12} />Recently used</h3>{recentEntries.map(entry => renderEntry(entry, 'recent'))}</section>}
        {providerGroups.map(({ provider: groupProvider, entries }) => <section className="model-picker-group" key={groupProvider.id}><h3>{groupProvider.name}</h3>{entries.map(entry => renderEntry(entry, groupProvider.id))}</section>)}
        {filtered.length === 0 && <p className="model-picker-empty">{normalizedQuery ? 'No matching models.' : 'No available models. Refresh the provider catalog in Accounts.'}</p>}
      </div>
      <footer className="model-picker-footer"><span><Wrench size={11} />Tools</span><span><Eye size={11} />Vision</span><span><BrainCircuit size={11} />Reasoning</span><span><Radio size={11} />Streaming</span></footer>
    </div>, document.body)}
  </>;
}

function capabilityText(model: Model): string {
  return [model.supportsTools && 'Tools', model.supportsVision && 'Vision', model.supportsReasoning && 'Reasoning', model.supportsStreaming && 'Streaming'].filter(Boolean).join(', ');
}
