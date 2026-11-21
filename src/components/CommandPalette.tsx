import { useEffect, useMemo, useRef, useState } from 'react';
import { AppWindow, Bot, Eye, FileCode2, FileOutput, FilePlus2, Files, FolderOpen, GitBranch, GitCompare, Github, Plug, Save, Search, Settings as SettingsIcon, Smartphone, TerminalSquare, X, Columns2, PanelLeftClose, XSquare } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

export interface Command {
  id: string;
  label: string;
  hint?: string;
  hotkey?: string;
  Icon: LucideIcon;
  disabled?: boolean;
  run: () => void;
}

function match(query: string, ...fields: (string | undefined)[]): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return fields.some(field => field?.toLowerCase().includes(q));
}

/// Fuzzy-free filter: matches the label, hint and hotkey so "тер" finds the
/// terminal and "ctrl+k" documents the palette itself.
export function CommandPalette({ commands, close }: { commands: Command[]; close: () => void }) {
  const [query, setQuery] = useState('');
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const visible = useMemo(() => commands.filter(command => match(query, command.label, command.hint, command.hotkey)), [commands, query]);
  useEffect(() => { setIndex(0); }, [query]);
  useEffect(() => { inputRef.current?.focus(); }, []);
  useEffect(() => { listRef.current?.querySelector('.palette-item.selected')?.scrollIntoView({ block: 'nearest' }); }, [index, visible.length]);
  function run(command: Command) {
    close();
    command.run();
  }
  function onKeyDown(event: React.KeyboardEvent) {
    if (event.key === 'Escape') { event.preventDefault(); close(); return; }
    if (event.key === 'ArrowDown') { event.preventDefault(); setIndex(old => Math.min(old + 1, visible.length - 1)); return; }
    if (event.key === 'ArrowUp') { event.preventDefault(); setIndex(old => Math.max(old - 1, 0)); return; }
    if (event.key === 'Enter') { event.preventDefault(); const command = visible[index]; if (command && !command.disabled) run(command); }
  }
  return <div className="palette-scrim" role="dialog" aria-modal="true" aria-label="Центр команд" onPointerDown={event => { if (event.target === event.currentTarget) close(); }}>
    <div className="palette">
      <input ref={inputRef} className="palette-input" value={query} placeholder="Команда: сохранить, терминал, тема…" aria-label="Поиск команды" onChange={event => setQuery(event.target.value)} onKeyDown={onKeyDown}/>
      <div className="palette-list scroll-area" ref={listRef}>
        {!visible.length && <p className="palette-empty">Ничего не найдено. Esc — закрыть.</p>}
        {visible.map((command, position) => <button key={command.id} type="button" className={`palette-item ${position === index ? 'selected' : ''}`} disabled={command.disabled} aria-selected={position === index} onPointerMove={() => setIndex(position)} onClick={() => run(command)}>
          <command.Icon/>
          <span className="palette-label"><strong>{command.label}</strong>{command.hint && <small>{command.hint}</small>}</span>
          {command.hotkey && <kbd>{command.hotkey}</kbd>}
        </button>)}
      </div>
      <button type="button" className="icon-button palette-close" title="Закрыть (Esc)" onClick={close}><X/></button>
    </div>
  </div>;
}

/// Builds the standard palette for the main window. Actions are passed in from
/// App so the palette stays a dumb, testable view.
export function buildCommands(actions: {
  openProject: () => void;
  save: () => void;
  closeTab: () => void;
  hasActiveTab: boolean;
  hasProject: boolean;
  toggleSidebar: () => void;
  toggleTerminal: () => void;
  toggleAgent: () => void;
  toggleSplit: () => void;
  navigate: (target: 'files'|'search'|'github'|'browser'|'git'|'mcp'|'devices') => void;
  openSettings: () => void;
  openQuickOpen: () => void;
  openFilePick: () => void;
  saveAsExternal: () => void;
  hasExternal: boolean;
  hasDirty: boolean;
  toggleDiff: () => void;
  zen: () => void;
  openRecentProject: (path: string) => void;
  recentProjects: { id: string; name: string; path: string }[];
  minimapOn: boolean;
  toggleMinimap: () => void;
}): Command[] {
  const { navigate } = actions;
  const recentCommands: Command[] = actions.recentProjects.slice(0, 5).map((project) => ({
    id: 'recent-' + project.id,
    label: 'Проект: ' + project.name,
    hint: project.path,
    Icon: FolderOpen,
    run: () => actions.openRecentProject(project.path),
  }));
  return [
    { id: 'quick-open', label: 'Перейти к файлу', hint: 'Быстрый поиск по всем файлам проекта', hotkey: 'Ctrl+P', Icon: FileCode2, run: actions.openQuickOpen },
    { id: 'open-file', label: 'Открыть файл…', hint: 'Любой файл с диска, вне проекта', hotkey: 'Ctrl+O', Icon: FilePlus2, run: actions.openFilePick },
    { id: 'open-project', label: 'Открыть папку…', hint: 'Папка как проект', Icon: FolderOpen, run: actions.openProject },
    { id: 'save', label: 'Сохранить файл', hint: 'Активная вкладка', hotkey: 'Ctrl+S', Icon: Save, disabled: !actions.hasActiveTab, run: actions.save },
    { id: 'save-as', label: 'Сохранить как…', hint: 'Внешний файл под новым именем', Icon: FileOutput, disabled: !actions.hasExternal, run: actions.saveAsExternal },
    { id: 'diff', label: 'Сравнить с сохранённым', hint: 'Diff несохранённых правок', Icon: GitCompare, disabled: !actions.hasDirty, run: actions.toggleDiff },
    { id: 'close-tab', label: 'Закрыть вкладку', hint: 'Активный файл', hotkey: 'Ctrl+W', Icon: XSquare, disabled: !actions.hasActiveTab, run: actions.closeTab },
    { id: 'zen', label: 'Дзен-режим', hint: 'Скрыть все панели — только код', Icon: Eye, run: actions.zen },
    { id: 'minimap', label: actions.minimapOn ? 'Скрыть мини-карту' : 'Показать мини-карту', hint: 'Мини-карта в редакторе', Icon: FileCode2, run: actions.toggleMinimap },
    ...recentCommands,
    { id: 'nav-files', label: 'Проводник', hint: 'Файлы проекта', Icon: Files, run: () => navigate('files') },
    { id: 'nav-search', label: 'Поиск по проекту', hotkey: 'Ctrl+Shift+F', Icon: Search, run: () => navigate('search') },
    { id: 'nav-github', label: 'GitHub', hint: 'Репозитории, клонирование, ветки', Icon: Github, run: () => navigate('github') },
    { id: 'nav-browser', label: 'Браузер', hint: 'Превью проекта и веб-страницы', Icon: AppWindow, run: () => navigate('browser') },
    { id: 'nav-git', label: 'Git', hint: 'Статус, коммиты, ветки', Icon: GitBranch, run: () => navigate('git') },
    { id: 'nav-mcp', label: 'MCP-серверы', hint: 'Внешние инструменты агента', Icon: Plug, run: () => navigate('mcp') },
    { id: 'nav-devices', label: 'Телефон и Desktop', hint: 'Связка с мобильным приложением', Icon: Smartphone, run: () => navigate('devices') },
    { id: 'toggle-sidebar', label: 'Показать или скрыть проводник', hotkey: 'Ctrl+B', Icon: PanelLeftClose, run: actions.toggleSidebar },
    { id: 'toggle-terminal', label: 'Показать или скрыть терминал', hotkey: 'Ctrl+`', Icon: TerminalSquare, run: actions.toggleTerminal },
    { id: 'toggle-agent', label: 'Показать или скрыть AI-панель', Icon: Bot, run: actions.toggleAgent },
    { id: 'toggle-split', label: 'Разделить редактор', Icon: Columns2, disabled: !actions.hasActiveTab, run: actions.toggleSplit },
    { id: 'settings', label: 'Настройки', hint: 'Аккаунт, модели, тема, память агента', Icon: SettingsIcon, run: actions.openSettings },
  ];
}
