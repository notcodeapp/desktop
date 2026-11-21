import Monaco, { loader, type OnMount } from '@monaco-editor/react';
import * as monaco from 'monaco-editor';
import EditorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker';
import JsonWorker from 'monaco-editor/esm/vs/language/json/json.worker?worker';
import CssWorker from 'monaco-editor/esm/vs/language/css/css.worker?worker';
import HtmlWorker from 'monaco-editor/esm/vs/language/html/html.worker?worker';
import TsWorker from 'monaco-editor/esm/vs/language/typescript/ts.worker?worker';
import { useEffect, useRef, useState } from 'react';
import { BookOpen, Code2 } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import tokens from '../../design-system/tokens.json';
import { languageFor } from '../lib/pure';
import { markdownToHtml } from '../lib/markdown';
import { initTheme } from '../lib/theme';
self.MonacoEnvironment = { getWorker(_moduleId: string, label: string) {
  if (label === 'json') return new JsonWorker();
  if (label === 'css' || label === 'scss' || label === 'less') return new CssWorker();
  if (label === 'html' || label === 'handlebars' || label === 'razor') return new HtmlWorker();
  if (label === 'typescript' || label === 'javascript') return new TsWorker();
  return new EditorWorker();
} };
loader.config({ monaco });
initTheme();
const MARKDOWN = /\.(md|markdown|mdown)$/i;

/// Markdown preview pane. Content is rendered by the escaping renderer, and
/// link clicks are routed to the system browser, never in-app navigation.
function MarkdownPreview({ content, report }: { content: string; report: (value: unknown) => void }) {
  const html = markdownToHtml(content);
  return <div
    className="md-preview scroll-area"
    role="document"
    aria-label="Предпросмотр Markdown"
    onClick={event => {
      const anchor = (event.target as HTMLElement).closest('a');
      if (!anchor) return;
      const href = anchor.getAttribute('href') ?? '';
      if (/^https?:\/\//i.test(href)) { event.preventDefault(); void openUrl(href).catch(report); }
    }}
    dangerouslySetInnerHTML={{ __html: html }}
  />;
}

export function CodeEditor({path, projectId, content, change, save, line, report, minimapOn = false}: {path: string; projectId: string; content: string; change: (value:string) => void; save: () => void; line?: number; report?: (value:unknown) => void; minimapOn?: boolean}) {
  const editor = useRef<monaco.editor.IStandaloneCodeEditor | null>(null); const saveRef = useRef(save); saveRef.current = save;
  const [preview, setPreview] = useState(false);
  const isMarkdown = MARKDOWN.test(path);
  const mount: OnMount = instance => { editor.current = instance; instance.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => saveRef.current()); if (line) { instance.revealLineInCenter(line); instance.setPosition({lineNumber:line,column:1}); } };
  useEffect(() => { if (line && editor.current) { editor.current.revealLineInCenter(line); editor.current.setPosition({lineNumber:line,column:1}); editor.current.focus(); } }, [line,path]);
  return <div className="editor-host">
    {isMarkdown && <div className="editor-viewbar">
      <div className="toolbar">
        <button className={`icon-button ${preview?'':'active'}`} title="Редактор" aria-pressed={!preview} onClick={() => setPreview(false)}><Code2/></button>
        <button className={`icon-button ${preview?'active':''}`} title="Предпросмотр" aria-pressed={preview} onClick={() => setPreview(true)}><BookOpen/></button>
      </div>
    </div>}
    {isMarkdown && preview
      ? <MarkdownPreview content={content} report={value => report?.(value)}/>
      : <Monaco path={`notcode://${projectId}/${path}`} language={languageFor(path)} theme="notcode" value={content} onChange={v => change(v || '')} onMount={mount} loading={<p className="muted inset">Загрузка редактора…</p>} options={{fontFamily:tokens.typography.code,fontSize:13,lineHeight:22,minimap:{enabled:minimapOn},padding:{top:16},scrollBeyondLastLine:false,automaticLayout:true,smoothScrolling:true,tabSize:2,renderWhitespace:'selection',bracketPairColorization:{enabled:false},wordWrap:'off',contextmenu:true}}/>}
  </div>;
}
