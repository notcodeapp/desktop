import { Fragment, type ReactNode } from 'react';
function inline(text:string):ReactNode {
  return text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g).map((part,i)=>part.startsWith('`')?<code key={i}>{part.slice(1,-1)}</code>:part.startsWith('**')?<strong key={i}>{part.slice(2,-2)}</strong>:<Fragment key={i}>{part}</Fragment>);
}
function Markdown({text}:{text:string}) {
  const chunks=text.split(/(```[\s\S]*?(?:```|$))/g);
  return <>{chunks.map((chunk,i)=>{
    if(chunk.startsWith('```')){
      const first=chunk.indexOf('\n');const lang=chunk.slice(3,first<0?undefined:first).trim();
      if(lang==='notcode-read'||lang==='notcode-write')return null;
      return <div className="chat-code" key={i}><div>{lang||'code'}</div><pre><code>{first<0?'':chunk.slice(first+1).replace(/```$/,'')}</code></pre></div>;
    }
    return <Fragment key={i}>{chunk.split(/\n\s*\n/).filter(Boolean).map((paragraph,j)=>{
      const lines=paragraph.split('\n');
      if(lines.every(l=>/^\s*[-*] /.test(l)))return <ul key={j}>{lines.map((l,k)=><li key={k}>{inline(l.replace(/^\s*[-*] /,''))}</li>)}</ul>;
      if(lines.every(l=>/^\s*\d+[.)] /.test(l)))return <ol key={j}>{lines.map((l,k)=><li key={k}>{inline(l.replace(/^\s*\d+[.)] /,''))}</li>)}</ol>;
      return <div className="chat-paragraph" key={j}>{lines.map((l,k)=>/^#{1,6} /.test(l)?<h4 key={k}>{inline(l.replace(/^#{1,6} /,''))}</h4>:<Fragment key={k}>{k>0&&<br/>}{inline(l)}</Fragment>)}</div>;
    })}</Fragment>;
  })}</>;
}
export function MessageBody({content,streaming=false}:{content:string;streaming?:boolean}) {
  const thoughts:string[]=[];
  let thinking=false;
  const text=content.replace(/<(think|thinking)>([\s\S]*?)(?:<\/\1>|$)/gi,(whole,_tag,body)=>{thoughts.push(body);if(!/<\/(think|thinking)>$/i.test(whole))thinking=true;return '';});
  const active=streaming&&thinking;
  return <div className="formatted-message">{(thoughts.length>0||(streaming&&!content))&&<details className={`thinking-block ${active||!content?'is-thinking':''}`}><summary><span className="thinking-orbit"/>Thinking<span className="thinking-dots">{active||!content?'…':''}</span></summary><div><Markdown text={thoughts.join('\n\n')||'Ожидание ответа модели…'}/></div></details>}<Markdown text={text}/></div>;
}
