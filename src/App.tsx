import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { getVersion } from '@tauri-apps/api/app';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { DiffEditor } from '@monaco-editor/react';
import { Files, Search, GitBranch, GitCompare, Github, AppWindow, Plug, Settings as SettingsIcon, FolderOpen, X, PanelRightClose, PanelRightOpen, TerminalSquare, Save, SaveAll, Columns2, Code2, ChevronRight, AlertCircle, PanelLeftClose, ShieldCheck, Smartphone, History } from 'lucide-react';
import { api, event, nativeAvailable, type AuthProfile } from './lib/native';
import { basename, clamp, isDirty, languageFor, replaceTab, savedTab } from './lib/pure';
import type { FileHistoryEntry, FileTab, Project, WorkspaceFileChange } from './lib/types';
import { Explorer } from './components/Explorer';
import { CodeEditor } from './components/CodeEditor';
import { AgentPanel } from './components/AgentPanel';
import { TerminalPanel } from './components/TerminalPanel';
import { GitPanel } from './components/GitPanel';
import { McpPanel } from './components/McpPanel';
import { Settings } from './components/Settings';
import { Dashboard } from './components/Dashboard';
import { BridgePanel, useBridgePairing } from './components/BridgePanel';
import { CommandPalette, buildCommands } from './components/CommandPalette';
import { GitHubPanel } from './components/GitHubPanel';
import { BrowserPanel } from './components/BrowserPanel';
import { QuickOpen } from './components/QuickOpen';
import { Titlebar, buildMenuGroups } from './components/Titlebar';
import { ContextMenuPortal, type ContextMenuItem } from './components/ContextMenu';
import { Modal, Dropdown, type Confirm } from './components/UI';
import './styles/app.css';

type Navigation = 'files'|'search'|'github'|'browser'|'git'|'mcp'|'devices';
interface Confirmation { title:string; detail:string; resolve:(value:boolean)=>void; scope?:string; allowRemember:boolean }
export default function App(){
  const [project,setProject]=useState<Project|null>(null);
  const [tabs,setTabs]=useState<FileTab[]>([]);const [active,setActive]=useState('');const [targetLine,setTargetLine]=useState<number>();
  const [navigation,setNavigation]=useState<Navigation>('files');const [showSidebar,setShowSidebar]=useState(true);const [showAgent,setShowAgent]=useState(true);const [showTerminal,setShowTerminal]=useState(true);
  const [authProfile, setAuthProfile] = useState<AuthProfile | null>(null);
  const [splitEditor,setSplitEditor]=useState(false);const [secondary,setSecondary]=useState('');const [settings,setSettings]=useState(false);
  const [palette,setPalette]=useState(false);
  const [quickOpen,setQuickOpen]=useState(false);
  const [diffView,setDiffView]=useState(false);
  const [fileHistory,setFileHistory]=useState<{path:string;entries:FileHistoryEntry[]}|null>(null);
  const [tabMenu,setTabMenu]=useState<{x:number;y:number;path:string}|null>(null);
  const [recentFiles,setRecentFiles]=useState<string[]>(()=>{
    try{const raw=window.localStorage.getItem('notcode.recentFiles');const list=raw?JSON.parse(raw):[];return Array.isArray(list)?list.filter((p):p is string=>typeof p==='string').slice(0,10):[];}catch{return[];}
  });
  const [recentProjects,setRecentProjects]=useState<Project[]>([]);
  const [minimapOn,setMinimapOn]=useState(()=>window.localStorage.getItem('notcode.minimap')==='on');
  const [titlebarCustom,setTitlebarCustom]=useState(()=>window.localStorage.getItem('notcode.titlebar')!=='native');
  // Кастомный тайтлбар = без системных декораций. Переключение применяется
  // и при старте, и по клику в настройках; при ошибке остаётся текущий вид.
  useEffect(()=>{
    if(!nativeAvailable)return;
    void getCurrentWindow().setDecorations(!titlebarCustom).catch(()=>{});
  },[titlebarCustom]);
  function toggleTitlebar(){ setTitlebarCustom(old=>{ const next=!old; window.localStorage.setItem('notcode.titlebar',next?'custom':'native'); return next; }); }
  useEffect(()=>{ if(nativeAvailable&&recentProjects.length===0) void api.projectsRecent().then(list=>{ if(list.length) setRecentProjects(list); }).catch(()=>{}); },[]);
  function toggleMinimap(){ setMinimapOn(old=>{ const next=!old; window.localStorage.setItem('notcode.minimap',next?'on':'off'); window.dispatchEvent(new CustomEvent('notcode:minimap-changed',{detail:next})); return next; }); }
  const [petVisible,setPetVisible]=useState(()=>window.localStorage.getItem('notcode.pet.visible')==='on');
  function togglePet(){ setPetVisible(old=>{ const next=!old; window.localStorage.setItem('notcode.pet.visible',next?'on':'off'); return next; }); }
  function pushRecent(path:string){setRecentFiles(old=>{const next=[path,...old.filter(p=>p!==path)].slice(0,10);window.localStorage.setItem('notcode.recentFiles',JSON.stringify(next));return next;});}
  const [widths,setWidths]=useState({explorer:240,agent:360,terminal:220});const [error,setError]=useState('');const [notice,setNotice]=useState('');const [saving,setSaving]=useState(false);const [opening,setOpening]=useState(false);const [workspaceRevision,setWorkspaceRevision]=useState(0);
  const [confirmation,setConfirmation]=useState<Confirmation|null>(null);const [confirmationRemember,setConfirmationRemember]=useState(false);const confirmRef=useRef<Confirmation|null>(null);const openSequence=useRef(0);
  const [settingsMounted,setSettingsMounted]=useState(false);
  const [version,setVersion]=useState('0.3.7');
  useEffect(()=>{if(nativeAvailable)void getVersion().then(value=>{if(value)setVersion(value);}).catch(()=>{});},[]);
  const tabsRef=useRef(tabs);tabsRef.current=tabs;const projectRef=useRef(project);projectRef.current=project;
  const report=useCallback((value:unknown)=>{setError(value instanceof Error?value.message:String(value));},[]);
  const confirm:Confirm=useCallback((title,detail,allowRemember=true,scope)=>new Promise<boolean>(resolve=>{if(allowRemember&&window.localStorage.getItem(`notcode.confirm.skip.${scope??'app'}.${title}`)==='true'){resolve(true);return;}if(confirmRef.current){resolve(false);return;}const value={title,detail,resolve,scope,allowRemember};confirmRef.current=value;setConfirmationRemember(false);setConfirmation(value);}),[]);
  const settle=useCallback((value:boolean)=>{if(value&&confirmationRemember&&confirmation?.allowRemember)window.localStorage.setItem(`notcode.confirm.skip.${confirmation.scope??'app'}.${confirmation.title}`,'true');confirmRef.current?.resolve(value);confirmRef.current=null;setConfirmation(null);},[confirmation,confirmationRemember]);
  const cancelConfirm=useCallback(()=>settle(false),[settle]);
  useBridgePairing(confirm, report);
  useEffect(()=>{if(settings)setSettingsMounted(true);},[settings]);
  useEffect(()=>{if(nativeAvailable)void api.projectCurrent().then(setProject).catch(report);},[report]);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;let unlisten:(()=>void)|undefined;event<AuthProfile|null>('auth:profile',profile=>{if(live)setAuthProfile(profile);}).then(fn=>{if(live)unlisten=fn;else fn();}).catch(report);return()=>{live=false;unlisten?.();};},[report]);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;let unlisten:(()=>void)|undefined;event<string>('auth:error',message=>{if(live)setError(message);}).then(fn=>{if(live)unlisten=fn;else fn();}).catch(report);return()=>{live=false;unlisten?.();};},[report]);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;const refresh=()=>{void api.authMe().then(profile=>{if(live)setAuthProfile(profile);}).catch(error=>{if(live)report(error);});};refresh();const timer=window.setInterval(refresh,12*60*60*1000);return()=>{live=false;window.clearInterval(timer);};},[report]);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;let unlisten:(()=>void)|undefined;event<WorkspaceFileChange>('workspace:file-changed',change=>{
    if(projectRef.current?.id!==change.projectId)return;setWorkspaceRevision(value=>value+1);
    const open=tabsRef.current.find(item=>item.path===change.path);if(!open)return;
    if(isDirty(open)){setNotice(`Агент изменил ${change.path} на диске, но вкладка содержит несохранённые правки.`);return;}
    void api.read(change.path).then(content=>{if(projectRef.current?.id===change.projectId)setTabs(current=>current.map(item=>item.path===change.path?{...item,content,saved:content}:item));}).catch(report);
  }).then(fn=>{if(live)unlisten=fn;else fn();}).catch(report);return()=>{live=false;unlisten?.();};},[report]);
  useEffect(()=>{const check=(e:BeforeUnloadEvent)=>{if(tabsRef.current.some(isDirty)){e.preventDefault();e.returnValue='';}};window.addEventListener('beforeunload',check);return()=>window.removeEventListener('beforeunload',check);},[]);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;let unlisten:(()=>void)|undefined;let closing=false;
    getCurrentWindow().onCloseRequested(e=>{e.preventDefault();if(closing)return;closing=true;void(async()=>{if(await confirm('Закрыть NotCode?',`${tabsRef.current.some(isDirty)?'Есть несохранённые файлы. Изменения будут потеряны. ':''}Запущенные терминалы, MCP-серверы и генерации будут остановлены.`)){await getCurrentWindow().destroy();}})().catch(report).finally(()=>{closing=false;});}).then(fn=>{if(live)unlisten=fn;else fn();}).catch(report);
    return()=>{live=false;unlisten?.();};
  },[confirm,report]);
  async function openAuthWindow(){await api.authLogin();}

  const applyProject=useCallback((value:Project)=>{openSequence.current++;const kept=tabsRef.current.filter(t=>t.external);setTabs(kept);if(!kept.some(t=>t.path===active))setActive(kept.at(-1)?.path||'');setTargetLine(undefined);setSecondary('');setSplitEditor(false);setProject(value);setNotice('Проект открыт.');},[active]);
  const switchProject=useCallback(async(value:Project|null)=>{if(!value)return false;if(value.id===projectRef.current?.id){setProject(value);return true;}const dirtyProject=tabsRef.current.some(t=>isDirty(t)&&!t.external);if(dirtyProject&&!await confirm('Сменить проект?','Несохранённые изменения в файлах проекта будут потеряны. Сначала сохраните их или подтвердите переход. Внешние файлы останутся открытыми.'))return false;applyProject(value);return true;},[applyProject,confirm]);
  async function openProject(){if(opening)return;setOpening(true);try{const value=await api.projectOpen();if(value)await switchProject(value);}catch(e){report(e);}finally{setOpening(false);}}
  async function openFile(path:string,line?:number){const seq=++openSequence.current;const root=projectRef.current?.id;if(!root)return;
    if(tabsRef.current.some(tab=>tab.path===path)){setActive(path);setTargetLine(line);return;}
    try{const content=await api.read(path);if(seq!==openSequence.current||projectRef.current?.id!==root)return;setTabs(old=>old.some(t=>t.path===path)?old:[...old,{path,content,saved:content}]);setActive(path);setTargetLine(line);}catch(e){report(e);}}
  async function save(){const tab=tabsRef.current.find(t=>t.path===active);if(!tab||saving)return;setSaving(true);try{
    if(tab.external){await api.externalFileWrite(tab.path,tab.content);setTabs(old=>savedTab(old,tab.path,tab.content));setNotice(`Сохранено: ${tab.path}`);}
    else{const root=projectRef.current?.id;await api.write(tab.path,tab.content);if(projectRef.current?.id===root){setTabs(old=>savedTab(old,tab.path,tab.content));setNotice(`Сохранено: ${tab.path}`);}}
  }catch(e){report(e);}finally{setSaving(false);}}
  async function closeTab(path:string){const tab=tabsRef.current.find(t=>t.path===path);if(!tab)return;if(isDirty(tab)&&!await confirm('Закрыть файл без сохранения?',`${path}\nНесохранённые изменения будут потеряны.`))return;
    openSequence.current++;setTabs(old=>{const next=old.filter(t=>t.path!==path);if(active===path)setActive(next.at(-1)?.path||'');if(secondary===path)setSecondary(next.at(-1)?.path||'');return next;});}
  async function openHistory(path:string){try{setFileHistory({path,entries:await api.fileHistoryList(path)});}catch(e){report(e);}}

  // ---- восстановление вкладок: пути хранятся в localStorage, контент читается с диска
  const restoredProject = useRef('');
  useEffect(() => {
    const id = project?.id;
    if (!id) { restoredProject.current = ''; return; }
    if (restoredProject.current === id) return;
    restoredProject.current = id;
    try {
      const raw = window.localStorage.getItem(`notcode.tabs:${id}`);
      if (!raw) return;
      const saved = JSON.parse(raw) as {paths:string[];active?:string};
      for (const path of saved.paths.slice(0,20)) if (!tabsRef.current.some(tab=>tab.path===path)) void openFile(path);
      if (saved.active && saved.paths.includes(saved.active)) setActive(saved.active);
    } catch { /* битые данные — просто не восстанавливаем */ }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project?.id]);
  useEffect(() => {
    const id = project?.id;
    if (!id) return;
    const paths = tabs.filter(tab=>!tab.external).map(tab=>tab.path).slice(-20);
    const activePath = tabs.some(tab=>tab.path===active&&!tab.external)?active:paths.at(-1);
    try { window.localStorage.setItem(`notcode.tabs:${id}`, JSON.stringify({paths, active:activePath})); } catch { /* ignore */ }
  }, [tabs, active, project?.id]);
  useEffect(() => {
    // Внешние файлы живут между проектами и перезапусками.
    try {
      const raw = window.localStorage.getItem('notcode.tabs.__external__');
      if (raw) for (const path of (JSON.parse(raw) as {paths:string[]}).paths.slice(0,10)) void openExternal(path);
    } catch { /* ignore */ }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    const paths = tabs.filter(tab=>tab.external).map(tab=>tab.path).slice(-10);
    try { window.localStorage.setItem('notcode.tabs.__external__', JSON.stringify({paths})); } catch { /* ignore */ }
  }, [tabs]);
  const [historyDiffVersion,setHistoryDiffVersion]=useState<FileHistoryEntry|null>(null);
  const [historyDiffContent,setHistoryDiffContent]=useState('');
  async function showVersionDiff(timestamp:number){ if(!fileHistory)return; try{ setHistoryDiffContent(await api.fileHistoryRead(fileHistory.path,timestamp)); setHistoryDiffVersion(fileHistory.entries.find(e=>e.timestamp===timestamp)??null); }catch(e){ report(e); } }
  async function restoreHistory(timestamp:number){if(!fileHistory)return;if(!await confirm('Восстановить версию?',`${fileHistory.path}\nТекущее содержимое автоматически сохранится в историю, так что восстановление можно откатить.`))return;try{await api.fileHistoryRestore(fileHistory.path,timestamp,true);const content=await api.read(fileHistory.path);setTabs(old=>old.map(t=>t.path===fileHistory.path?{path:t.path,content,saved:content}:t));setNotice(`Восстановлено: ${fileHistory.path}`);setFileHistory(null);}catch(e){report(e);}}
  function formatBytes(size:number){return size<1024?`${size} Б`:size<1024*1024?`${(size/1024).toFixed(1)} КиБ`:`${(size/1024/1024).toFixed(1)} МиБ`;}
  /// Внешние файлы: «Открыть с помощью», drag & drop, Ctrl+O. Они живут в
  /// отдельных вкладках с абсолютным путём и не требуют открытого проекта.
  async function openExternal(rawPath:string){
    const normalized=rawPath.replace(/\\/g,'/');
    const existing=tabsRef.current.find(t=>t.external&&t.path.replace(/\\/g,'/')===normalized);
    if(existing){setActive(existing.path);setTargetLine(undefined);return;}
    try{const file=await api.externalFileRead(rawPath);const path=file.path.replace(/\\/g,'/');
      setTabs(old=>old.some(t=>t.external&&t.path.replace(/\\/g,'/')===path)?old:[...old,{path,content:file.content,saved:file.content,external:true}]);
      setActive(path);setTargetLine(undefined);setNotice(`Открыт внешний файл: ${file.name}`);pushRecent(path);
    }catch(e){report(e);}}
  async function pickExternalFile(){try{const path=await api.externalFilePick();if(path)await openExternal(path);}catch(e){report(e);}}
  async function openRecentProject(path:string){try{const value=await api.projectOpenPath(path);await switchProject(value);}catch(e){report(e);}}
  function tabMenuItems():ContextMenuItem[]{return [
    {id:'close',label:'Закрыть вкладку',hint:'Ctrl+W'},
    {id:'close-others',label:'Закрыть остальные'},
    {id:'copy-path',label:'Копировать путь'},
    {id:'reveal',label:'Показать в проводнике'},
  ];}
  async function runTabMenu(path:string,id:string){
    setTabMenu(null);
    if(id==='close'){await closeTab(path);}
    else if(id==='close-others'){for(const t of tabsRef.current){if(t.path!==path)await closeTab(t.path);}}
    else if(id==='copy-path'){try{await navigator.clipboard.writeText(path);setNotice('Путь скопирован.');}catch(e){report(e);}}
    else if(id==='reveal'){try{await revealItemInDir(path);}catch(e){report(e);}}
  }
  async function saveAsExternal(){const tab=tabsRef.current.find(t=>t.path===active);if(!tab?.external)return;try{
    const saved=await api.externalFileSaveAs(tab.content,basename(tab.path));
    if(saved){const normalized=saved.replace(/\\/g,'/');setTabs(old=>old.map(t=>t.path===tab.path?{path:normalized,content:tab.content,saved:tab.content,external:true}:t));setActive(normalized);pushRecent(normalized);setNotice(`Сохранено: ${normalized}`);}
  }catch(e){report(e);}}
  useEffect(()=>{if(!nativeAvailable)return;let live=true;let stopOpen:(()=>void)|undefined;let stopDrop:(()=>void)|undefined;
    event<string>('external:open',path=>{if(live)void openExternal(path);}).then(fn=>{if(live)stopOpen=fn;else fn();}).catch(()=>{});
    getCurrentWindow().onDragDropEvent(dropEvent=>{const payload=dropEvent.payload as {type:string;paths?:string[]};if(payload.type==='drop'&&payload.paths)for(const p of payload.paths)void openExternal(p);}).then(fn=>{if(live)stopDrop=fn;else fn();}).catch(()=>{});
    return()=>{live=false;stopOpen?.();stopDrop?.();};
  },[report]);
  const actions=useRef({openProject,save,closeTab,active});actions.current={openProject,save,closeTab,active};
  const settingsRef=useRef(settings);settingsRef.current=settings;
  const paletteRef=useRef(palette);paletteRef.current=palette;
  useEffect(()=>{function keys(e:KeyboardEvent){
    if(confirmRef.current||settingsRef.current)return;
    if((e.ctrlKey||e.metaKey)&&!e.altKey&&!e.shiftKey&&e.code==='KeyK'){e.preventDefault();e.stopPropagation();setPalette(value=>!value);return;}
    if((e.ctrlKey||e.metaKey)&&!e.altKey&&!e.shiftKey&&e.code==='KeyP'){e.preventDefault();e.stopPropagation();setQuickOpen(value=>!value);return;}
    if((e.ctrlKey||e.metaKey)&&e.code==='Tab'&&!e.altKey){
      e.preventDefault();e.stopPropagation();
      const list=tabsRef.current;if(list.length<2)return;
      const idx=list.findIndex(t=>t.path===active);
      const next=e.shiftKey?(idx-1+list.length)%list.length:(idx+1)%list.length;
      setActive(list[next].path);setTargetLine(undefined);return;
    }
    if(paletteRef.current)return;
    const dialogOpen=Array.from(document.querySelectorAll<HTMLElement>('[role="dialog"]')).some(node=>node.getClientRects().length>0);
    if(dialogOpen)return;
    if(!(e.ctrlKey||e.metaKey)||e.altKey)return;
    const target=e.target instanceof HTMLElement?e.target:null;
    if(target?.closest('.xterm'))return;
    const editable=!!target?.closest('input,textarea,select,[contenteditable="true"]')&&!target?.closest('.monaco-editor');
    const code=e.code;
    if(code==='KeyS'){e.preventDefault();e.stopPropagation();void actions.current.save();}
    else if(code==='KeyO'){e.preventDefault();void pickExternalFile();}
    else if(code==='KeyW'){if(editable)return;e.preventDefault();void actions.current.closeTab(actions.current.active);}
    else if(code==='KeyF'&&e.shiftKey){if(editable)return;e.preventDefault();setNavigation('search');setShowSidebar(true);}
    else if(code==='KeyB'&&!e.shiftKey){if(editable)return;e.preventDefault();setShowSidebar(v=>!v);}
    else if(code==='Backquote'){e.preventDefault();setShowTerminal(v=>!v);}
  }window.addEventListener('keydown',keys,true);return()=>window.removeEventListener('keydown',keys,true);},[]);
  function resize(e:ReactPointerEvent<HTMLDivElement>,panel:'explorer'|'agent'|'terminal'){
    if(e.button!==0)return;e.preventDefault();const el=e.currentTarget;const pointer=e.pointerId;el.setPointerCapture(pointer);const x=e.clientX,y=e.clientY,start=widths[panel];
    const move=(value:PointerEvent)=>{const next=start+(panel==='explorer'?value.clientX-x:panel==='agent'?x-value.clientX:y-value.clientY);const max=panel==='terminal'?window.innerHeight*0.55:window.innerWidth*0.35;setWidths(old=>({...old,[panel]:clamp(next,panel==='terminal'?120:200,max)}));};
    const end=()=>{el.removeEventListener('pointermove',move);el.removeEventListener('pointerup',end);el.removeEventListener('pointercancel',end);if(el.hasPointerCapture(pointer))el.releasePointerCapture(pointer);};
    el.addEventListener('pointermove',move);el.addEventListener('pointerup',end);el.addEventListener('pointercancel',end);
  }
  function separator(panel:'explorer'|'agent'|'terminal'){return <div className={`splitter ${panel==='terminal'?'horizontal':''}`} role="separator" aria-label={`Размер панели ${panel}`} aria-orientation={panel==='terminal'?'horizontal':'vertical'} aria-valuemin={panel==='terminal'?120:200} aria-valuemax={Math.round(panel==='terminal'?window.innerHeight*.55:window.innerWidth*.35)} aria-valuenow={widths[panel]} tabIndex={0} onPointerDown={e=>resize(e,panel)} onKeyDown={e=>{const step=16;const delta=panel==='terminal'?(e.key==='ArrowUp'?step:e.key==='ArrowDown'?-step:0):panel==='explorer'?(e.key==='ArrowRight'?step:e.key==='ArrowLeft'?-step:0):(e.key==='ArrowLeft'?step:e.key==='ArrowRight'?-step:0);if(delta){e.preventDefault();setWidths(old=>({...old,[panel]:clamp(old[panel]+delta,panel==='terminal'?120:200,panel==='terminal'?window.innerHeight*.55:window.innerWidth*.35)}));}}}/>;}
  const tab=tabs.find(item=>item.path===active);const second=tabs.find(item=>item.path===(secondary||active));
  const navigate=(next:Navigation)=>{if(next===navigation)setShowSidebar(v=>!v);else{setNavigation(next);setShowSidebar(true);}};
  const paletteCommands=buildCommands({openProject:()=>void openProject(),openFilePick:()=>void pickExternalFile(),save:()=>void save(),saveAsExternal:()=>void saveAsExternal(),hasExternal:!!tab?.external,closeTab:()=>void closeTab(active),hasActiveTab:!!tab,hasProject:!!project,toggleSidebar:()=>setShowSidebar(v=>!v),toggleTerminal:()=>setShowTerminal(v=>!v),toggleAgent:()=>setShowAgent(v=>!v),toggleSplit:()=>{setSplitEditor(v=>!v);if(!secondary)setSecondary(active);},navigate,openSettings:()=>setSettings(true),openQuickOpen:()=>setQuickOpen(true),hasDirty:!!tab&&isDirty(tab),toggleDiff:()=>setDiffView(v=>!v),zen:()=>{setShowSidebar(false);setShowAgent(false);setShowTerminal(false);},openRecentProject:path=>void openRecentProject(path),recentProjects,minimapOn,toggleMinimap});
  const menuGroups=buildMenuGroups(paletteCommands,()=>getCurrentWindow().close());
  return <div className="app-shell">
    {titlebarCustom&&<Titlebar groups={menuGroups} brand="NotCode Desktop"/>}
    <header className="app-header app-header-slim"><button className="project-switch" disabled={!nativeAvailable||opening} onClick={()=>void openProject()} title="Открыть папку как проект"><FolderOpen/><span>{project?.name||'Открыть проект'}</span><ChevronRight/></button><div className="header-spacer"/>
        {authProfile ? (
          <button className="user-info" title="Открыть настройки аккаунта" onClick={()=>setSettings(true)}>
            {authProfile.avatarUrl&&<img className="user-avatar" src={authProfile.avatarUrl} alt={authProfile.username} />}
            <span className="user-name">{authProfile.username}</span>
          </button>
        ) : (
          <div className="user-auth">
            <button className="button-small" onClick={() => void openAuthWindow().catch(report)}>Вход через GitHub</button>
          </div>
        )}
        <span className="local-badge"><ShieldCheck/>Локальная среда</span>
        <div className="toolbar"><button className="palette-button" title="Центр команд · Ctrl+K" onClick={()=>setPalette(true)}><Search/><span>Команды</span><kbd>Ctrl+K</kbd></button><button className="icon-button" title="Показать / скрыть проводник · Ctrl+B" onClick={()=>setShowSidebar(v=>!v)}><PanelLeftClose/></button><button className={`icon-button ${showTerminal?'active':''}`} title="Терминал · Ctrl+`" onClick={()=>setShowTerminal(v=>!v)}><TerminalSquare/></button><button className="icon-button" title="Показать / скрыть AI" onClick={()=>setShowAgent(v=>!v)}>{showAgent?<PanelRightClose/>:<PanelRightOpen/>}</button></div></header>
    {!nativeAvailable&&<div className="preview-banner">Просмотр интерфейса в браузере. Файлы, AI, Git, MCP и PowerShell доступны только в Tauri — <code>npm run desktop:dev</code>. Демонстрационных данных нет.</div>}
    {error&&<div className="error-banner" role="alert"><AlertCircle/><span>{error}</span><button className="icon-button" title="Закрыть ошибку" onClick={()=>setError('')}><X/></button></div>}
      <div className="workspace"><nav className="activity-rail" aria-label="Разделы IDE">{([{id:'files',label:'Проводник',Icon:Files},{id:'search',label:'Поиск · Ctrl+Shift+F',Icon:Search},{id:'github',label:'GitHub · репозитории и ветки',Icon:Github},{id:'browser',label:'Браузер · превью проекта',Icon:AppWindow},{id:'git',label:'Git',Icon:GitBranch},{id:'mcp',label:'MCP',Icon:Plug},{id:'devices',label:'Телефон и Desktop',Icon:Smartphone}] as const).map(({id,label,Icon})=><button key={id} className={`rail-button ${showSidebar&&navigation===id?'selected':''}`} aria-label={label} title={label} aria-pressed={showSidebar&&navigation===id} onClick={()=>navigate(id)}><Icon/></button>)}<div className="rail-spacer"/><button className="rail-button" title="Настройки" aria-label="Настройки" disabled={!nativeAvailable} onClick={()=>setSettings(true)}><SettingsIcon/></button></nav>
      {showSidebar&&<><div className="sidebar" style={{width:widths.explorer}}>{navigation==='files'||navigation==='search'?<Explorer projectId={project?.id} projectName={project?.name} refreshToken={workspaceRevision} mode={navigation} openFile={(path,line)=>void openFile(path,line)} report={report} openProject={()=>void openProject()} confirm={confirm}/>:navigation==='git'?<GitPanel projectId={project?.id} confirm={confirm} report={report}/>:navigation==='github'?<GitHubPanel projectId={project?.id} profile={authProfile} onCloned={project=>{void switchProject(project);}} confirm={confirm} report={report} openSettings={()=>setSettings(true)}/>:navigation==='browser'?<BrowserPanel report={report}/>:navigation==='devices'?<BridgePanel projectId={project?.id} projectName={project?.name} confirm={confirm} report={report}/>:<McpPanel projectId={project?.id} confirm={confirm} report={report}/>}</div>{separator('explorer')}</>}
      <main className="main-workspace"><section className="editor-workspace" aria-label="Редактор"><div className="editor-tabbar"><div role="tablist" className="file-tabs" aria-label="Открытые файлы">{tabs.map(item=><div key={item.path} className={`file-tab ${active===item.path?'selected':''}`} onContextMenu={event=>{event.preventDefault();setTabMenu({x:event.clientX,y:event.clientY,path:item.path});}}><button role="tab" aria-selected={active===item.path} title={item.path} onClick={()=>{setActive(item.path);setTargetLine(undefined);}}><Code2/><span>{basename(item.path)}</span>{isDirty(item)&&<span className="dirty-dot" aria-label="Не сохранён"/>}</button><button className="icon-button" title={`Закрыть ${basename(item.path)}`} onClick={()=>void closeTab(item.path)}><X/></button></div>)}</div><div className="toolbar"><button className="icon-button" title="Сохранить · Ctrl+S" disabled={!tab||saving||!isDirty(tab)} onClick={()=>void save()}><Save/></button><button className="icon-button" title="Сохранить как…" disabled={!tab?.external} onClick={()=>void saveAsExternal()}><SaveAll/></button><button className={`icon-button ${diffView?'active':''}`} title={diffView?'Скрыть сравнение':'Сравнить с сохранённым'} disabled={!tab||!isDirty(tab)} onClick={()=>setDiffView(v=>!v)}><GitCompare/></button><button className="icon-button" title="История файла · локальные версии" disabled={!tab} onClick={()=>{if(tab)void openHistory(tab.path);}}><History/></button><button className={`icon-button ${splitEditor?'active':''}`} title="Разделить редактор" disabled={!tab} onClick={()=>{setSplitEditor(v=>!v);if(!secondary)setSecondary(active);}}><Columns2/></button></div></div>
        {tab&&(project||tab.external)?<><div className="breadcrumbs">{tab.external?(<><span>Внешний файл</span><ChevronRight/><span>{tab.path}</span></>):(<><span>{project?.name}</span><ChevronRight/><span>{tab.path}</span></>)}</div><div className="editor-panes"><div className="editor-pane">{diffView&&isDirty(tab)?<DiffEditor original={tab.saved} modified={tab.content} language={languageFor(tab.path)} theme="notcode" loading={<p className="muted inset">Сравниваю с сохранённым…</p>} options={{readOnly:true,renderSideBySide:true,minimap:{enabled:false},automaticLayout:true,scrollBeyondLastLine:false,fontFamily:'Cascadia Code, Consolas, monospace',fontSize:13,lineHeight:22}}/>:<CodeEditor path={tab.path} projectId={tab.external?'ext':project?.id||'ext'} content={tab.content} change={content=>setTabs(old=>replaceTab(old,tab.path,content))} save={()=>void save()} line={targetLine} report={report} minimapOn={minimapOn}/>}</div>{splitEditor&&second&&<div className="editor-pane secondary-pane"><Dropdown className="secondary-file" label="Второй редактор" value={second.path} options={tabs.map(t=>({value:t.path,label:t.path}))} onChange={value=>setSecondary(value)}/><CodeEditor path={second.path} projectId={second.external?'ext':project?.id||'ext'} content={second.content} change={content=>setTabs(old=>replaceTab(old,second.path,content))} save={()=>{if(second.external){void api.externalFileWrite(second.path,second.content).then(()=>setTabs(old=>savedTab(old,second.path,second.content))).catch(report);}else{void api.write(second.path,second.content).then(()=>setTabs(old=>savedTab(old,second.path,second.content))).catch(report);}}} report={report}/></div>}</div></>:<Dashboard project={project} profile={authProfile} openProject={()=>void openProject()} openFilePick={()=>void pickExternalFile()} openRecentProject={path=>void openRecentProject(path)} recentFiles={recentFiles} openRecent={path=>void openExternal(path)} login={()=>void openAuthWindow().catch(report)} openSettings={()=>setSettings(true)} openAgent={()=>setShowAgent(true)} openTerminal={()=>setShowTerminal(true)} openDevices={()=>{setNavigation('devices');setShowSidebar(true);}} report={report}/>}
      </section><div style={{display:showTerminal?'contents':'none'}}>{separator('terminal')}<div className="terminal-container" style={{height:widths.terminal}}><TerminalPanel projectId={project?.id} confirm={confirm} report={report} hide={()=>setShowTerminal(false)}/></div></div></main>
      <div style={{display:showAgent?'contents':'none'}}>{separator('agent')}<div className="agent-container" style={{width:widths.agent}}><AgentPanel key={project?.id||'no-project'} projectId={project?.id} activePath={active||undefined} dirty={!!tab&&isDirty(tab)} report={report} confirm={confirm}/></div></div>
    </div><footer className="statusbar"><span className="status-dot online"/><span>{project?.name||'Нет открытого проекта'}</span><span className="status-message" role="status">{saving?'Сохранение…':notice}</span>{tab&&<><span>{isDirty(tab)?'Изменён':'Сохранён'}</span><span>UTF-8</span></>}<span>NotCode Desktop v{version}</span></footer>
    {settingsMounted&&<div style={{display:settings?'contents':'none'}}><Settings close={()=>setSettings(false)} report={report} minimapOn={minimapOn} onToggleMinimap={toggleMinimap} titlebarCustom={titlebarCustom} onToggleTitlebar={toggleTitlebar} petVisible={petVisible} onTogglePet={togglePet} projectId={project?.id} onProject={switchProject} onOpenMcp={()=>{setSettings(false);setNavigation('mcp');setShowSidebar(true);}} confirm={confirm}/></div>}{palette&&<CommandPalette commands={paletteCommands} close={()=>setPalette(false)}/>}{quickOpen&&<QuickOpen close={()=>setQuickOpen(false)} onOpen={path=>void openFile(path)} report={report}/>}{tabMenu&&<ContextMenuPortal payload={{x:tabMenu.x,y:tabMenu.y,items:tabMenuItems(),onSelect:id=>void runTabMenu(tabMenu.path,id)}}/>}{fileHistory&&<Modal title={`История · ${fileHistory.path}`} close={()=>setFileHistory(null)}><div className="form-stack">
  <p className="hint">Версии сохраняются локально при каждой перезаписи файла (до 50 последних). Восстановление тоже попадает в историю — его можно откатить.</p>
  {!fileHistory.entries.length&&<p className="hint">Версий ещё нет: они появятся после первого изменения файла.</p>}
  {fileHistory.entries.map(entry=><div key={entry.timestamp} className="history-row"><div className="repo-meta"><History/><div><strong>{new Date(entry.timestamp).toLocaleString('ru-RU')}</strong><small>{formatBytes(entry.size)}</small></div></div><button title="Сравнить с текущей" onClick={()=>void showVersionDiff(entry.timestamp)}><GitCompare/></button><button onClick={()=>void restoreHistory(entry.timestamp)}>Восстановить</button></div>)}
</div></Modal>}{historyDiffVersion&&tab&&<Modal title={`Версия от ${new Date(historyDiffVersion.timestamp).toLocaleString('ru-RU')} · ${fileHistory?.path ?? ''}`} close={()=>setHistoryDiffVersion(null)} wide><div style={{height:'56vh'}}><DiffEditor original={historyDiffContent} modified={tab.content} language={languageFor(tab.path)} theme="notcode" options={{readOnly:true,renderSideBySide:true,minimap:{enabled:false},automaticLayout:true}}/></div>
  <div className="dialog-actions"><button onClick={()=>setHistoryDiffVersion(null)}>Закрыть</button><button className="primary" onClick={()=>{ void restoreHistory(historyDiffVersion.timestamp); setHistoryDiffVersion(null); }}>Восстановить эту версию</button></div></Modal>}{confirmation&&<Modal title={confirmation.title} close={cancelConfirm}><p className="confirmation-detail">{confirmation.detail}</p>{confirmation.allowRemember&&<label className="remember-row"><input type="checkbox" checked={confirmationRemember} onChange={e=>setConfirmationRemember(e.target.checked)}/><span>{confirmation.scope?'Больше не спрашивать в этом проекте':'Больше не спрашивать'}</span></label>}<div className="dialog-actions"><button onClick={()=>settle(false)}>Отмена</button><button className="primary" onClick={()=>settle(true)}>Подтвердить</button></div></Modal>}
  {petVisible&&<PetOverlay visible={true} onHide={()=>{window.localStorage.setItem('notcode.pet.visible','off');setPetVisible(false);}}/>}
  </div>;
}

/// Плавающий питомец: рисуется поверх всех панелей внизу слева.
/// Позиция сохраняется в localStorage, перетаскивается мышью,
/// кнопка «×» прячет его до включения в настройках.
function PetOverlay({visible,onHide}:{visible:boolean;onHide:()=>void}) {
  const [pos,setPos]=useState(()=>{
    try{ const raw=window.localStorage.getItem('notcode.pet.pos'); return raw?JSON.parse(raw):{x:16,y:0}; }catch{return{x:16,y:0};}
  });
  const dragging=useRef(false);
  const offset=useRef({x:0,y:0});
  function onPointerDown(e:React.PointerEvent){
    dragging.current=true;
    offset.current={x:e.clientX-pos.x,y:e.clientY-pos.y};
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e:React.PointerEvent){
    if(!dragging.current)return;
    const next={x:Math.max(0,e.clientX-offset.current.x),y:Math.max(0,e.clientY-offset.current.y)};
    setPos(next);
    window.localStorage.setItem('notcode.pet.pos',JSON.stringify(next));
  }
  function onPointerUp(){ dragging.current=false; }
  if(!visible)return null;
  return <div className="pet-overlay" onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} style={{left:pos.x,top:pos.y}} title="Перетащите, чтобы передвинуть">
    <PetFloating/>
    <button className="pet-hide" title="Убрать питомца (включается в настройках)" onPointerDown={e=>e.stopPropagation()} onClick={onHide}><X/></button>
  </div>;
}

/// Мини-питомец для оверлея: визуал + огонёк серии + реакция на поглаживание.
function PetFloating() {
  const [streak,setStreak]=useState(0);
  const [love,setLove]=useState(false);
  const loveTimer=useRef<number|undefined>(undefined);
  useEffect(()=>{
    let live=true;
    const load=()=>{ void (async()=>{ try{ if(!nativeAvailable)return; const s=await api.activityStats(); if(live)setStreak(s.streak); }catch{} })(); };
    load();
    const timer=window.setInterval(load,60_000);
    return()=>{live=false;window.clearInterval(timer);};
  },[]);
  useEffect(()=>()=>window.clearTimeout(loveTimer.current),[]);
  function stroke(){
    setLove(true);
    window.clearTimeout(loveTimer.current);
    loveTimer.current=window.setTimeout(()=>setLove(false),1200);
  }
  const lit=streak>0;
  return <div className={`pet-mini ${love?'pet-love':''}`} onClick={stroke} title="Погладить питомца">
    {lit&&<div className="pet-flame" title={`Серия: ${streak} дн. подряд`}>🔥</div>}
    {love&&<div className="pet-heart">💙</div>}
    <svg viewBox="0 0 60 60" width={52} height={52} aria-label="Питомец NotCode">
      <line x1="30" y1="8" x2="30" y2="16" stroke={lit?'#c4f16b':'#555'} strokeWidth={2}/>
      <circle cx="30" cy="6" r={3.5} fill={lit?'#c4f16b':'#555'}/>
      <rect x="10" y="16" width="40" height="30" rx={10} fill="#202820" stroke={lit?'#c4f16b':'#666'} strokeWidth={1.5}/>
      <rect x="16" y="22" width="28" height="18" rx={5} fill="#101410"/>
      <circle cx="24" cy="30" r={3} fill={lit?'#c4f16b':'#666'}/>
      <circle cx="36" cy="30" r={3} fill={lit?'#c4f16b':'#666'}/>
      <path d="M26 35 Q30 38 34 35" stroke={lit?'#c4f16b':'#666'} strokeWidth={1.5} fill="none"/>
    </svg>
    {streak>0&&<div className="pet-streak" title="Дней подряд">{streak}</div>}
  </div>;
}
