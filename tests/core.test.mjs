import test from 'node:test';
import assert from 'node:assert/strict';
import {parseArgs,parseToolArgs,savedTab,isDirty,languageFor,buildTimeline,actionCountLabel,mergeLiveAction} from '../src/lib/pure.ts';
import {markdownToHtml} from '../src/lib/markdown.ts';
test('markdown renders a safe subset',()=>{
  const html=markdownToHtml('# Заголовок\n\nТекст **жирный** и `код`.\n\n- один\n- два\n\n> цитата\n\n```js\nconst a = "<b>";\n```\n\n[ссылка](https://example.com) и [плохая](javascript:alert(1))');
  assert.match(html,/<h1>Заголовок<\/h1>/);
  assert.match(html,/<strong>жирный<\/strong>/);
  assert.match(html,/<code>код<\/code>/);
  assert.match(html,/<ul><li>один<\/li><li>два<\/li><\/ul>/);
  assert.match(html,/<blockquote>/);
  assert.match(html,/const a = &quot;&lt;b&gt;&quot;/);
  assert.match(html,/<a href="https:\/\/example\.com">ссылка<\/a>/);
  // Dangerous hrefs are stripped to their label, never rendered as attributes.
  assert.doesNotMatch(html,/javascript:/);
  // Raw HTML never passes through.
  assert.match(markdownToHtml('<img src=x onerror=alert(1)>'),/^(<p>&lt;img)/);
});
test('markdown fenced code keeps line breaks',()=>{
  const html=markdownToHtml('```\nline one\nline two\n```');
  assert.match(html,/line one\nline two/);
});
test('MCP arguments reject wrong shapes',()=>{
 assert.deepEqual(parseArgs('["a b","--flag"]'),['a b','--flag']);
 for(const value of ['{}','[1]','null']) assert.throws(()=>parseArgs(value));
 for(const value of ['[]','null','1']) assert.throws(()=>parseToolArgs(value));
});
test('saving an older snapshot preserves newer unsaved edits',()=>{
 const tabs=[{path:'a.ts',content:'new edit',saved:'original'}];
 const result=savedTab(tabs,'a.ts','saved snapshot');
 assert.equal(result[0].content,'new edit'); assert.equal(isDirty(result[0]),true);
});
test('editor language recognition',()=>{assert.equal(languageFor('src/main.rs'),'rust');assert.equal(languageFor('App.tsx'),'typescript');});
test('timeline groups only adjacent actions and keeps message order',()=>{
 const msg=(id,role,at)=>({id,sessionId:'s',role,content:id,createdAt:at});
 const act=(id,at)=>({id,sessionId:'s',name:'write_file',arguments:'{}',result:'{}',status:'ok',createdAt:at});
 const groups=buildTimeline([msg('u','user',1),msg('a1','assistant',2),msg('a2','assistant',6),msg('a3','assistant',9)],[act('t1',3),act('t2',4),act('t3',7)]);
 assert.deepEqual(groups.map(g=>g.kind),['message','message','actions','message','actions','message']);
 assert.equal(groups[2].entries.length,2);
 assert.equal(groups[4].entries.length,1);
 assert.deepEqual(buildTimeline([],[]),[]);
});
test('action count label uses Russian plural forms',()=>{
 assert.equal(actionCountLabel(1),'1 действие');
 assert.equal(actionCountLabel(2),'2 действия');
 assert.equal(actionCountLabel(5),'5 действий');
 assert.equal(actionCountLabel(11),'11 действий');
 assert.equal(actionCountLabel(21),'21 действие');
 assert.equal(actionCountLabel(112),'112 действий');
});
test('live actions pair running with final and keep parallel runs distinct',()=>{
 let list=[];
 list=mergeLiveAction(list,{name:'write_file',status:'running',detail:'a.kt'});
 list=mergeLiveAction(list,{name:'write_file',status:'ok',detail:'a.kt записан'});
 assert.equal(list.length,1); assert.equal(list[0].status,'ok');
 list=mergeLiveAction(list,{name:'write_file',status:'running',detail:'b.kt'});
 list=mergeLiveAction(list,{name:'write_file',status:'running',detail:'c.kt'});
 assert.equal(list.length,3);
 list=mergeLiveAction(list,{name:'write_file',status:'error',detail:'c.kt ошибка'});
 assert.equal(list.length,3); assert.equal(list.at(-1).status,'error'); assert.equal(list[1].status,'running');
});
test('live action list is bounded to 500',()=>{
 let list=[];
 for(let i=0;i<600;i++)list=mergeLiveAction(list,{name:`tool-${i}`,status:'ok',detail:''});
 assert.equal(list.length,500); assert.equal(list.at(-1).name,'tool-599');
});
