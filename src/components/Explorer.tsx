import { useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, FileCode2, Folder, RefreshCw, Search, FolderOpen, Replace } from 'lucide-react';
import { api } from '../lib/native';
import type { FileEntry, SearchHit } from '../lib/types';
import { Empty, type Confirm as UIConfirm, type Report } from './UI';
function Node({ entry, openFile, report, depth }: {entry: FileEntry; openFile: (path: string, line?: number) => void; report: Report; depth: number}) {
  const [expanded, setExpanded] = useState(false); const [entries, setEntries] = useState<FileEntry[]>([]); const [busy, setBusy] = useState(false);
  async function toggle() {
    if (!entry.isDir) { openFile(entry.path); return; }
    if (!expanded) { setBusy(true); try { setEntries(await api.tree(entry.path)); setExpanded(true); } catch (e) { report(e); } finally { setBusy(false); } }
    else setExpanded(false);
  }
  return <div><button className="tree-row" style={{paddingLeft: `calc(var(--spacing-2) + ${depth} * var(--spacing-3))`}} onClick={() => void toggle()} aria-expanded={entry.isDir ? expanded : undefined} title={entry.path} disabled={busy}>
    {entry.isDir ? <>{expanded ? <ChevronDown/> : <ChevronRight/>}<Folder className="folder-icon"/></> : <><span className="tree-spacer"/><FileCode2/></>}<span>{entry.name}</span>
  </button>{expanded && entries.map(item => <Node key={item.path} entry={item} openFile={openFile} report={report} depth={depth + 1}/>)}</div>;
}
export function Explorer({ projectId, projectName, refreshToken = 0, openFile, report, mode, openProject, confirm }: {projectId?: string; projectName?: string; refreshToken?: number; openFile: (path: string, line?: number) => void; report: Report; mode: 'files' | 'search'; openProject: () => void; confirm?: UIConfirm}) {
  const [entries, setEntries] = useState<FileEntry[]>([]); const [revision, setRevision] = useState(0);
  const [query, setQuery] = useState(''); const [hits, setHits] = useState<SearchHit[]>([]); const [busy, setBusy] = useState(false); const [searched, setSearched] = useState(false); const [replacement, setReplacement] = useState(''); const [replaceNote, setReplaceNote] = useState('');
  const seq = useRef(0); const lastProject = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (lastProject.current !== projectId) { lastProject.current = projectId; setEntries([]); }
    if (!projectId) return;
    let live = true;
    api.tree().then(value => { if (live) setEntries(value); }).catch(report);
    return () => { live = false; };
  }, [projectId, revision, refreshToken, report]);
  useEffect(() => { seq.current++; setHits([]); setSearched(false); }, [projectId]);
  async function search() { const id = ++seq.current; setBusy(true); try { const results = await api.search(query); if (id === seq.current) { setHits(results); setSearched(true); } } catch(e) { report(e); } finally { if (id === seq.current) setBusy(false); } }
  async function replaceAll() {
    if (!query.trim() || !confirm) return;
    const same = query === replacement;
    if (!await confirm('Заменить во всех файлах?', `«${query}» → «${replacement || '(пусто)'}» по всему проекту. Открытые несохранённые файлы не перезаписываются на диске — проверьте диффы после замены.`, false)) return;
    setBusy(true); setReplaceNote('');
    try {
      const result = await api.replace(query, replacement);
      setReplaceNote(same ? 'Поиск и замена совпадают — ничего не сделано.' : `Заменено ${result.replacements} вхождений в ${result.filesChanged} файлах${result.skipped ? `, пропущено ${result.skipped}` : ''}.`);
      setRevision(x => x + 1);
    } catch (e) { report(e); } finally { setBusy(false); }
  }
  return <section className="explorer panel" aria-label={mode === 'files' ? 'Проводник проекта' : 'Поиск по проекту'}><div className="panel-heading"><span>{mode === 'files' ? 'Проводник' : 'Поиск'}</span><button className="icon-button" onClick={() => setRevision(x => x + 1)} title="Обновить дерево" disabled={!projectId}><RefreshCw/></button></div>
    {!projectId ? <Empty title="Откройте проект"><p>Локальная папка с вашим кодом.</p><button onClick={openProject}><FolderOpen/>Открыть папку</button></Empty> : mode === 'files' ? <><div className="project-label"><FolderOpen/><strong>{projectName}</strong></div><div className="scroll-area tree" key={projectId ?? 'none'}>{entries.map(entry => <Node key={entry.path} entry={entry} depth={0} openFile={openFile} report={report}/>)}{entries.length === 0 && <p className="muted inset">Папка пуста или содержит только исключённые каталоги.</p>}</div></> : <><form className="search-form" onSubmit={e => { e.preventDefault(); void search(); }}><label className="sr-only" htmlFor="search">Текст для поиска</label><input id="search" value={query} onChange={e => setQuery(e.target.value)} placeholder="Найти в файлах…"/><button disabled={busy || !query.trim()} className="icon-button" title="Найти"><Search/></button></form><div className="search-form"><label className="sr-only" htmlFor="replace">Заменить на</label><input id="replace" value={replacement} onChange={e => setReplacement(e.target.value)} placeholder="Заменить на…"/><button disabled={busy || !query.trim()} className="icon-button" title="Заменить во всех файлах" onClick={() => void replaceAll()}><Replace/></button></div>{replaceNote&&<p className="hint inset" role="status">{replaceNote}</p>}<p className="hint inset">Буквальный поиск · до 500 совпадений. Исключены .git, node_modules, dist и target.</p><div className="scroll-area">{hits.map((hit,i) => <button className="search-hit" key={`${hit.path}:${hit.line}:${i}`} onClick={() => openFile(hit.path, hit.line)}><strong>{hit.path}:{hit.line}</strong><span>{hit.preview}</span></button>)}{searched && hits.length === 0 && <Empty title="Совпадений нет"/>}{busy && <p className="inset muted">Поиск…</p>}</div></>}
  </section>;
}
