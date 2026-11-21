import { useEffect, useRef, useState } from 'react';
import { Terminal as XTerminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { Plus, X, TerminalSquare } from 'lucide-react';
import { api, event } from '../lib/native';
import type { TerminalInfo } from '../lib/types';
import tokens from '../../design-system/tokens.json';
import { THEME_EVENT, xtermTheme } from '../lib/theme';
import { Empty, type Confirm, type Report } from './UI';
type Runtime = { term: XTerminal; fit: FitAddon; observer: ResizeObserver };
function TerminalView({info, active, buffers, runtimes, report}: {info:TerminalInfo; active:boolean; buffers:React.RefObject<Map<string,string>>; runtimes:React.RefObject<Map<string,Runtime>>; report:Report}) {
  const ref=useRef<HTMLDivElement>(null);
  useEffect(()=>{
    if(!ref.current)return;
    const term=new XTerminal({fontFamily:tokens.typography.code,fontSize:12,scrollback:4000,cursorBlink:true,allowProposedApi:false,theme:xtermTheme()});
    const fit=new FitAddon();term.loadAddon(fit);term.open(ref.current);
    const retheme=()=>{term.options.theme=xtermTheme();};
    window.addEventListener(THEME_EVENT,retheme);
    function resize(){if(ref.current&&ref.current.clientWidth>0&&ref.current.clientHeight>0){fit.fit();void api.terminalResize(info.id,term.cols,term.rows).catch(report);}}
    const observer=new ResizeObserver(resize);observer.observe(ref.current);
    runtimes.current.set(info.id,{term,fit,observer});
    const pending=buffers.current.get(info.id);if(pending){term.write(pending);buffers.current.delete(info.id);}
    const input=term.onData(data=>{void api.terminalWrite(info.id,data).catch(report);});
    resize();return()=>{input.dispose();window.removeEventListener(THEME_EVENT,retheme);observer.disconnect();runtimes.current.delete(info.id);term.dispose();};
  },[info.id,buffers,runtimes,report]);
  useEffect(()=>{if(active){requestAnimationFrame(()=>{const rt=runtimes.current.get(info.id);if(rt&&ref.current?.clientWidth){rt.fit.fit();void api.terminalResize(info.id,rt.term.cols,rt.term.rows).catch(report);rt.term.focus();}});}},[active,info.id,runtimes,report]);
  return <div className="terminal-view" style={{display:active?'block':'none'}} ref={ref} aria-label={`${info.shell} ${info.id}`}/>;
}
export function TerminalPanel({projectId,confirm,report,hide}:{projectId?:string;confirm:Confirm;report:Report;hide:()=>void}) {
  const [tabs,setTabs]=useState<TerminalInfo[]>([]);const [active,setActive]=useState('');const [ready,setReady]=useState(false);const [busy,setBusy]=useState(false);
  const [exited,setExited]=useState<Set<string>>(new Set());
  const buffers=useRef(new Map<string,string>());const runtimes=useRef(new Map<string,Runtime>());
  useEffect(()=>{let live=true;const unlisteners:(()=>void)[]=[];
    Promise.all([
      event<{id:string;data:string}>('terminal:data',e=>{const rt=runtimes.current.get(e.id);if(rt)rt.term.write(e.data);else buffers.current.set(e.id,((buffers.current.get(e.id)||'')+e.data).slice(-1024*1024));}),
      event<{id:string;code:number|null}>('terminal:exit',e=>{setExited(old=>new Set(old).add(e.id));runtimes.current.get(e.id)?.term.writeln(`\r\n[Процесс завершён: ${e.code??'unknown'}]`);}),
    ]).then(list=>{if(live){unlisteners.push(...list);setReady(true);}else list.forEach(fn=>fn());}).catch(report);
    return()=>{live=false;unlisteners.forEach(fn=>fn());};
  },[report]);
  async function create(){if(!await confirm('Разрешить запуск PowerShell?',`Терминал запустит локальную оболочку в папке проекта. Введённые команды выполняются с правами вашей учётной записи и не ограничены папкой проекта. AI не имеет доступа к терминалу.`))return;setBusy(true);try{const info=await api.terminalCreate(true);setTabs(old=>[...old,info]);setActive(info.id);}catch(e){report(e);}finally{setBusy(false);}}
  async function close(info:TerminalInfo){if(!exited.has(info.id)&&!await confirm('Закрыть терминал?',`Будет остановлена оболочка ${info.shell}. Несохранённая работа запущенных в ней программ может быть потеряна.`))return;try{await api.terminalClose(info.id);setTabs(old=>{const next=old.filter(t=>t.id!==info.id);if(active===info.id)setActive(next.at(-1)?.id||'');return next;});buffers.current.delete(info.id);}catch(e){report(e);}}
  return <section className="terminal-panel"><div className="panel-heading"><span><TerminalSquare/> Терминал</span><div className="terminal-tabs" role="tablist" aria-label="Терминалы">{tabs.map((tab,i)=><div className={`terminal-tab ${active===tab.id?'selected':''}`} key={tab.id}><button role="tab" aria-selected={active===tab.id} onClick={()=>setActive(tab.id)}>{tab.shell} {i+1}{exited.has(tab.id)?' · завершён':''}</button><button className="icon-button" title="Закрыть терминал" onClick={()=>void close(tab)}><X/></button></div>)}</div><div className="toolbar"><button className="icon-button" title="Новый терминал" disabled={!projectId||!ready||busy} onClick={()=>void create()}><Plus/></button><button className="icon-button" title="Скрыть панель, не останавливая процессы" onClick={hide}><X/></button></div></div>
    <div className="terminal-body">{!tabs.length?<Empty title="Терминал не запущен"><p>PowerShell в папке проекта. Запуск — только с вашего разрешения.</p><button disabled={!projectId||!ready||busy} onClick={()=>void create()}><Plus/>Новый терминал</button></Empty>:tabs.map(info=><TerminalView key={info.id} info={info} active={active===info.id} buffers={buffers} runtimes={runtimes} report={report}/>)}</div>
  </section>;
}
