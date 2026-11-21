import { useCallback, useEffect, useState } from 'react';
import { Copy, FolderGit2, Globe, Lock, Plus, RefreshCw, GitBranch, Check, ExternalLink } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api, nativeAvailable, type AuthProfile } from '../lib/native';
import type { GithubRepo, Project } from '../lib/types';
import { Empty, Modal, SpinnerLabel, type Confirm, type Report } from './UI';

export function RepoPicker({ close, onCloned, report, confirm }: { close: () => void; onCloned: (project: Project) => void; report: Report; confirm: Confirm }) {
  const [repos, setRepos] = useState<GithubRepo[] | null>(null);
  const [error, setError] = useState('');
  const [filter, setFilter] = useState('');
  const [busy, setBusy] = useState('');
  const load = useCallback(() => { setRepos(null); setError(''); void api.githubRepos().then(setRepos).catch(e => setError(e instanceof Error ? e.message : String(e))); }, []);
  useEffect(() => { load(); }, [load]);
  async function clone(repo: GithubRepo) {
    if (busy) return;
    if (!await confirm('Клонировать репозиторий?', `${repo.fullName}${repo.private ? ' · приватный' : ''}\n\nNotCode предложит выбрать папку на диске и склонирует репозиторий в неё. Приватные репозитории клонируются через прокси NotCode с вашей GitHub-сессией; токен не сохраняется в настройках репозитория.`)) return;
    setBusy(repo.fullName);
    try { const project = await api.gitClone(repo.htmlUrl, true); onCloned(project); } catch (e) { report(e); } finally { setBusy(''); }
  }
  const visible = (repos ?? []).filter(repo => repo.fullName.toLowerCase().includes(filter.trim().toLowerCase()));
  return <Modal title="Репозиторий GitHub как проект" close={close} wide><div className="form-stack">
    <div className="toolbar"><input value={filter} onChange={e => setFilter(e.target.value)} placeholder="Поиск по названию…" aria-label="Поиск репозитория" /><button type="button" className="icon-button" title="Обновить список" onClick={load}><RefreshCw /></button></div>
    {error && <p className="warning" role="alert">{error}</p>}
    {!repos && !error && <SpinnerLabel>Загружаю репозитории…</SpinnerLabel>}
    {repos && !visible.length && <Empty title="Репозитории не найдены"><p>{filter ? 'Измените запрос.' : 'У вашей сессии GitHub нет доступных репозиториев.'}</p></Empty>}
    <div className="repo-list scroll-area">{visible.map(repo => <div key={repo.fullName} className="repo-row"><div className="repo-meta">{repo.private ? <Lock /> : <Globe />}<div><strong>{repo.fullName}</strong><small>{repo.private ? 'Приватный' : 'Публичный'} · {repo.defaultBranch} · обновлён {new Date(repo.updatedAt).toLocaleDateString('ru-RU')}</small></div></div><button type="button" className="primary" disabled={!!busy} onClick={() => void clone(repo)}>{busy === repo.fullName ? 'Клонирую…' : 'Клонировать'}</button></div>)}</div>
    <p className="hint">После клонирования папка откроется как проект NotCode. Существующие локальные папки открываются кнопкой «Открыть проект» в шапке.</p>
  </div></Modal>;
}

function CopyButton({ value, title }: { value: string; title: string }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try { await navigator.clipboard.writeText(value); setCopied(true); setTimeout(() => setCopied(false), 1500); } catch { /* clipboard unavailable */ }
  }
  return <button type="button" className="icon-button" title={copied ? 'Скопировано' : title} onClick={() => void copy()}>{copied ? <Check /> : <Copy />}</button>;
}

/// Left-sidebar GitHub view: your repositories with quick clone, copy URL and
/// branches of the currently open project.
export function GitHubPanel({ projectId, profile, onCloned, confirm, report, openSettings }: { projectId?: string; profile: AuthProfile | null; onCloned: (project: Project) => void; confirm: Confirm; report: Report; openSettings: () => void }) {
  const [repos, setRepos] = useState<GithubRepo[] | null>(null);
  const [picker, setPicker] = useState(false);
  const [busy, setBusy] = useState('');
  const [branches, setBranches] = useState<string[] | null>(null);
  const load = useCallback(() => {
    if (!nativeAvailable) return;
    setRepos(null);
    void api.githubRepos().then(setRepos).catch(e => report(e instanceof Error ? e : new Error(String(e))));
  }, [report]);
  useEffect(() => { load(); }, [load]);
  useEffect(() => {
    if (!projectId || !nativeAvailable) { setBranches(null); return; }
    let live = true;
    void api.gitBranches().then(list => { if (live) setBranches(list); }).catch(() => { if (live) setBranches([]); });
    return () => { live = false; };
  }, [projectId]);
  async function clone(repo: GithubRepo) {
    if (busy) return;
    if (!await confirm('Клонировать репозиторий?', `${repo.fullName}${repo.private ? ' · приватный' : ''}\n\nРепозиторий будет склонирован в выбранную папку и открыт как проект.`)) return;
    setBusy(repo.fullName);
    try { const project = await api.gitClone(repo.htmlUrl, true); onCloned(project); } catch (e) { report(e); } finally { setBusy(''); }
  }
  return <div className="github-panel panel">
    <div className="panel-heading"><span>GitHub</span><div className="toolbar"><button className="icon-button" title="Обновить" onClick={load}><RefreshCw /></button><button className="icon-button" title="Полный выбор репозитория" onClick={() => setPicker(true)}><Plus /></button></div></div>
    <div className="scroll-area inset">
      {!nativeAvailable && <p className="hint">Доступно в приложении NotCode Desktop.</p>}
      {nativeAvailable && !profile && <Empty title="Нужен вход через GitHub"><p>Войдите, чтобы видеть свои репозитории и клонировать их одним щелчком.</p><button onClick={openSettings}>Войти в настройках</button></Empty>}
      {profile && !repos && <SpinnerLabel>Загружаю репозитории…</SpinnerLabel>}
      {profile && repos && !repos.length && <p className="hint">Репозиториев нет.</p>}
      {profile && repos && repos.slice(0, 30).map(repo => <div className="server-card" key={repo.fullName}>
        <div className="server-title">{repo.private ? <Lock /> : <Globe />}<strong>{repo.fullName}</strong></div>
        <small>{repo.defaultBranch} · обновлён {new Date(repo.updatedAt).toLocaleDateString('ru-RU')}</small>
        <div className="toolbar">
          <button disabled={!!busy} onClick={() => void clone(repo)}><FolderGit2 />{busy === repo.fullName ? 'Клонирую…' : 'Клонировать'}</button>
          <CopyButton value={repo.htmlUrl} title="Скопировать ссылку на репозиторий" />
          <button className="icon-button" title="Открыть на GitHub" onClick={() => void openUrl(repo.htmlUrl).catch(report)}><ExternalLink /></button>
        </div>
      </div>)}
      {projectId && branches && branches.length > 0 && <div className="settings-section" style={{ marginTop: 'var(--spacing-4)' }}>
        <h3><GitBranch /> Ветки проекта</h3>
        {branches.slice(0, 20).map(branch => <div className="hotkey-row" key={branch}><kbd style={{ border: 'none', background: 'transparent' }}><GitBranch /></kbd><span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis' }}>{branch}</span><CopyButton value={branch} title="Скопировать имя ветки" /></div>)}
      </div>}
      {projectId && !branches && <p className="hint">Ветки проекта загружаются…</p>}
    </div>
    {picker && <RepoPicker close={() => setPicker(false)} onCloned={project => { setPicker(false); onCloned(project); }} report={report} confirm={confirm} />}
  </div>;
}
