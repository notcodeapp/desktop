import { MessageBody } from './MessageBody';
import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowUp, Square, Plus, Trash2, Paperclip, Bot, History, Mic, ChevronDown, ChevronRight, Wrench, Download, ListChecks, X, GripVertical } from 'lucide-react';
import { api, event, nativeAvailable } from '../lib/native';
import { adapters, agentCatalog, ollama } from '../lib/agents';
import { actionCountLabel, buildTimeline, mergeLiveAction } from '../lib/pure';
import { lookup as lookupModel } from '../model-catalog';
import type { AgentToolRequest, AuditEntry, LiveAction, Message, PlanStep, QueuedMessage, Session, VoiceStatus } from '../lib/types';
import { CliImportPanel } from './CliImportPanel';
import { ContextMenuPortal, type ContextMenuItem } from './ContextMenu';
import { Dropdown, Empty, SpinnerLabel, type Confirm, type Report } from './UI';
const ACTION_STATUS: Record<string,string> = { ok:'готово', error:'ошибка', denied:'отклонено', running:'выполняется', approval:'ожидает подтверждения' };

/// Карточка плана: агент ведёт чеклист шагов через инструмент update_plan.
/// В чат текстом не попадает — живёт отдельным блоком над сообщениями.
function PlanCard({ steps }: { steps: PlanStep[] }) {
  const [open, setOpen] = useState(true);
  const done = steps.filter(step => step.status === 'done').length;
  const finished = done === steps.length && steps.length > 0;
  return <div className={`plan-card ${finished ? 'finished' : ''}`}>
    <button type="button" className="plan-head" aria-expanded={open} onClick={() => setOpen(v => !v)}>
      {open ? <ChevronDown/> : <ChevronRight/>}
      <ListChecks/>
      <strong>План</strong>
      <span className="plan-progress">{done}/{steps.length}</span>
      {finished && <span className="plan-badge">выполнен</span>}
    </button>
    {open && <ol className="plan-steps">
      {steps.map((step, index) => <li key={`${index}-${step.title}`} className={`plan-step ${step.status}`}>
        <span className="plan-mark" aria-hidden>{step.status === 'done' ? '✓' : step.status === 'in_progress' ? '▶' : '·'}</span>
        <span className="plan-title">{step.title}</span>
      </li>)}
    </ol>}
  </div>;
}

/// Очередь сообщений: отправленные во время генерации, редактируются,
/// перетаскиваются (выше = раньше) и форсируются в отправку.
function QueueStrip({ items, editingId, setEditingId, onChange, onForce, onDelete }: {
  items: QueuedMessage[];
  editingId: string | null;
  setEditingId: (id: string | null) => void;
  onChange: (items: QueuedMessage[]) => void;
  onForce: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  const dragIndex = useRef<number | null>(null);
  if (!items.length) return null;
  function move(from: number, to: number) {
    if (to < 0 || to >= items.length || from === to) return;
    const next = [...items];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item);
    onChange(next);
  }
  return <div className="queue-strip" aria-label="Очередь сообщений">
    <div className="queue-title"><span>Очередь · {items.length}</span><small>выше — раньше отправится</small></div>
    {items.map((item, index) => <div
      key={item.id}
      className="queue-item"
      draggable={editingId !== item.id}
      onDragStart={e => { dragIndex.current = index; e.dataTransfer.effectAllowed = 'move'; }}
      onDragOver={e => e.preventDefault()}
      onDrop={e => { e.preventDefault(); if (dragIndex.current !== null) move(dragIndex.current, index); dragIndex.current = null; }}
    >
      <GripVertical className="queue-grip"/>
      {editingId === item.id
        ? <input
            className="queue-edit"
            autoFocus
            defaultValue={item.text}
            onBlur={e => { const value = e.target.value.trim(); onChange(items.map(old => old.id === item.id ? { ...old, text: value || old.text } : old)); setEditingId(null); }}
            onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); (e.target as HTMLInputElement).blur(); } if (e.key === 'Escape') setEditingId(null); }}
          />
        : <button type="button" className="queue-text" title="Изменить сообщение" onClick={() => setEditingId(item.id)}>{item.text}</button>}
      <button type="button" className="icon-button" title="Отправить сейчас (прервать текущую генерацию)" onClick={() => onForce(item.id)}><ArrowUp/></button>
      <button type="button" className="icon-button" title="Убрать из очереди" onClick={() => onDelete(item.id)}><X/></button>
    </div>)}
  </div>;
}
function prettyJson(value: string): string {
  if (!value) return '';
  try { return JSON.stringify(JSON.parse(value), null, 2); } catch { return value; }
}
function ActionDetails({ name, status, detail, result }: { name: string; status: string; detail?: string; result?: string }) {
  return <div className={`action-item ${status}`}>
    <div className="action-head"><span className="action-name">{name}</span><span className={`action-status ${status}`}>{ACTION_STATUS[status] || status}</span></div>
    {detail && <pre className="action-detail">{prettyJson(detail)}</pre>}
    {result && <pre className="action-detail result">{prettyJson(result)}</pre>}
  </div>;
}
function ActionGroup({ entries }: { entries: AuditEntry[] }) {
  const [open, setOpen] = useState(false);
  return <div className="action-group"><button type="button" className="action-toggle" aria-expanded={open} onClick={() => setOpen(v => !v)}>{open ? <ChevronDown/> : <ChevronRight/>}<Wrench/><span>{actionCountLabel(entries.length)}</span></button>
    {open && <div className="action-list">{entries.map(entry => <ActionDetails key={entry.id} name={entry.name} status={entry.status} detail={entry.arguments} result={entry.result}/>)}</div>}
  </div>;
}
function LiveActionGroup({ actions }: { actions: LiveAction[] }) {
  const [open, setOpen] = useState(false);
  return <div className="action-group live"><button type="button" className="action-toggle" aria-expanded={open} onClick={() => setOpen(v => !v)}>{open ? <ChevronDown/> : <ChevronRight/>}<Wrench/><span>{actionCountLabel(actions.length)}</span><span className="busy-dot"/></button>
    {open && <div className="action-list">{actions.map((action, index) => <ActionDetails key={`${action.name}-${index}`} name={action.name} status={action.status} detail={action.detail}/>)}</div>}
  </div>;
}
export function AgentPanel({projectId, activePath, dirty, report, confirm}: {projectId?: string; activePath?: string; dirty: boolean; report: Report; confirm: Confirm}) {
  const [provider,setProvider] = useState('compatible');
  const adapter = adapters[provider] ?? ollama;
  // OpenCode Zen теперь HTTP-провайдер (шлюз opencode.ai), внешние CLI — Codex и Claude.
  const external = provider === 'codex' || provider === 'claude';
  const [catalogRevision, setCatalogRevision] = useState(0);
  useEffect(() => {
    const changed = (e: Event) => {
      if ((e as CustomEvent<{provider?: string}>).detail?.provider === provider) setCatalogRevision(value => value + 1);
    };
    window.addEventListener('native:provider-changed', changed);
    return () => window.removeEventListener('native:provider-changed', changed);
  }, [provider]);
  const [models,setModels] = useState<string[]>([]); const [model,setModel] = useState('');
  const [sessions,setSessions] = useState<Session[]>([]); const [selected,setSelected] = useState(''); const selectedRef = useRef('');
  const [messages,setMessages] = useState<Message[]>([]); const [audit,setAudit] = useState<AuditEntry[]>([]); const [draft,setDraft] = useState(''); const [attach,setAttach] = useState(false);
  const [streams,setStreams] = useState<Record<string,string>>({}); const [running,setRunning] = useState<Set<string>>(new Set());
  const [generationStatus,setGenerationStatus] = useState<Record<string,string>>({});
  const [liveActions,setLiveActions] = useState<Record<string,LiveAction[]>>({}); const [busy,setBusy] = useState(false); const [connected,setConnected] = useState(false);
  const [plans,setPlans] = useState<Record<string,PlanStep[]>>({});
  const [queue,setQueue] = useState<QueuedMessage[]>([]); const queueRef = useRef<QueuedMessage[]>([]); queueRef.current = queue;
  const [queueEditing,setQueueEditing] = useState<string|null>(null);
  const sendingRef = useRef(false);
  useEffect(() => {
    if (!nativeAvailable) return;
    let live = true;
    const stopPromise = event<{sessionId:string;steps:PlanStep[]}>('agent:plan', payload => {
      if (live) setPlans(old => ({ ...old, [payload.sessionId]: payload.steps }));
    });
    return () => { live = false; void stopPromise.then(fn => fn()); };
  }, []);
  const [connectError,setConnectError] = useState('');
  const [history,setHistory] = useState(false); const [ready,setReady] = useState(false); const [toolReady,setToolReady] = useState(false);
  const [voice,setVoice] = useState<VoiceStatus|null>(null); const [recording,setRecording] = useState(false); const recordingRef = useRef(false); recordingRef.current = recording;
  const bottom = useRef<HTMLDivElement>(null); const loadSeq = useRef(0); const approvalQueue = useRef(Promise.resolve()); const stick = useRef(true);
  const activePathRef=useRef(activePath);activePathRef.current=activePath;const dirtyRef=useRef(dirty);dirtyRef.current=dirty;
  const select = useCallback((id:string) => { stick.current=true; selectedRef.current=id; setSelected(id); },[]);
  const loadSessions = useCallback(async () => { if (projectId) setSessions(await api.sessions()); },[projectId]);
  const loadChat = useCallback(async (id: string) => {
    const [chat, entries] = await Promise.all([api.messages(id), api.sessionAudit(id).catch(() => [] as AuditEntry[])]);
    if (selectedRef.current === id) { setMessages(chat); setAudit(entries); }
    void api.sessionPlan(id).then(steps => { if (selectedRef.current === id) setPlans(old => ({ ...old, [id]: steps })); }).catch(() => {});
  }, []);
  useEffect(() => { let live=true; let stop: (() => void) | undefined;
    ollama.getEvents(e => {
      if (!live) return;
      if (e.type==='delta'||e.type==='status'||e.type==='action') setRunning(old => new Set(old).add(e.sessionId));
      if (e.type==='delta') setStreams(old => ({...old,[e.sessionId]:(old[e.sessionId]||'')+e.text}));
      else if (e.type==='action') {
        try {
          const payload = JSON.parse(e.text) as Partial<LiveAction>;
          if (payload && typeof payload.name==='string') {
            const action: LiveAction = {name:payload.name,status:String(payload.status??''),detail:String(payload.detail??'')};
            setLiveActions(old => ({...old,[e.sessionId]:mergeLiveAction(old[e.sessionId]||[],action)}));
            // The pet reacts to real command outcomes, not to tool noise.
            if (action.name==='run_command'&&(action.status==='ok'||action.status==='error')) {
              window.dispatchEvent(new CustomEvent('notcode:pet-command',{detail:{ok:action.status==='ok'}}));
            }
          }
        } catch { /* malformed action payloads are ignored */ }
      }
      else if (e.type==='message') {
        setStreams(old => ({...old,[e.sessionId]:''}));
        setLiveActions(old => {const next={...old};delete next[e.sessionId];return next;});
        if (selectedRef.current===e.sessionId) void loadChat(e.sessionId).catch(report);
      }
      else {
        if(e.type==='status'||e.type==='tool')setGenerationStatus(old=>({...old,[e.sessionId]:e.text}));
        if (e.type==='done'||e.type==='error') {
          setGenerationStatus(old=>{const next={...old};delete next[e.sessionId];return next;});
          setRunning(old => {const next=new Set(old); next.delete(e.sessionId); return next;});
          setStreams(old => {const next={...old};delete next[e.sessionId];return next;});
          setLiveActions(old => {const next={...old};delete next[e.sessionId];return next;});
          if(e.type==='error') report(e.text);
          if(selectedRef.current===e.sessionId) void loadChat(e.sessionId).catch(report);
          void loadSessions().catch(report);
        }
      }
    }).then(unlisten => { if(!live) unlisten(); else {stop=unlisten;setReady(true);} }).catch(report);
    return () => {live=false;stop?.();};
  },[report,loadSessions,loadChat]);
  useEffect(() => {
    if(!nativeAvailable)return;
    let live=true;let stop:(()=>void)|undefined;
    event<AgentToolRequest>('agent:tool-request',request=>{
      approvalQueue.current=approvalQueue.current.then(async()=>{
        if(!live){await api.agentToolRespond(request.requestId,false).catch(report);return;}
        const args=request.arguments as {path?:unknown;contentPreview?:unknown;contentBytes?:unknown;command?:unknown;cwd?:unknown;timeoutSeconds?:unknown;agent?:unknown;action?:unknown;paths?:unknown;reason?:unknown;detail?:unknown};
        const path=typeof args.path==='string'?args.path:'(неизвестный путь)';
        const preview=typeof args.contentPreview==='string'?args.contentPreview:'';
        const cliAgent=typeof args.agent==='string'?args.agent:'внешний агент';
        const cliPaths=Array.isArray(args.paths)?args.paths.filter((p):p is string=>typeof p==='string'&&!!p):[];
        const cliDetail=typeof args.detail==='string'?args.detail:'';
        const cliReason=typeof args.reason==='string'?args.reason:'';
        const title=request.name==='cli_action'
          ?(args.action==='command'?`Разрешить ${cliAgent}: запуск команды?`:args.action==='edit'?`Разрешить ${cliAgent}: изменение файлов?`:`Разрешить ${cliAgent}: доступ?`)
          :request.name==='run_command'?'Разрешить запуск команды?':request.name==='write_file'?'Разрешить запись файла?':request.name==='create_file'?'Разрешить создать файл?':'Разрешить создать папку?';
        const detail=request.name==='cli_action'
          ?`${args.action==='command'?(typeof args.command==='string'&&args.command?args.command:'(команда не указана)'):cliPaths.length?cliPaths.join('\n'):'(пути не указаны)'}${typeof args.cwd==='string'&&args.cwd?`\n\nПапка: ${args.cwd}`:''}${cliReason?`\n\n${cliReason}`:''}${cliDetail?`\n\n${cliDetail}${cliDetail.length>=1200?'\n…':''}`:''}\n\n${cliAgent} — отдельный процесс с вашими правами, вне песочницы NotCode. Он сам решает, какие файлы читать и менять; подтверждение получает только то, о чём спросит.${dirtyRef.current?'\n\nВ редакторе есть несохранённые изменения. Сохраните их, чтобы не потерять свою версию.':''}`
          :request.name==='run_command'
          ?`${typeof args.command==='string'?args.command:'(пустая команда)'}\n\nПапка: ${typeof args.cwd==='string'&&args.cwd?args.cwd:'корень проекта'}\nТаймаут: ${typeof args.timeoutSeconds==='number'?args.timeoutSeconds:120} с\n\nКоманда выполняется с вашими правами, без песочницы, и не может запрашивать ввод. Остановка генерации убивает оболочку, но не обязательно её дочерние процессы.`
          :request.name==='write_file'
          ?`${path}\nРазмер: ${typeof args.contentBytes==='number'?args.contentBytes:0} байт\n\n${preview}${preview.length>=1200?'\n…':''}${dirtyRef.current&&path===activePathRef.current?'\n\nВ редакторе есть несохранённые изменения. Сначала сохраните их, если хотите оставить обе версии.':''}`
          :`${path}${request.name==='create_file'?'\n\nБудет создан пустой файл. После этого агент запишет в него содержимое.':''}`;
        let approved=false;
        try{approved=await confirm(title,detail,false,projectId);}catch(error){report(error);}
        try{await api.agentToolRespond(request.requestId,approved);}catch(error){report(error);}
      }).catch(report);
    }).then(unlisten=>{if(!live)unlisten();else{stop=unlisten;setToolReady(true);}}).catch(report);
    return()=>{live=false;stop?.();setToolReady(false);};
  },[confirm,report,projectId]);
  useEffect(() => {select('');setMessages([]);setAudit([]);setRunning(new Set());setStreams({});setLiveActions({});setAttach(false);setDraft(''); void loadSessions().catch(report);},[projectId,loadSessions,report,select]);
  useEffect(() => {if(stick.current)bottom.current?.scrollIntoView({block:'end'});},[messages,audit,streams,liveActions,selected]);
  useEffect(()=>{if(!nativeAvailable)return;let live=true;setConnected(false);setConnectError('');void adapter.connect().then(list=>{if(live){setModels(list);setModel(old=>list.includes(old)?old:list[0]||'');setConnected(true);}}).catch(error=>{if(live){setModels([]);setModel('');setConnectError(error instanceof Error?error.message:String(error));}});return()=>{live=false;};},[adapter,catalogRevision]);
  useEffect(() => {
    if (!nativeAvailable) return;
    let live = true; const stops: (() => void)[] = [];
    void api.voiceStatus().then(value => { if (live) setVoice(value); }).catch(() => {});
    const track = (promise: Promise<() => void>) => { void promise.then(fn => { if (live) stops.push(fn); else fn(); }).catch(() => {}); };
    track(event<{text:string}>('voice:ready', () => { if (live) setRecording(true); }));
    track(event<{text:string}>('voice:text', payload => { if (live && payload.text) setDraft(old => old ? `${old.trimEnd()} ${payload.text}` : payload.text); }));
    track(event<{message:string}>('voice:error', payload => { if (live) { setRecording(false); report(payload.message); } }));
    track(event<{text:string}>('voice:stopped', () => { if (live) setRecording(false); }));
    return () => { live = false; stops.forEach(fn => fn()); if (recordingRef.current) void api.voiceStop().catch(() => {}); };
  }, [report]);
  async function toggleVoice() {
    if (recording) { await api.voiceStop().catch(report); setRecording(false); return; }
    try { await api.voiceStart(window.localStorage.getItem('notcode.voice.language') || 'ru-RU'); } catch (error) { report(error); }
  }
  async function choose(id:string) {const seq=++loadSeq.current;select(id);setMessages([]);setAudit([]);setHistory(false);setQueue([]);setQueueEditing(null);const session=sessions.find(s=>s.id===id);if(session)setProvider(session.provider);if(!id)return;try{await loadChat(id);if(seq!==loadSeq.current)return;}catch(e){report(e);}}
  function searchChats(query:string) {
    setChatSearch(query);
    if (!projectId || query.trim().length < 2) { setChatHits(null); return; }
    void api.chatsSearch(query.trim()).then(hits => setChatHits(hits)).catch(() => setChatHits(null));
  }
  async function imported(session: Session) {
    const sequence = ++loadSeq.current;
    select(session.id); setProvider(session.provider); setModel(session.model); setHistory(false); setMessages([]); setAudit([]);
    setSessions(old => [session, ...old.filter(value => value.id !== session.id)]);
    try {
      await loadChat(session.id);
      if (sequence !== loadSeq.current) return;
      await loadSessions();
    } catch (error) { report(error); }
  }
  async function remove(session:Session) {if(!await confirm('Удалить диалог?',`«${session.title}» и его сообщения будут удалены из локальной истории.`))return;try{await api.deleteSession(session.id);if(selected===session.id){select('');setMessages([]);setAudit([]);}await loadSessions();}catch(e){report(e);}}
  async function share(session:Session) {
    if (!await confirm('Опубликовать диалог?', `Создаётся публичная ссылка вида …/s/<id>: её сможет открыть любой, у кого есть ссылка. Ссылка действует 90 дней, не индексируется поисковиками, отменить можно в любой момент.`, false)) return;
    try {
      const result = await api.shareChat(session.id);
      await navigator.clipboard.writeText(result.url).catch(() => {});
      setForkNote(`Ссылка скопирована: ${result.url}`);
      window.setTimeout(()=>setForkNote(''),8000);
    } catch(e) { report(e); }
  }
  async function fork(session:Session) {
    try {
      const copy = await api.forkSession(session.id);
      await loadSessions();
      void imported(copy);
      setForkNote(`Скопировано: ${copy.title}`);
      window.setTimeout(()=>setForkNote(''),4000);
    } catch(e) { report(e); }
  }
  const [exported,setExported]=useState('');
  const [forkNote,setForkNote]=useState('');
  const [chatSearch,setChatSearch]=useState('');
  const [chatHits,setChatHits]=useState<{sessionId:string;sessionTitle:string;snippet:string}[]|null>(null);
  const [sessionMenu,setSessionMenu]=useState<{x:number;y:number;id:string}|null>(null);
  /// Сохраняет диалог в .notcode/exports как Markdown: заголовок, участники,
  /// все сообщения по порядку. Папки создаёт под проект (user-initiated).
  async function exportMarkdown(session:Session) {
    if(!projectId)return;
    setExported('…');
    try {
      const chat=await api.messages(session.id);
      const parts:string[]=[`# ${session.title}`,'',`- Провайдер: \`${session.provider}\``,`- Модель: \`${session.model}\``,`- Экспортирован: ${new Date().toLocaleString('ru-RU')}`,''];
      for(const message of chat){
        const who=message.role==='user'?'Вы':'Агент';
        parts.push(`## ${who}`,'',message.content.trim().replace(/\r\n/g,'\n'),'');
      }
      const safe=session.title.replace(/[^\p{L}\p{N}_-]+/gu,'-').replace(/^-+|-+$/g,'').slice(0,60)||'dialog';
      const dir='.notcode/exports';
      await api.ensureDir('.notcode');
      await api.ensureDir(dir);
      const path=`${dir}/${safe}-${session.id.slice(0,8)}.md`;
      await api.write(path,parts.join('\n'));
      setExported(path);
      window.setTimeout(()=>setExported(''),4000);
    } catch(e) { setExported(''); report(e); }
  }
  async function send() {
    if(!draft.trim()||!model||!projectId||!ready||!toolReady||busy)return;
    // Агент занят — сообщение уходит в очередь и отправится автоматически.
    if(running.has(selected)){enqueue(draft.trim());setDraft('');return;}
    await dispatchMessage(draft.trim());
    setDraft('');
  }
  function enqueue(text:string){setQueue(old=>[...old,{id:`q${Date.now()}-${Math.random().toString(36).slice(2,7)}`,text}]);}
  /// Общий конвейер отправки: создаёт сессию при необходимости, ставит running,
  /// дожидается ответа. Используется и обычной отправкой, и очередью.
  async function dispatchMessage(content:string) {
    if(!model||!projectId)return;
    stick.current=true;setBusy(true);
    try {
      let id=selected;
      if(!id){const session=await adapter.startSession(model);id=session.id;select(id);setSessions(old=>[session,...old]);setMessages([]);setAudit([]);}
      const context=attach&&activePath?activePath:null;
      setStreams(old=>({...old,[id]:''}));
      setGenerationStatus(old=>({...old,[id]:`Подключаюсь к ${adapter.label}…`}));
      setRunning(old=>new Set(old).add(id));
      try {await adapter.sendMessage(id,content,context);}catch(e){setRunning(old=>{const next=new Set(old);next.delete(id);return next;});throw e;}
      await loadChat(id);
      await loadSessions();
    }catch(e){report(e);}finally{setBusy(false);}
  }
  /// Пока идёт генерация, очередь сама продолжает диалог: как только running
  /// для выбранной сессии опустел — отправляем следующий из очереди.
  useEffect(() => {
    const id = selected;
    if (!id || running.has(id) || busy || sendingRef.current) return;
    const next = queueRef.current[0];
    if (!next || !model) return;
    sendingRef.current = true;
    setQueue(old => old.filter(item => item.id !== next.id));
    void dispatchMessage(next.text).finally(() => { sendingRef.current = false; });
  }, [running, selected, busy, model]);
  /// Принудительная отправка: сообщение в начало очереди + прерывание текущей
  /// генерации; авто-эффект отправит его сразу после освобождения.
  function forceSend(id:string) {
    setQueue(old => { const item=old.find(x=>x.id===id); if(!item)return old; return [item,...old.filter(x=>x.id!==id)]; });
    if (running.has(selected)) void adapter.interrupt(selected).catch(report);
  }
  const active = sessions.find(s=>s.id===selected); const generating=running.has(selected);
  const canSend=!!draft.trim()&&!!model&&!!projectId&&connected&&ready&&toolReady&&!busy&&!generating;
  const canQueue=!!draft.trim()&&!!model&&!!projectId&&!busy&&generating;
  const visibleMessages = generating&&messages.at(-1)?.role==='assistant'?messages.slice(0,-1):messages;
  const timeline = buildTimeline(visibleMessages, audit);
  const modelOptions = [...new Set([...models,...(active?[active.model]:[])])].map(name=>{
    const meta = lookupModel(name);
    return { value: name, label: name, hint: meta ? `контекст ~${Math.round(meta.ctx/1024)}K${meta.free ? ' · бесплатно' : ''}` : undefined };
  });
  return <aside className="agent-panel panel" aria-label="AI-агент"><div className="panel-heading"><span><Bot/> AI-агент</span><div className="toolbar"><button className="icon-button" title="История диалогов" onClick={()=>setHistory(v=>!v)} disabled={!projectId}><History/></button><button className="icon-button" title="Новый диалог" onClick={()=>void choose('')} disabled={busy}><Plus/></button></div></div>
    {connectError&&messages.length>0&&<p className="warning connect-warning" role="alert">Не удалось подключиться: {connectError}</p>}
    {plans[selected]?.length?<PlanCard steps={plans[selected]}/>:null}
    {history?<div className="scroll-area session-list"><CliImportPanel projectId={projectId} onImported={session => void imported(session)} confirm={confirm} report={report}/><h3>Диалоги проекта</h3>{forkNote&&<p className="hint" role="status">{forkNote}</p>}
      <input className="chat-search" placeholder="Поиск по чатам проекта…" value={chatSearch} onChange={e=>searchChats(e.target.value)} aria-label="Поиск по чатам" spellCheck={false}/>
      {chatHits&&<div className="chat-hits">{!chatHits.length&&<p className="hint">Совпадений нет.</p>}{chatHits.map(hit=><button key={hit.sessionId} className="recent-file" onClick={()=>void choose(hit.sessionId)}><strong>{hit.sessionTitle}</strong><small>{hit.snippet}</small></button>)}</div>}{!sessions.length&&<Empty title="История пуста"/>}{sessions.map(s=><div key={s.id} className={`session-row ${selected===s.id?'selected':''}`} onContextMenu={event=>{event.preventDefault();setSessionMenu({x:event.clientX,y:event.clientY,id:s.id});}}><button onClick={()=>void choose(s.id)}><strong>{s.title}</strong><small>{s.model} {running.has(s.id)?'· генерация…':''}</small></button><button className="icon-button" disabled={running.has(s.id)} title={exported&&exported!=='…'?`Сохранено: ${exported}`:'Экспорт диалога в Markdown (.notcode/exports)'} onClick={()=>void exportMarkdown(s)}><Download/></button><button className="icon-button" disabled={running.has(s.id)} title="Удалить диалог" onClick={()=>void remove(s)}><Trash2/></button></div>)}
    <ContextMenuPortal payload={sessionMenu?{x:sessionMenu.x,y:sessionMenu.y,items:[
      {id:'export',label:'Экспорт в Markdown',hint:'.md'},
      {id:'fork',label:'Скопировать диалог',hint:'полная копия в новый чат'},
      {id:'share',label:'Поделиться ссылкой',hint:'публично · 90 дней'},
      {id:'delete',label:'Удалить диалог',danger:true,disabled:running.has(sessionMenu.id)},
    ] as ContextMenuItem[],onSelect:id=>{const session=sessions.find(x=>x.id===sessionMenu?.id);setSessionMenu(null);if(!session)return;if(id==='export')void exportMarkdown(session);else if(id==='fork')void fork(session);else if(id==='share')void share(session);else if(id==='delete')void remove(session);}}:null}/></div>:<div className="messages scroll-area" aria-live="polite" onScroll={e=>{const el=e.currentTarget;stick.current=el.scrollHeight-el.scrollTop-el.clientHeight<80;}}>
      {!messages.length&&!generating&&<Empty title={projectId?'Обсудите ваш код':'Сначала откройте проект'}><p>{connected?'Создайте диалог. При необходимости прикрепите текущий файл.':connectError||'Укажите API и модели в настройках. Выбор модели находится в поле сообщения.'}</p><p className="hint">{external?`${adapter.label} работает как отдельный процесс с вашими правами и использует ваш собственный вход в CLI. История диалога продолжается через его сессию. NotCode показывает запросы на команды, изменение файлов и дополнительный доступ — подтверждайте только то, что понимаете.`:'Агент может сам просматривать папки, читать и искать файлы, а также запускать команды сборки и тестов. Перед созданием и изменением файлов и перед каждой командой он запросит подтверждение.'}</p></Empty>}
      {timeline.map(group=>group.kind==='message'?<article className={`message ${group.message.role}`} key={group.message.id}><div className="message-label">{group.message.role==='user'?'Вы':group.message.role==='assistant'?adapter.label:'Система'}</div><div className="message-content">{group.message.role==='assistant'?<MessageBody content={group.message.content}/>:group.message.content||'Ответ не содержит текста.'}</div></article>:<ActionGroup key={`actions-${group.entries[0].id}`} entries={group.entries}/>)}
      {generating&&<>{!!liveActions[selected]?.length&&<LiveActionGroup actions={liveActions[selected]}/>}<article className="message assistant"><div className="message-label"><SpinnerLabel>{generationStatus[selected]||`${adapter.label} отвечает`}</SpinnerLabel></div><div className="message-content"><MessageBody content={streams[selected]||''} streaming/></div></article></>}
      <div ref={bottom}/>
    </div>}
    <form className="composer" onSubmit={e=>{e.preventDefault();void send();}}>
      <QueueStrip items={queue} editingId={queueEditing} setEditingId={setQueueEditing} onChange={setQueue} onForce={forceSend} onDelete={id=>setQueue(old=>old.filter(item=>item.id!==id))}/>
      {attach&&activePath&&<div className="context-chip"><Paperclip/><span title={activePath}>{activePath}</span><button type="button" onClick={()=>setAttach(false)} title="Убрать контекст">×</button></div>}
      {attach&&dirty&&<p className="hint warning">Контекст берётся с диска. Сохраните изменения перед отправкой.</p>}
      <label className="sr-only" htmlFor="prompt">Сообщение агенту</label><textarea id="prompt" value={draft} onChange={e=>setDraft(e.target.value)} placeholder={recording?'Говорите — текст появится здесь…':generating?'Агент работает — Ctrl+Enter поставит сообщение в очередь…':'Спросите о коде…'} disabled={!projectId} onKeyDown={e=>{if(e.key==='Enter'&&(e.ctrlKey||e.metaKey)&&!e.nativeEvent.isComposing){e.preventDefault();void send();}}}/>
      <div className="composer-footer"><button type="button" className={`icon-button ${attach?'active':''}`} title="Прикрепить текущий сохранённый файл" disabled={!activePath} onClick={()=>setAttach(v=>!v)}><Paperclip/></button>{voice?.available&&<button type="button" className={`icon-button mic-button ${recording?'recording':''}`} title={recording?'Остановить запись':'Голосовой ввод'} aria-pressed={recording} disabled={!projectId} onClick={()=>void toggleVoice()}><Mic/></button>}<Dropdown className="composer-provider" label="Провайдер" disabled={busy||generating} value={provider} options={agentCatalog.map(a=>({value:a.id,label:a.label}))} onChange={value=>{if(selected){select('');setMessages([]);setAudit([]);}setProvider(value);setModel('');}}/><Dropdown className="composer-model" label="Модель" placeholder="Выберите модель" disabled={busy||generating} value={active?.model||model} options={modelOptions} onChange={value=>{if(selected){select('');setMessages([]);setAudit([]);}setModel(value);}}/>{generating?<>
        <button type="button" className="send-button icon-button" title="Поставить в очередь" disabled={!canQueue} onClick={()=>{enqueue(draft.trim());setDraft('');}}><ArrowUp/></button>
        <button type="button" className="icon-button stop" title="Остановить генерацию" onClick={()=>void adapter.interrupt(selected).catch(report)}><Square/></button>
      </>:<button className="send-button icon-button" title="Отправить" disabled={!canSend}><ArrowUp/></button>}</div>
    </form><div className="agent-footnote">{external?`Локальная история · ${adapter.label} отправляет код и запросы своему серверу`:'Локальная история · API отправляет сообщения выбранному серверу'}</div>
  </aside>;
}
