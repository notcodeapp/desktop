import { useCallback, useEffect, useState } from 'react';
import { User, Cpu, Sparkles, Plug, Mic, ShieldCheck, X, Github, RefreshCw, Trash2, Pencil, Plus, FolderGit2, Palette, Keyboard, Brain, FlaskConical, Sun, Moon, Save, Cat, CloudUpload, Globe } from 'lucide-react';
import { api, event, nativeAvailable, type AuthProfile, type OpencodeModel } from '../lib/native';
import { agentCatalog } from '../lib/agents';
import { applyTheme, currentTheme, type ThemeName } from '../lib/theme';
import type { ActivityStats, CliStatus, Project, ProviderProfile, Skill, VoiceStatus } from '../lib/types';
import { Dropdown, Modal, SpinnerLabel, type Confirm, type Report } from './UI';
import { LocalModelsPanel } from './LocalModelsPanel';
import { RepoPicker } from './GitHubPanel';
import { PetSection } from './Pet';
function CliAdapters() {
  const [status,setStatus] = useState<CliStatus[]>([]);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;void Promise.all(['codex','claude','opencode'].map(p=>api.cliStatus(p).catch(()=>null))).then(list=>{if(live)setStatus(list.filter((s):s is CliStatus=>!!s));});return()=>{live=false;};},[]);
  return <>{agentCatalog.filter(a=>a.id==='codex'||a.id==='claude'||a.id==='opencode').map(a=>{const found=status.find(s=>s.provider===a.id);return <div key={a.id} className="adapter-row"><strong>{a.label}</strong><small>{found?(found.available?`${found.version} · ${found.path}`:found.error):a.note}</small></div>;})}</>;
}
function GitHubAuth({onProfile}:{onProfile?:(profile:AuthProfile|null)=>void}) {
  const [profile, setProfile] = useState<AuthProfile | null>(null);
  const [checking, setChecking] = useState(true);
  useEffect(() => {
    if (!nativeAvailable) return;
    let live = true;
    let unlisten: (() => void) | undefined;
    void api.authMe()
      .then(p => { if (live) { setProfile(p); onProfile?.(p); } })
      .catch(() => {})
      .finally(() => { if (live) setChecking(false); });
    void event<AuthProfile | null>('auth:profile', p => { if (live) { setProfile(p); onProfile?.(p); } }).then(fn => { if (live) unlisten = fn; else fn(); });
    return () => { live = false; unlisten?.(); };
  }, [onProfile]);
  const login = () => { void api.authLogin().catch(console.error); };
  const logout = () => { void api.authLogout().then(() => { setProfile(null); onProfile?.(null); }).catch(console.error); };
  if (checking) return <p className="hint">Проверка сессии…</p>;
  if (profile) return <div className="account-card"><img src={profile.avatarUrl} alt="" width={40} height={40}/><div className="account-meta"><strong>{profile.username}</strong><small>GitHub · сессия активна до {new Date(profile.expiresAt).toLocaleDateString('ru-RU')}</small></div><button type="button" onClick={logout}>Выйти</button></div>;
  return <div className="form-stack"><button type="button" className="primary" onClick={login}><Github/>Войти через GitHub</button><p className="hint">Откроется браузер; после входа NotCode вернётся по ссылке notcode://auth. Токен хранится только в Windows Credential Manager.</p></div>;
}
function SkillsSection({projectId,report,confirm}:{projectId?:string;report:Report;confirm:Confirm}) {
  const [skills,setSkills]=useState<Skill[]>([]);const [busy,setBusy]=useState(false);
  const [editor,setEditor]=useState<{id?:string;name:string;description:string;body:string;enabled:boolean;scope:string}|null>(null);
  const refresh=useCallback(async()=>{setSkills(await api.skillsList());},[]);
  useEffect(()=>{void refresh().catch(report);},[refresh,report,projectId]);
  async function save(){
    if(!editor||busy)return;
    if(!editor.name.trim()||!editor.body.trim()){report('У скилла должны быть название и инструкции.');return;}
    setBusy(true);
    try{await api.skillSave({id:editor.id??null,name:editor.name,description:editor.description,body:editor.body,enabled:editor.enabled,scope:editor.scope});setEditor(null);await refresh();}catch(e){report(e);}finally{setBusy(false);}
  }
  async function remove(skill:Skill){if(!await confirm('Удалить скилл?',`«${skill.name}» (${skill.scope==='global'?'глобальный':'проектный'}) будет удалён.`))return;setBusy(true);try{await api.skillDelete(skill.id,skill.scope);await refresh();}catch(e){report(e);}finally{setBusy(false);}}
  async function toggle(skill:Skill){setBusy(true);try{await api.skillSave({id:skill.id,name:skill.name,description:skill.description,body:skill.body,enabled:!skill.enabled,scope:skill.scope});await refresh();}catch(e){report(e);}finally{setBusy(false);}}
  const groups=[{scope:'global',title:'Глобальные · во всех проектах'},{scope:'project',title:'Проектные · только в текущем проекте'}];
  return <div className="form-stack">
    <p className="hint">Скиллы — это инструкции, которые агент получает автоматически в начале генерации. Включённые скиллы добавляются к системному промпту выбранного проекта.</p>
    <div className="toolbar"><button type="button" className="primary" disabled={busy} onClick={()=>setEditor({name:'',description:'',body:'',enabled:true,scope:projectId?'project':'global'})}><Plus/>Новый скилл</button><button type="button" className="icon-button" title="Обновить" disabled={busy} onClick={()=>void refresh().catch(report)}><RefreshCw/></button></div>
    {editor&&<form className="skill-editor" onSubmit={e=>{e.preventDefault();void save();}}>
      <h3>{editor.id?'Изменить скилл':'Новый скилл'}</h3>
      <label>Название<input required maxLength={120} value={editor.name} onChange={e=>setEditor({...editor,name:e.target.value})} placeholder="Например: Стиль коммитов"/></label>
      <label>Краткое описание<input maxLength={500} value={editor.description} onChange={e=>setEditor({...editor,description:e.target.value})} placeholder="Когда агент должен применять скилл"/></label>
      <label>Инструкции<textarea required rows={7} value={editor.body} onChange={e=>setEditor({...editor,body:e.target.value})} placeholder="Текст, который агент получит перед ответом…"/></label>
      <div className="toolbar"><Dropdown label="Область" value={editor.scope} options={[{value:'global',label:'Глобальный'},...(projectId?[{value:'project',label:'Текущий проект'}]:[])]} onChange={value=>setEditor({...editor,scope:value})}/><label className="check-row"><input type="checkbox" checked={editor.enabled} onChange={e=>setEditor({...editor,enabled:e.target.checked})}/><span>Включён</span></label></div>
      <div className="toolbar"><button className="primary" disabled={busy}>{busy?'Сохраняю…':'Сохранить скилл'}</button><button type="button" disabled={busy} onClick={()=>setEditor(null)}>Отмена</button></div>
    </form>}
    {groups.map(group=><div key={group.scope} className="skill-group"><h3>{group.title}</h3>{group.scope==='project'&&!projectId&&<p className="hint">Откройте проект, чтобы создавать проектные скиллы.</p>}{skills.filter(s=>s.scope===group.scope).map(skill=><div key={skill.id} className={`skill-row ${skill.enabled?'':'disabled'}`}><div className="skill-meta"><strong>{skill.name}</strong>{skill.description&&<small>{skill.description}</small>}<small className="muted">{skill.enabled?'Включён':'Выключен'} · {skill.body.length} символов</small></div><div className="toolbar"><button type="button" className={`icon-button ${skill.enabled?'active':''}`} title={skill.enabled?'Выключить':'Включить'} disabled={busy} onClick={()=>void toggle(skill)}><Sparkles/></button><button type="button" className="icon-button" title="Изменить" disabled={busy} onClick={()=>setEditor({id:skill.id,name:skill.name,description:skill.description,body:skill.body,enabled:skill.enabled,scope:skill.scope})}><Pencil/></button><button type="button" className="icon-button" title="Удалить" disabled={busy} onClick={()=>void remove(skill)}><Trash2/></button></div></div>)}{!skills.some(s=>s.scope===group.scope)&&(group.scope==='global'||projectId)&&<p className="hint">Пока пусто.</p>}</div>)}
  </div>;
}
function VoiceSection({report}:{report:Report}) {
  const [status,setStatus]=useState<VoiceStatus|null>(null);
  const [language,setLanguage]=useState(()=>window.localStorage.getItem('notcode.voice.language')||'ru-RU');
  const [testing,setTesting]=useState(false);const [heard,setHeard]=useState('');
  useEffect(()=>{
    if(!nativeAvailable)return;
    let live=true;const stops:(()=>void)[]=[];
    void api.voiceStatus().then(value=>{if(live)setStatus(value);}).catch(report);
    const track=(promise:Promise<()=>void>)=>{void promise.then(fn=>{if(live)stops.push(fn);else fn();}).catch(()=>{});};
    track(event<{text:string}>('voice:ready',()=>{if(live)setTesting(true);}));
    track(event<{text:string}>('voice:text',payload=>{if(live&&payload.text)setHeard(old=>old?`${old} ${payload.text}`:payload.text);}));
    track(event<{message:string}>('voice:error',payload=>{if(live){setTesting(false);report(payload.message);}}));
    track(event<{text:string}>('voice:stopped',()=>{if(live)setTesting(false);}));
    return()=>{live=false;stops.forEach(fn=>fn());};
  },[report]);
  function chooseLanguage(value:string){setLanguage(value);window.localStorage.setItem('notcode.voice.language',value);}
  async function test(){
    if(testing){await api.voiceStop().catch(report);setTesting(false);return;}
    setHeard('');
    try{await api.voiceStart(language);}catch(e){report(e);}
  }
  return <div className="form-stack">
    <p className="hint">Голосовой ввод работает локально через Windows Speech (System.Speech) и не отправляет аудио в облако. Кнопка микрофона находится в поле сообщения агента.</p>
    {!status&&<SpinnerLabel>Проверяю движок…</SpinnerLabel>}
    {status&&!status.available&&<p className="warning" role="alert">{status.engine}</p>}
    {status?.available&&<>
      <div className="adapter-row"><strong>{status.engine}</strong><small>Движок доступен{status.running?' · сейчас идёт запись':''}</small></div>
      <label className="field-label"><span>Язык распознавания</span><Dropdown label="Язык распознавания" value={language} options={[{value:'ru-RU',label:'Русский (ru-RU)'},{value:'en-US',label:'English (en-US)'}]} onChange={chooseLanguage}/></label>
      <div className="toolbar"><button type="button" className={testing?'':'primary'} onClick={()=>void test()}><Mic/>{testing?'Остановить проверку':'Проверить микрофон'}</button>{testing&&<SpinnerLabel>Слушаю…</SpinnerLabel>}</div>
      {heard&&<div className="server-card"><strong>Распознано</strong><p>{heard}</p></div>}
      <p className="hint">Если распознавание не работает, проверьте микрофон в параметрах Windows → Конфиденциальность → Микрофон и установите язык распознавания в системе.</p>
    </>}
  </div>;
}
/// Хранилище аккаунта: провайдеры синхронизируются между устройствами
/// через воркер (сервер хранит их зашифрованными).
/// Google Drive: статус привязки, подключение, бэкап чатов.
function GoogleDriveSection({profile, report}:{profile: AuthProfile | null; report: Report}) {
  const [status,setStatus]=useState<{connected:boolean;email:string|null}|null>(null);
  const [busy,setBusy]=useState('');
  const [result,setResult]=useState('');
  const refresh=useCallback(async()=>{ if(!profile){setStatus(null);return;} try{ setStatus(await api.googleStatus()); }catch(e){ setStatus({connected:false,email:null}); } },[profile]);
  useEffect(()=>{ void refresh(); },[refresh]);
  if (!profile) return null;
  return <div className="settings-section">
    <header><CloudUpload/><h3>Google Drive — бэкап чатов</h3></header>
    <p className="hint">Чаты сохраняются одним JSON-файлом в скрытой папке приложения на вашем Google Drive. Сервер хранит только refresh-токен в зашифрованном виде.</p>
    {status&&!status.connected&&<button type="button" className="primary" disabled={busy!==''} onClick={()=>{ setBusy('connect'); void api.googleConnect().catch(e=>{ setBusy(''); report(e); }); }}><Globe/>Подключить Google Drive</button>}
    {status?.connected&&<>
      <div className="adapter-row"><strong>{status.email || 'Google аккаунт'}</strong><small>подключён</small></div>
      <div className="toolbar">
        <button type="button" className="primary" disabled={busy!==''} onClick={()=>{ setBusy('push'); void (async()=>{ try{ const r=await api.driveBackupPush(); setResult(`Бэкап сохранён (${r.bytes} байт).`); }catch(e){ report(e); } finally{ setBusy(''); } })(); }}><CloudUpload/>Сохранить чаты в Drive</button>
        <button type="button" disabled={busy!==''} onClick={()=>{ setBusy('pull'); void (async()=>{ try{ const r=await api.driveBackupPull(); setResult(r && !('error' in r) && r.backup ? 'Бэкап загружен с Drive (просмотр — в консоли).' : 'Бэкапа ещё нет.'); }catch(e){ report(e); } finally{ setBusy(''); } })(); }}>Скачать из Drive</button>
        <button type="button" disabled={busy!==''} onClick={()=>{ void api.googleDisconnect().then(()=>refresh()).catch(e=>report(e)); }}><X/>Отключить</button>
      </div>
      {result&&<p className="hint" role="status">{result}</p>}
    </>}
    {busy==='connect'&&<SpinnerLabel>Открываю браузер…</SpinnerLabel>}
    {busy==='push'&&<SpinnerLabel>Сохраняю…</SpinnerLabel>}
    {busy==='pull'&&<SpinnerLabel>Загружаю…</SpinnerLabel>}
  </div>;
}

function AccountVaultSection({profile, report, reloadProviders}:{profile: AuthProfile | null; report: Report; reloadProviders: () => void}) {
  const [items,setItems]=useState<{id:string;name:string;baseUrl:string;models:string[];key:string|null;updatedAt:number}[]|null>(null);
  const [busy,setBusy]=useState('');
  const refresh=useCallback(async()=>{ if(!profile){setItems(null);return;} try{ setItems(await api.vaultProvidersList()); }catch(e){ setItems([]); report(e); } },[profile,report]);
  useEffect(()=>{ void refresh(); },[refresh]);
  if (!profile) return null;
  async function importOne(item:{id:string;name:string;baseUrl:string;models:string[];key:string|null}) {
    setBusy(item.id);
    try {
      await api.vaultProviderImport({ id: item.id, name: item.name, baseUrl: item.baseUrl, models: item.models, key: item.key });
      reloadProviders();
      setItems(old => (old ?? []).filter(i => i.id !== item.id));
    } catch(e) { report(e); } finally { setBusy(''); }
  }
  return <div className="settings-section">
    <header><CloudUpload/><h3>Хранилище аккаунта</h3></header>
    <p className="hint">Провайдеры и ключи хранятся на сервере в зашифрованном виде и подтягиваются на всех устройствах, где вы вошли.</p>
    {!items&&<SpinnerLabel>Загружаю…</SpinnerLabel>}
    {items&&!items.length&&<p className="hint">В аккаунте пока нет провайдеров. Отправьте локальные — они станут доступны на других устройствах.</p>}
    {items?.map(item=><div key={item.id} className="provider-row">
      <div className="provider-meta">
        <strong>{item.name || item.id}</strong>
        <small>{item.baseUrl}</small>
        <small className="muted">{item.models.length} модел. · ключ {item.key ? 'есть' : 'нет'}</small>
      </div>
      <div className="toolbar">
        <button type="button" disabled={busy===item.id} onClick={()=>void importOne(item)}>{busy===item.id?'…':'Импортировать'}</button>
      </div>
    </div>)}
    <div className="toolbar">
      <button type="button" disabled={busy!==''} onClick={()=>{ setBusy('push-all'); void (async()=>{ try{ const list=await api.providerProfiles(); for(const p of list){ await api.vaultProviderPush(p.id); } }catch(e){report(e);} finally{ setBusy(''); } })(); }}><CloudUpload/>Отправить локальные провайдеры</button>
    </div>
    <ChatVaultSection report={report}/>
  </div>;
}

/// Чаты из аккаунта (например, синхронизированные с телефона или CLI) — импорт в проект.
function ChatVaultSection({report}:{report: Report}) {
  const [chats,setChats]=useState<{vaultId:string;name:string;model:string;messagesCount:number;updatedAt:number}[]|null>(null);
  const [busy,setBusy]=useState('');
  const [done,setDone]=useState('');
  const refresh=useCallback(async()=>{ try{ setChats(await api.vaultChatsList()); }catch(e){ setChats([]); report(e); } },[]);
  useEffect(()=>{ void refresh(); },[refresh]);
  async function importChat(chat:{vaultId:string;name:string}) {
    setBusy(chat.vaultId);
    try {
      await api.vaultChatImport(chat.vaultId);
      setDone(`Чат «${chat.name}» импортирован — откройте AI-панель.`);
      await refresh();
    } catch(e) { report(e); } finally { setBusy(''); }
  }
  return <div className="settings-section">
    <header><Sparkles/><h3>Чаты из аккаунта</h3></header>
    <p className="hint">Диалоги, синхронизированные с других устройств (CLI, телефон). Импорт создаёт копию чата в текущем проекте.</p>
    {!chats&&<SpinnerLabel>Загружаю…</SpinnerLabel>}
    {chats&&!chats.length&&<p className="hint">В аккаунте нет чатов.</p>}
    {chats?.map(chat=><div key={chat.vaultId} className="provider-row">
      <div className="provider-meta">
        <strong>{chat.name}</strong>
        <small className="muted">{chat.model || 'без модели'} · {chat.messagesCount} сообщений</small>
      </div>
      <button type="button" disabled={busy===chat.vaultId} onClick={()=>void importChat(chat)}>Импортировать</button>
    </div>)}
    {done&&<p className="hint" role="status">{done}</p>}
  </div>;
}
function ActivitySection({report}:{report:Report}) {
  const [stats,setStats]=useState<ActivityStats|null>(null);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;void api.activityStats().then(value=>{if(live)setStats(value);}).catch(report);return()=>{live=false;};},[report]);
  if(!nativeAvailable) return <p className="hint">Статистика доступна в приложении NotCode Desktop.</p>;
  if(!stats) return <SpinnerLabel>Считаю активность…</SpinnerLabel>;
  const max=Math.max(1,...stats.days.map(d=>d.messages));
  const formatDay=(date:string)=>new Date(`${date}T00:00:00`).toLocaleDateString('ru-RU',{day:'numeric',month:'short'});
  return <div className="form-stack">
    <div className="settings-section"><header><Sparkles/><h3>За всё время</h3></header>
      <div className="settings-stats">
        <article><strong>{stats.messages}</strong><span>Сообщений</span></article>
        <article><strong>{stats.sessions}</strong><span>Диалогов</span></article>
        <article><strong>{stats.toolCalls}</strong><span>Действий агента</span></article>
        <article><strong>{stats.peakDay?`${stats.peakDayMessages}`:'—'}</strong><span>{stats.peakDay?`Пик · ${formatDay(stats.peakDay)}`:'Пика ещё нет'}</span></article>
      </div>
    </div>
    <div className="settings-section"><header><Sparkles/><h3>Последние 14 дней</h3></header>
      {stats.messages===0?<p className="hint">Пока нет сообщений. Напишите агенту — график оживёт.</p>:<div className="activity-chart" role="img" aria-label="Сообщений по дням за две недели">
        {stats.days.map(d=><div key={d.date} className="activity-column" title={`${formatDay(d.date)}: ${d.messages}`}>
          <div className="activity-bar" style={{height:`${Math.round(d.messages/max*100)}%`,opacity:d.messages?1:.25}}/>
          <small>{new Date(`${d.date}T00:00:00`).getDate()}</small>
        </div>)}
      </div>}
    </div>
    <div className="settings-section"><header><Sparkles/><h3>Привычки</h3></header>
      <div className="settings-stats">
        <article><strong>{stats.streak}</strong><span>Дней подряд</span></article>
        <article><strong>{stats.activeDays}</strong><span>Активных дней за год</span></article>
      </div>
      <p className="hint">Статистика считается локально из истории диалогов и не покидает компьютер.</p>
    </div>
  </div>;
}
function AppearanceSection({report,minimapOn,onToggleMinimap,titlebarCustom,onToggleTitlebar}:{report:Report;minimapOn:boolean;onToggleMinimap:()=>void;titlebarCustom:boolean;onToggleTitlebar:()=>void}) {
  const [theme,setTheme]=useState<ThemeName>(()=>currentTheme());
  function choose(value:ThemeName){setTheme(value);try{applyTheme(value);}catch(e){report(e);}}
  return <div className="form-stack">
    <div className="settings-section"><header><Palette/><h3>Тема</h3></header>
      <div className="theme-choices">
        <button type="button" className={`theme-choice ${theme==='dark'?'selected':''}`} aria-pressed={theme==='dark'} onClick={()=>choose('dark')}><Moon/><strong>Тёмная</strong><small>По умолчанию</small></button>
        <button type="button" className={`theme-choice ${theme==='light'?'selected':''}`} aria-pressed={theme==='light'} onClick={()=>choose('light')}><Sun/><strong>Светлая</strong><small>Мягкие зелёные акценты</small></button>
      </div>
      <p className="hint">Тема применяется ко всему приложению, включая редактор и терминал, и запоминается на этом устройстве.</p>
    </div>
    <div className="settings-section"><header><Palette/><h3>Плотность интерфейса</h3></header>
      <label className="remember-row"><input type="checkbox" checked={minimapOn} onChange={onToggleMinimap}/><span>Мини-карта в редакторе</span></label>
      <label className="remember-row"><input type="checkbox" checked={titlebarCustom} onChange={onToggleTitlebar}/><span>Кастомная строка заголовка (Title Bar)</span></label>
      <p className="hint">Компактная плотность включена по умолчанию и подстраивается под размер окна. Выключение кастомной строки возвращает системные кнопки Windows.</p>
    </div>
  </div>;
}
function MemorySection({projectId,report}:{projectId?:string;report:Report}) {
  const [memory,setMemory]=useState<{global:string;project:string|null}|null>(null);
  const [busy,setBusy]=useState(false);
  const [savedScope,setSavedScope]=useState('');
  useEffect(()=>{if(!nativeAvailable)return;let live=true;void api.memoryGet().then(value=>{if(live)setMemory(value);}).catch(report);return()=>{live=false;};},[report,projectId]);
  async function save(scope:'global'|'project'){
    if(!memory||busy)return;
    setBusy(true);setSavedScope('');
    try{await api.memorySet(scope,scope==='global'?memory.global:memory.project??'');setSavedScope(scope);}catch(e){report(e);}finally{setBusy(false);}
  }
  if(!nativeAvailable) return <p className="hint">Память агента доступна в приложении NotCode Desktop.</p>;
  if(!memory) return <SpinnerLabel>Читаю память…</SpinnerLabel>;
  return <div className="form-stack">
    <p className="hint">Память — это долговременный контекст, который агент получает в начале каждой генерации: привычки, договорённости, устройство проекта. Память проекта хранится в его папке (.notcode/memory.md), глобальная — в данных NotCode.</p>
    <label className="field-label"><span>Глобальная память · во всех проектах</span>
      <textarea rows={6} value={memory.global} onChange={e=>setMemory({...memory,global:e.target.value})} placeholder="Например: «Отвечай кратко. В этом окружении используем PowerShell 7.»"/>
    </label>
    <div className="toolbar"><button type="button" className="primary" disabled={busy} onClick={()=>void save('global')}><Save/>{savedScope==='global'?'Сохранено':'Сохранить глобальную'}</button></div>
    {projectId&&memory.project!==null?<>
      <label className="field-label"><span>Память проекта</span>
        <textarea rows={6} value={memory.project} onChange={e=>setMemory({...memory,project:e.target.value})} placeholder="Например: «Сборка — npm run build. Тесты требуют Node 22. Не трогать папку legacy/.»"/>
      </label>
      <div className="toolbar"><button type="button" className="primary" disabled={busy} onClick={()=>void save('project')}><Save/>{savedScope==='project'?'Сохранено':'Сохранить проектную'}</button><button type="button" className="icon-button" title="Перечитать" disabled={busy} onClick={()=>{void api.memoryGet().then(setMemory).catch(report);}}><RefreshCw/></button></div>
    </>:<p className="hint">Откройте проект, чтобы вести отдельную память для него.</p>}
    <p className="hint">Чтобы удалить память, очистите поле и сохраните. Память не отправляется никуда, кроме выбранного вами провайдера модели.</p>
  </div>;
}
function KeyboardSection(){
  const rows:{keys:string;action:string}[]=[
    {keys:'Ctrl+K',action:'Центр команд: поиск по всем действиям'},
    {keys:'Ctrl+O',action:'Открыть папку проекта'},
    {keys:'Ctrl+S',action:'Сохранить активный файл'},
    {keys:'Ctrl+W',action:'Закрыть вкладку'},
    {keys:'Ctrl+Shift+F',action:'Поиск по проекту'},
    {keys:'Ctrl+B',action:'Показать или скрыть проводник'},
    {keys:'Ctrl+`',action:'Показать или скрыть терминал'},
    {keys:'Ctrl+Enter',action:'Отправить сообщение агенту'},
  ];
  return <div className="form-stack">
    <div className="settings-section"><header><Keyboard/><h3>Сочетания клавиш</h3></header>
      <div className="hotkey-list">{rows.map(row=><div key={row.keys} className="hotkey-row"><kbd>{row.keys}</kbd><span>{row.action}</span></div>)}</div>
    </div>
  </div>;
}
function ProviderProfiles({report,confirm}:{report:Report;confirm:Confirm}) {
  const [profiles,setProfiles]=useState<ProviderProfile[]|null>(null);
  const [editor,setEditor]=useState<null|{id?:string;name:string;baseUrl:string;models:string;secret:string}>(null);
  const [busy,setBusy]=useState(false);
  const [message,setMessage]=useState('');
  const load=useCallback(async()=>{setProfiles(await api.providerProfiles());},[]);
  useEffect(()=>{void load().catch(report);},[load,report]);
  async function fetchModels(){
    if(!editor||busy)return;
    setMessage('Проверяю API с /v1 и без /v1…');
    try{const found=await api.compatibleModels(editor.baseUrl,editor.secret);setEditor({...editor,baseUrl:found.baseUrl,models:found.models.join(', ')});setMessage(`Загружено моделей: ${found.models.length}. Теперь нажмите «Сохранить».`);}catch(e){setMessage(e instanceof Error?e.message:String(e));}
  }
  async function save(){
    if(!editor||busy)return;
    setBusy(true);setMessage('');
    try{
      const models=editor.models.split(/[\n,]+/).map(v=>v.trim()).filter(Boolean);
      const saved=await api.providerProfilesSave({id:editor.id??null,name:editor.name,baseUrl:editor.baseUrl,models});
      if(editor.secret)await api.keySet(`profile:${saved.id}`,editor.secret);
      setEditor(null);await load();
      window.dispatchEvent(new CustomEvent('native:provider-changed',{detail:{provider:'compatible'}}));
    }catch(e){report(e);}finally{setBusy(false);}
  }
  async function remove(profile:ProviderProfile){
    if(!await confirm('Удалить провайдера?',`«${profile.name}» и его API-ключ будут удалены. Диалоги не затрагиваются.`))return;
    setBusy(true);try{await api.providerProfilesDelete(profile.id);await load();}catch(e){report(e);}finally{setBusy(false);}
  }
  async function activate(profile:ProviderProfile){
    setBusy(true);try{await api.providerProfilesSetActive(profile.id);await load();window.dispatchEvent(new CustomEvent('native:provider-changed',{detail:{provider:'compatible'}}));}catch(e){report(e);}finally{setBusy(false);}
  }
  return <div className="form-stack">
    <p className="hint">Добавляйте сколько нужно провайдеров — OpenRouter, DeepSeek, локальный шлюз и т.д. Активный провайдер питает диалог агента: его модели появляются в выборе модели. Ключи лежат в Windows Credential Manager и не видны модели.</p>
    <div className="toolbar"><button type="button" className="primary" disabled={busy} onClick={()=>setEditor({name:'',baseUrl:'https://',models:'',secret:''})}><Plus/>Добавить провайдера</button><button type="button" className="icon-button" title="Обновить" disabled={busy} onClick={()=>void load().catch(report)}><RefreshCw/></button></div>
    {!profiles&&<SpinnerLabel>Загружаю провайдеров…</SpinnerLabel>}
    {profiles&&!profiles.length&&<p className="hint">Пока пусто: добавьте первого провайдера или продолжайте пользоваться legacy-настройкой ниже.</p>}
    {profiles?.map(profile=><div key={profile.id} className={`provider-row ${profile.active?'active':''}`}>
      <div className="provider-meta">
        <strong>{profile.name}{profile.active&&<span className="settings-badge">активен</span>}</strong>
        <small>{profile.baseUrl}</small>
        <small className="muted">{profile.models.length?`${profile.models.length} модел.: ${profile.models.slice(0,3).join(', ')}${profile.models.length>3?'…':''}`:'Модели не заданы'}</small>
      </div>
      <div className="toolbar">
        {!profile.active&&<button type="button" disabled={busy} onClick={()=>void activate(profile)}>Сделать активной</button>}
        <button type="button" className="icon-button" title="Изменить" disabled={busy} onClick={()=>setEditor({id:profile.id,name:profile.name,baseUrl:profile.baseUrl,models:profile.models.join(', '),secret:''})}><Pencil/></button>
        <button type="button" className="icon-button" title="Удалить" disabled={busy} onClick={()=>void remove(profile)}><Trash2/></button>
      </div>
    </div>)}
    {editor&&<Modal title={editor.id?'Изменить провайдера':'Новый провайдер'} close={()=>{if(!busy)setEditor(null);}}><form className="form-stack" onSubmit={e=>{e.preventDefault();void save();}}>
      <label>Название<input required maxLength={120} value={editor.name} onChange={e=>setEditor({...editor,name:e.target.value})} placeholder="Например: OpenRouter"/></label>
      <label>Адрес API (HTTPS)<input required type="url" value={editor.baseUrl} onChange={e=>setEditor({...editor,baseUrl:e.target.value})} placeholder="https://openrouter.ai/api/v1"/></label>
      <label>Модели через запятую<textarea rows={3} value={editor.models} onChange={e=>setEditor({...editor,models:e.target.value})} placeholder="model-one, model-two"/></label>
      <label>API-ключ<input type="password" autoComplete="off" value={editor.secret} onChange={e=>setEditor({...editor,secret:e.target.value})} placeholder={editor.id?'Оставьте пустым, чтобы сохранить текущий ключ':'Ключ провайдера'}/></label>
      <div className="toolbar"><button type="button" disabled={busy} onClick={()=>void fetchModels()}>Fetch Models</button><button className="primary" disabled={busy}>{busy?'Сохраняю…':'Сохранить'}</button><button type="button" disabled={busy} onClick={()=>setEditor(null)}>Отмена</button></div>
      <p className="hint" role="status">{message||'Fetch Models проверит /models и /v1/models и подставит список. Ключ хранится в Windows Credential Manager.'}</p>
    </form></Modal>}
  </div>;
}
/// OpenCode Zen: ключ шлюза opencode.ai + живой каталог моделей (free/paid).
function OpencodeSection({report}:{report:Report}) {
  const [models,setModels]=useState<OpencodeModel[]|null>(null);
  const [key,setKey]=useState('');
  const [busy,setBusy]=useState(false);
  const [saved,setSaved]=useState('');
  useEffect(()=>{ void api.opencodeModels().then(setModels).catch(()=>setModels([])); },[]);
  async function saveKey(){
    if(!key.trim())return;
    setBusy(true);setSaved('');
    try{ await api.keySet('opencode',key.trim()); setKey(''); setSaved('Ключ сохранён в Windows Credential Manager — выберите OpenCode Zen в поле провайдера.'); }
    catch(e){ report(e); } finally { setBusy(false); }
  }
  return <div className="settings-section"><header><Sparkles/><h3>OpenCode Zen · бесплатные модели</h3></header>
    <p className="hint">Шлюз opencode.ai даёт бесплатные модели (big-pickle, DeepSeek V4.1 Flash и другие) и платные флагманы. Ключ создаётся на opencode.ai/console и хранится только в Windows Credential Manager.</p>
    <label className="field-label"><span>API-ключ OpenCode</span>
      <input type="password" autoComplete="off" value={key} onChange={e=>setKey(e.target.value)} placeholder="Вставьте ключ с opencode.ai/console"/>
    </label>
    <div className="toolbar"><button type="button" className="primary" disabled={busy||!key.trim()} onClick={()=>void saveKey()}>{busy?'Сохраняю…':'Сохранить ключ'}</button><button type="button" className="icon-button" title="Обновить каталог" onClick={()=>{setModels(null);void api.opencodeModels().then(setModels).catch(()=>setModels([]));}}><RefreshCw/></button></div>
    {saved&&<p className="hint" role="status">{saved}</p>}
    {!models&&<SpinnerLabel>Загружаю каталог…</SpinnerLabel>}
    {models&&!models.length&&<p className="hint">Каталог недоступен — проверьте соединение.</p>}
    {models?.slice(0,12).map(m=><div key={m.id} className="provider-row">
      <div className="provider-meta">
        <strong>{m.name}{m.free&&<span className="settings-badge">бесплатно</span>}</strong>
        <small className="muted">{m.id} · контекст ~{Math.round(m.ctx/1024)}K{m.description?` · ${m.description}`:''}</small>
      </div>
    </div>)}
    {models&&models.length>12&&<p className="hint">И ещё {models.length-12} моделей — полный список в выборе модели диалога.</p>}
  </div>;
}
type Section='account'|'models'|'skills'|'plugins'|'voice'|'security'|'appearance'|'keyboard'|'pet'|'memory'|'experiments';
const SECTIONS:{group:string;id:Section;label:string;Icon:typeof User;badge?:string}[]=[
  {group:'Аккаунт',id:'account',label:'Аккаунт',Icon:User},
  {group:'Аккаунт',id:'models',label:'Модели',Icon:Cpu},
  {group:'Аккаунт',id:'skills',label:'Скиллы',Icon:Sparkles},
  {group:'Аккаунт',id:'plugins',label:'Плагины и MCP',Icon:Plug},
  {group:'Аккаунт',id:'voice',label:'Голос',Icon:Mic},
  {group:'Аккаунт',id:'security',label:'Безопасность',Icon:ShieldCheck},
  {group:'Интерфейс',id:'appearance',label:'Внешний вид',Icon:Palette},
  {group:'Интерфейс',id:'keyboard',label:'Горячие клавиши',Icon:Keyboard},
  {group:'Интерфейс',id:'pet',label:'Питомец',Icon:Cat},
  {group:'Дополнительно',id:'memory',label:'Память агента',Icon:Brain},
  {group:'Дополнительно',id:'experiments',label:'Эксперименты',Icon:FlaskConical,badge:'Beta'},
];
export function Settings({close,report,minimapOn,onToggleMinimap,titlebarCustom,onToggleTitlebar,petVisible,onTogglePet,projectId,onProject,onOpenMcp,confirm}:{close:()=>void;report:Report;minimapOn:boolean;onToggleMinimap:()=>void;titlebarCustom:boolean;onToggleTitlebar:()=>void;petVisible:boolean;onTogglePet:()=>void;projectId?:string;onProject:(project:Project)=>Promise<boolean>;onOpenMcp:()=>void;confirm:Confirm}) {
  const [section,setSection]=useState<Section>('account');
  const [query,setQuery]=useState('');
  const [profile,setProfile]=useState<AuthProfile|null>(null);
  const [repoPicker,setRepoPicker]=useState(false);
  const [modelsMounted,setModelsMounted]=useState(false);
  useEffect(()=>{if(section==='models')setModelsMounted(true);},[section]);
  const filtered=SECTIONS.filter(s=>!query.trim()||s.label.toLowerCase().includes(query.trim().toLowerCase()));
  const groups=[...new Set(filtered.map(s=>s.group))];
  const [url,setUrl]=useState('http://127.0.0.1:11434');const [busy,setBusy]=useState(false);const [saved,setSaved]=useState(false);
  useEffect(()=>{void api.settings().then(value=>setUrl(value.ollamaUrl)).catch(report);},[report]);
  useEffect(()=>{function keys(e:KeyboardEvent){if(e.key==='Escape'&&!document.querySelector('.scrim'))close();}window.addEventListener('keydown',keys);return()=>window.removeEventListener('keydown',keys);},[close]);
  const title=SECTIONS.find(s=>s.id===section)?.label??'';
  return <div className="settings-shell" role="dialog" aria-modal="true" aria-label="Настройки NotCode">
    <aside className="settings-nav"><header><strong>Настройки</strong><button className="icon-button" title="Закрыть настройки" onClick={close}><X/></button></header>
      <div className="settings-search"><input type="search" value={query} onChange={e=>setQuery(e.target.value)} placeholder="Поиск настроек…" aria-label="Поиск по настройкам"/></div>
      <nav aria-label="Разделы настроек">{!filtered.length&&<p className="hint" style={{padding:'0 var(--spacing-3)'}}>Ничего не найдено.</p>}{groups.map(group=><div key={group}><div className="settings-nav-group-label">{group}</div>{filtered.filter(s=>s.group===group).map(({id,label,Icon,badge})=><button key={id} type="button" className={`settings-nav-item ${section===id?'selected':''}`} aria-current={section===id?'page':undefined} onClick={()=>setSection(id)}><Icon/><span>{label}</span>{badge&&<span className="settings-badge">{badge}</span>}</button>)}</div>)}</nav>
      <footer className="hint">NotCode Desktop · локальная среда</footer>
    </aside>
    <main className="settings-body scroll-area"><div className="settings-content"><h2>{title}</h2>
      {section==='account'&&<div className="form-stack">
        <div className="settings-section"><header><User/><h3>{profile?`@${profile.username}`:'Гость'}</h3></header>
          <p className="hint">{profile?'GitHub-сессия активна. Профиль синхронизируется автоматически.':'Войдите через GitHub, чтобы открывать свои репозитории как проекты и синхронизировать профиль.'}</p>
          <GitHubAuth onProfile={setProfile}/>
        </div>
        <div className="settings-section"><header><Sparkles/><h3>Активность</h3></header>
          <ActivitySection report={report}/>
        </div>
        <div className="settings-section"><header><FolderGit2/><h3>Репозиторий как проект</h3></header>
          <p className="hint">Откройте свой репозиторий GitHub прямо из NotCode: он будет клонирован в выбранную папку и сразу станет проектом. Приватные репозитории доступны после входа.</p>
          <div className="toolbar"><button type="button" className="primary" disabled={!profile} onClick={()=>setRepoPicker(true)}><FolderGit2/>Выбрать репозиторий</button>{!profile&&<span className="hint">Сначала войдите через GitHub.</span>}</div>
        </div>
        <AccountVaultSection profile={profile} report={report} reloadProviders={()=>window.dispatchEvent(new CustomEvent('native:provider-changed',{detail:{provider:'all'}}))}/>
        <GoogleDriveSection profile={profile} report={report}/>
        <div className="settings-section settings-danger"><header><h3>Выход</h3></header>
          <div className="settings-signout"><p className="hint">Удалить учётные данные с этого устройства. Локальные проекты и настройки останутся.</p><button type="button" onClick={()=>{if(profile)void api.authLogout().then(()=>setProfile(null)).catch(report);else report('Вы не вошли в аккаунт.');}}>Выйти</button></div>
        </div>
      </div>}
      {modelsMounted&&<div className="form-stack" style={{display:section==='models'?'flex':'none'}}>
        <ProviderProfiles report={report} confirm={confirm}/>
        <OpencodeSection report={report}/>
        <h3>Ollama · локально</h3>
        <form onSubmit={e=>{e.preventDefault();setBusy(true);void api.setSettings(url).then(()=>{setSaved(true);window.dispatchEvent(new CustomEvent('native:provider-changed',{detail:{provider:'ollama'}}));}).catch(report).finally(()=>setBusy(false));}}>
          <label>Адрес Ollama<input value={url} onChange={e=>{setUrl(e.target.value);setSaved(false);}}/></label>
          <p className="hint">Разрешён только loopback HTTP без путей и авторизации. Пример: http://127.0.0.1:11434. NotCode не открывает удалённый доступ.</p>
          <button className="primary" disabled={busy}>{saved?'Сохранено':'Сохранить'}</button>
        </form>
        <LocalModelsPanel/>
      </div>}
      {section==='skills'&&<SkillsSection projectId={projectId} report={report} confirm={confirm}/>}
      {section==='plugins'&&<div className="form-stack">
        <h3>MCP-серверы</h3>
        <p className="hint">Model Context Protocol подключает внешние инструменты к агенту. Серверы запускаются как отдельные локальные процессы с вашими правами; каждое подключение и вызов инструмента подтверждается.</p>
        <div className="toolbar"><button type="button" className="primary" onClick={onOpenMcp}><Plug/>Открыть панель MCP</button></div>
        <h3>Адаптеры агентов</h3>
        {agentCatalog.filter(a=>a.id!=='codex'&&a.id!=='claude'&&a.id!=='opencode').map(a=><div key={a.id} className="adapter-row"><strong>{a.label}</strong><small>{a.note}</small></div>)}
        <CliAdapters/>
      </div>}
      {section==='voice'&&<VoiceSection report={report}/>}
      {section==='appearance'&&<AppearanceSection report={report} minimapOn={minimapOn} onToggleMinimap={onToggleMinimap} titlebarCustom={titlebarCustom} onToggleTitlebar={onToggleTitlebar}/>}
      {section==='keyboard'&&<KeyboardSection/>}
      {section==='pet'&&<div className="form-stack">
        <div className="settings-section"><header><Cat/><h3>Питомец в окне</h3></header>
          <label className="remember-row"><input type="checkbox" checked={petVisible} onChange={onTogglePet}/><span>Показывать питомца слева внизу</span></label>
          <p className="hint">Питомца можно перетаскивать мышью, спрятать кнопкой «×» на его карточке и снова включить здесь. Огонёк рядом с ним — ваша серия дней подряд.</p>
        </div>
        <PetSection report={report}/>
      </div>}
      {section==='memory'&&<MemorySection projectId={projectId} report={report}/>}
      {section==='experiments'&&<div className="form-stack">
        <div className="settings-section"><header><FlaskConical/><h3>Бета-функции</h3></header>
          <p className="hint">Голосовой ввод, импорт CLI-диалогов и связка с телефоном — экспериментальные функции. Их поведение может меняться между версиями.</p>
        </div>
      </div>}
      {section==='security'&&<div className="form-stack">
        <h3>Хранение секретов</h3>
        <p className="hint">Ollama не требует API-ключа. Ключ OpenAI-compatible API и токен GitHub хранятся в Windows Credential Manager и не сохраняются в SQLite или файлах проекта.</p>
        <h3>Границы проекта</h3>
        <p className="hint">Файловые команды ограничены выбранным проектом. PowerShell и MCP — отдельные локальные процессы с правами пользователя, не песочница.</p>
        <p className="hint">Codex и Claude Code используют ваш собственный вход в CLI: NotCode не хранит и не передаёт их токены. Это отдельные процессы с правами пользователя и собственным доступом к диску; NotCode показывает их запросы на команды, изменение файлов и дополнительный доступ, но не ограничивает их песочницей. OpenCode Zen работает через HTTPS-шлюз opencode.ai с вашим API-ключом.</p>
        <h3>Подтверждения</h3>
        <p className="hint">Галочка «Больше не спрашивать» запоминается отдельно для приложения и для каждого проекта. Команды, запись файлов и доступы внешних агентов подтверждаются каждый раз — для них галочка не показывается.</p>
      </div>}
    </div></main>
    {repoPicker&&<RepoPicker close={()=>setRepoPicker(false)} onCloned={project=>{void (async()=>{if(await onProject(project)){setRepoPicker(false);close();}})();}} report={report} confirm={confirm}/>}
  </div>;
}
