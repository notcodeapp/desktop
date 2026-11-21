import { useEffect, useState } from 'react';
import { Bot, Clock3, FileCode2, FolderOpen, Github, LogIn, MessageSquare, Settings, Smartphone, TerminalSquare, UserRound } from 'lucide-react';
import { api, nativeAvailable, type AuthProfile } from '../lib/native';
import { basename } from '../lib/pure';
import type { BridgeStatus, Project, Session } from '../lib/types';

function sessionDate(value: number) {
  return new Intl.DateTimeFormat('ru', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }).format(new Date(value));
}

function expiryDate(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : new Intl.DateTimeFormat('ru', { day: 'numeric', month: 'long' }).format(date);
}

export function Dashboard({
  project,
  profile,
  openProject,
  openFilePick,
  openRecentProject,
  recentFiles,
  openRecent,
  login,
  openSettings,
  openAgent,
  openTerminal,
  openDevices,
  report,
}: {
  project: Project | null;
  profile: AuthProfile | null;
  openProject: () => void;
  openFilePick: () => void;
  openRecentProject: (path: string) => void;
  recentFiles: string[];
  openRecent: (path: string) => void;
  login: () => void;
  openSettings: () => void;
  openAgent: () => void;
  openTerminal: () => void;
  openDevices: () => void;
  report: (value: unknown) => void;
}) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [bridge, setBridge] = useState<BridgeStatus | null>(null);
  const [recentProjects, setRecentProjects] = useState<Project[]>([]);

  useEffect(() => {
    if (!nativeAvailable || !project) {
      setSessions([]);
      setBridge(null);
      return;
    }
    let live = true;
    void Promise.all([
      api.sessions().catch(() => [] as Session[]),
      api.bridgeStatus().catch(() => null),
    ]).then(([nextSessions, nextBridge]) => {
      if (!live) return;
      setSessions(nextSessions);
      setBridge(nextBridge);
    }).catch(report);
    return () => { live = false; };
  }, [project?.id, report]);

  useEffect(() => {
    if (!nativeAvailable) return;
    let live = true;
    void api.projectsRecent().then(list => { if (live) setRecentProjects(list); }).catch(() => {});
    return () => { live = false; };
  }, [project?.id]);

  const expires = profile ? expiryDate(profile.expiresAt) : '';
  return <div className="dashboard scroll-area">
    <section className="dashboard-hero">
      <div>
        <span className="dashboard-eyebrow">NOTCODE DESKTOP</span>
        <h1>{profile ? `Привет, ${profile.username}` : 'Ваш код. Ваши агенты.'}</h1>
        <p>{project ? `Рабочая среда готова: ${project.name}` : 'Откройте локальный проект и продолжайте работу с AI-агентами.'}</p>
      </div>
      {profile ? <div className="dashboard-profile">
        {profile.avatarUrl ? <img src={profile.avatarUrl} alt={profile.username}/> : <UserRound/>}
        <div><strong>{profile.username}</strong><small>{expires ? `Без активности — до ${expires}` : 'GitHub подключён'}</small></div>
      </div> : <button className="primary dashboard-login" onClick={login}><Github/>Войти через GitHub</button>}
    </section>

    <section className="dashboard-stats" aria-label="Состояние рабочей среды">
      <article><FolderOpen/><div><strong>{project ? '1' : '0'}</strong><span>Открытый проект</span></div></article>
      <article><MessageSquare/><div><strong>{sessions.length}</strong><span>Диалогов в проекте</span></div></article>
      <article><Smartphone/><div><strong>{bridge?.devices.length ?? 0}</strong><span>Подключённых устройств</span></div></article>
    </section>

    <div className="dashboard-grid">
      <section className="dashboard-card dashboard-workspace-card">
        <header><div><span className="dashboard-card-kicker">РАБОЧАЯ СРЕДА</span><h2>{project?.name ?? 'Проект не открыт'}</h2></div><FolderOpen/></header>
        <p>{project?.path ?? 'Выберите папку — файлы останутся на компьютере. Или откройте отдельный файл.'}</p>
        <div className="toolbar wide">
          <button className="primary" disabled={!nativeAvailable} onClick={openProject}><FolderOpen/>{project ? 'Сменить проект' : 'Открыть проект'}</button>
          <button disabled={!nativeAvailable} onClick={openFilePick}><FileCode2/>Открыть файл…</button>
        </div>
      </section>
      {recentFiles.length>0&&<section className="dashboard-card">
        <header><div><span className="dashboard-card-kicker">НЕДАВНИЕ ФАЙЛЫ</span><h2>Вне проектов</h2></div><FileCode2/></header>
        <div className="dashboard-session-list">{recentFiles.slice(0,5).map(path=><button key={path} className="recent-file" title={path} onClick={()=>openRecent(path)}><strong>{basename(path)}</strong><small>{path}</small></button>)}</div>
      </section>}
      {recentProjects.length>0&&<section className="dashboard-card">
        <header><div><span className="dashboard-card-kicker">НЕДАВНИЕ ПРОЕКТЫ</span><h2>Один клик — и работаешь</h2></div><FolderOpen/></header>
        <div className="dashboard-session-list">{recentProjects.map(recent=><button key={recent.id} className="recent-file" title={recent.path} onClick={()=>openRecentProject(recent.path)}><strong>{recent.name}</strong><small>{recent.path}</small></button>)}</div>
      </section>}

      <section className="dashboard-card">
        <header><div><span className="dashboard-card-kicker">БЫСТРЫЙ СТАРТ</span><h2>Инструменты</h2></div><Bot/></header>
        <div className="dashboard-actions">
          <button disabled={!project} onClick={openAgent}><Bot/>AI-агент</button>
          <button disabled={!project} onClick={openTerminal}><TerminalSquare/>Терминал</button>
          <button disabled={!project} onClick={openDevices}><Smartphone/>Телефон</button>
          <button onClick={openSettings}><Settings/>Настройки</button>
        </div>
      </section>

      <section className="dashboard-card dashboard-recent">
        <header><div><span className="dashboard-card-kicker">ПОСЛЕДНЯЯ АКТИВНОСТЬ</span><h2>Диалоги</h2></div><Clock3/></header>
        {!project ? <p>Откройте проект, чтобы увидеть локальную историю.</p> : !sessions.length ? <p>Диалогов пока нет. Откройте AI-панель и начните первый.</p> : <div className="dashboard-session-list">{sessions.slice(0, 4).map(session => <div key={session.id}><span><strong>{session.title}</strong><small>{session.provider} · {session.model}</small></span><time>{sessionDate(session.updatedAt)}</time></div>)}</div>}
      </section>

      <section className="dashboard-card dashboard-account-card">
        <header><div><span className="dashboard-card-kicker">АККАУНТ</span><h2>GitHub</h2></div>{profile ? <UserRound/> : <LogIn/>}</header>
        {profile ? <><div className="dashboard-account">{profile.avatarUrl ? <img src={profile.avatarUrl} alt=""/> : <UserRound/>}<div><strong>@{profile.username}</strong><small>Профиль синхронизирован</small></div></div><button onClick={openSettings}>Управление аккаунтом</button></> : <><p>Войдите, чтобы показывать профиль и использовать облачные возможности NotCode.</p><button onClick={login}><Github/>Войти через GitHub</button></>}
      </section>
    </div>
  </div>;
}
