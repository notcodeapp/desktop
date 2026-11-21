import { useEffect, useMemo, useRef, useState } from 'react';
import { FileCode2, FolderOpen, Search, X } from 'lucide-react';
import { api, nativeAvailable } from '../lib/native';
import type { FileEntry } from '../lib/types';
import { SpinnerLabel, type Report } from './UI';

/// Subsequence fuzzy match with a small score: consecutive and
/// start-of-segment hits rank higher. Pure and cheap for ~5k files.
export function fuzzyScore(query: string, text: string): number {
  if (!query) return 1;
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  let score = 0;
  let ti = 0;
  let streak = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found === -1) return 0;
    streak = found === ti ? streak + 1 : 0;
    score += 1 + streak + (found === 0 || t[found - 1] === '/' ? 2 : 0);
    ti = found + 1;
  }
  // Prefer shallower paths on ties.
  return score - t.split('/').length * 0.1;
}

export function QuickOpen({ close, onOpen, report }: { close: () => void; onOpen: (path: string) => void; report: Report }) {
  const [files, setFiles] = useState<FileEntry[] | null>(null);
  const [query, setQuery] = useState('');
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    inputRef.current?.focus();
    if (!nativeAvailable) return;
    let live = true;
    void api.filesAll().then(list => { if (live) setFiles(list); }).catch(e => { if (live) report(e); });
    return () => { live = false; };
  }, [report]);
  const visible = useMemo(() => {
    if (!files) return [];
    return files
      .map(file => ({ file, score: fuzzyScore(query, file.path) }))
      .filter(item => item.score > 0)
      .sort((a, b) => b.score - a.score || a.file.path.length - b.file.path.length)
      .slice(0, 50)
      .map(item => item.file);
  }, [files, query]);
  useEffect(() => { setIndex(0); }, [query]);
  useEffect(() => { listRef.current?.querySelector('.palette-item.selected')?.scrollIntoView({ block: 'nearest' }); }, [index, visible.length]);
  function open(file: FileEntry) {
    close();
    onOpen(file.path);
  }
  function onKeyDown(event: React.KeyboardEvent) {
    if (event.key === 'Escape') { event.preventDefault(); close(); return; }
    if (event.key === 'ArrowDown') { event.preventDefault(); setIndex(old => Math.min(old + 1, visible.length - 1)); return; }
    if (event.key === 'ArrowUp') { event.preventDefault(); setIndex(old => Math.max(old - 1, 0)); return; }
    if (event.key === 'Enter') { event.preventDefault(); const file = visible[index]; if (file) open(file); }
  }
  return <div className="palette-scrim" role="dialog" aria-modal="true" aria-label="Быстрый переход к файлу" onPointerDown={event => { if (event.target === event.currentTarget) close(); }}>
    <div className="palette">
      <div className="quick-open-bar"><Search /><input ref={inputRef} className="palette-input" value={query} placeholder="Имя файла: main, Agent… " aria-label="Поиск файла" spellCheck={false} autoComplete="off" onChange={event => setQuery(event.target.value)} onKeyDown={onKeyDown} /></div>
      <div className="palette-list scroll-area" ref={listRef}>
        {!nativeAvailable && <p className="palette-empty">Доступно в приложении NotCode Desktop.</p>}
        {nativeAvailable && !files && <SpinnerLabel>Читаю файлы проекта…</SpinnerLabel>}
        {files && !visible.length && <p className="palette-empty">Ничего не найдено{query ? ' — измените запрос' : ''}. Esc — закрыть.</p>}
        {visible.map((file, position) => <button key={file.path} type="button" className={`palette-item ${position === index ? 'selected' : ''}`} aria-selected={position === index} onPointerMove={() => setIndex(position)} onClick={() => open(file)}>
          {file.isDir ? <FolderOpen /> : <FileCode2 />}
          <span className="palette-label"><strong>{file.name}</strong><small>{file.path}</small></span>
          {position < 9 && <kbd>{position + 1}</kbd>}
        </button>)}
      </div>
      <button type="button" className="icon-button palette-close" title="Закрыть (Esc)" onClick={close}><X /></button>
    </div>
  </div>;
}
