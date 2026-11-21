import { useEffect, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Minus, Square, Copy, X } from 'lucide-react';
import type { Command } from './CommandPalette';

export interface MenuEntry {
  label: string;
  hint?: string;
  disabled?: boolean;
  danger?: boolean;
  run: () => void;
}

interface MenuGroup { label: string; entries: MenuEntry[] }

/// VS Code-style frameless chrome: our own titlebar hosts the app menus and the
/// window controls. Everything is HTML — the trick that makes Electron/Tauri
/// apps indistinguishable from native ones.
export function Titlebar({ groups, brand }: { groups: MenuGroup[]; brand: string }) {
  const [open, setOpen] = useState<string | null>(null);
  const [maximized, setMaximized] = useState(false);
  const barRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    function onDown(event: PointerEvent) {
      if (!barRef.current?.contains(event.target as Node)) setOpen(null);
    }
    document.addEventListener('pointerdown', onDown);
    return () => document.removeEventListener('pointerdown', onDown);
  }, []);
  useEffect(() => {
    const window = getCurrentWindow();
    let live = true;
    let stop: (() => void) | undefined;
    void window.isMaximized().then(value => { if (live) setMaximized(value); }).catch(() => {});
    void window.onResized(() => { void window.isMaximized().then(value => { if (live) setMaximized(value); }).catch(() => {}); }).then(fn => { if (live) stop = fn; else fn(); }).catch(() => {});
    return () => { live = false; stop?.(); };
  }, []);
  function toggle(groupLabel: string) { setOpen(old => old === groupLabel ? null : groupLabel); }
  function runAndClose(entry: MenuEntry) {
    if (entry.disabled) return;
    setOpen(null);
    entry.run();
  }
  return <div ref={barRef} className="titlebar">
    <div className="titlebar-left">
      <span className="titlebar-brand"><img src="/notcode.svg" alt="" />{brand}</span>
      <nav className="titlebar-menus" aria-label="Главное меню">
        {groups.map(group => <div key={group.label} className="titlebar-menu">
          <button
            type="button"
            className={`titlebar-menu-button ${open === group.label ? 'open' : ''}`}
            aria-haspopup="menu"
            aria-expanded={open === group.label}
            onPointerDown={event => { event.stopPropagation(); toggle(group.label); }}
            onPointerEnter={() => setOpen(old => old === null ? old : group.label)}
          >{group.label}</button>
          {open === group.label && <div className="titlebar-dropdown" role="menu">
            {group.entries.map((entry, index) => entry.label === '—'
              ? <div key={`sep-${index}`} className="titlebar-sep" role="separator" />
              : <button key={entry.label} type="button" role="menuitem" className={`titlebar-item ${entry.danger ? 'danger' : ''}`} disabled={entry.disabled} onClick={() => runAndClose(entry)}>
                  <span>{entry.label}</span>
                  {entry.hint && <kbd>{entry.hint}</kbd>}
                </button>)}
          </div>}
        </div>)}
      </nav>
    </div>
    <div className="titlebar-drag" data-tauri-drag-region />
    <div className="titlebar-controls">
      <button type="button" className="titlebar-control" title="Свернуть" onClick={() => void getCurrentWindow().minimize()}><Minus /></button>
      <button type="button" className="titlebar-control" title={maximized ? 'Восстановить' : 'Развернуть'} onClick={() => void getCurrentWindow().toggleMaximize()}>{maximized ? <Copy style={{ transform: 'scaleX(-1)' }} /> : <Square />}</button>
      <button type="button" className="titlebar-control close" title="Закрыть" onClick={() => getCurrentWindow().close()}><X /></button>
    </div>
  </div>;
}

/// Builds the standard menu groups from app actions (reuses palette commands
/// so labels/hints never diverge).
export function buildMenuGroups(commands: Command[], exit: () => void): MenuGroup[] {
  const byId = (id: string): Command | undefined => commands.find(command => command.id === id);
  const entry = (id: string, overrides?: Partial<MenuEntry>): MenuEntry | null => {
    const command = byId(id);
    if (!command) return null;
    return { label: command.label, hint: command.hotkey, disabled: command.disabled, run: command.run, ...overrides };
  };
  const groups: MenuGroup[] = [
    { label: 'Файл', entries: [
      entry('open-file') ?? { label: 'Открыть файл…', run: () => {} },
      entry('open-project') ?? { label: 'Открыть папку…', run: () => {} },
      { label: '—', run: () => {} },
      entry('save') ?? { label: 'Сохранить', run: () => {} },
      entry('save-as') ?? { label: 'Сохранить как…', run: () => {} },
      { label: '—', run: () => {} },
      { label: 'Выход', run: exit },
    ] },
    { label: 'Вид', entries: [
      entry('quick-open') ?? { label: 'Перейти к файлу', run: () => {} },
      entry('nav-search') ?? { label: 'Поиск по проекту', run: () => {} },
      { label: '—', run: () => {} },
      entry('toggle-sidebar') ?? { label: 'Проводник', run: () => {} },
      entry('toggle-terminal') ?? { label: 'Терминал', run: () => {} },
      entry('toggle-agent') ?? { label: 'AI-панель', run: () => {} },
      entry('zen') ?? { label: 'Дзен-режим', run: () => {} },
      { label: '—', run: () => {} },
      entry('diff') ?? { label: 'Сравнить с сохранённым', run: () => {} },
    ] },
    { label: 'Справка', entries: [
      entry('settings', { label: 'Настройки…' }) ?? { label: 'Настройки…', run: () => {} },
      entry('nav-devices', { label: 'Телефон и Desktop' }) ?? { label: 'Телефон и Desktop', run: () => {} },
    ] },
  ];
  return groups.map(group => ({ ...group, entries: group.entries.filter(entry => entry.label !== '—' || true) }));
}
