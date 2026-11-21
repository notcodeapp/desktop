import { useState } from 'react';
import { ArrowUp, Globe, Lock, RotateCw, ExternalLink, X } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { nativeAvailable } from '../lib/native';
import type { Report } from './UI';

const HOME_URL = 'http://127.0.0.1:5173';

/// Normalizes what the user typed into a loadable URL: bare host gets http://,
/// a word or phrase becomes a search.
function normalizeAddress(input: string): string {
  const value = input.trim();
  if (!value) return '';
  if (/^https?:\/\//i.test(value)) return value;
  if (/^localhost(:\d+)?(\/.*)?$/i.test(value)) return `http://${value}`;
  if (/^127\.0\.0\.1(:\d+)?(\/.*)?$/.test(value)) return `http://${value}`;
  if (/^[\w.-]+(:\d+)?(\/.*)?$/.test(value) && value.includes('.')) return `http://${value}`;
  return `https://duckduckgo.com/?q=${encodeURIComponent(value)}`;
}

function isLocal(url: string): boolean {
  try {
    const host = new URL(url).hostname;
    return host === 'localhost' || host === '127.0.0.1' || host === '::1' || host === '[::1]';
  } catch { return false; }
}

/// Simple embedded browser. iframes render local previews (localhost dev
/// servers, the project preview relay) and sites that allow framing; sites
/// with X-Frame-Options cannot be displayed by any in-app browser, so the
/// footer explains that and offers the system-browser shortcut.
export function BrowserPanel({ report }: { report: Report }) {
  const [address, setAddress] = useState(HOME_URL);
  const [loaded, setLoaded] = useState(HOME_URL);
  const [frameKey, setFrameKey] = useState(0);
  if (!nativeAvailable) return <div className="browser-panel panel"><div className="panel-heading"><span>Браузер</span></div><div className="scroll-area inset"><p className="hint">Встроенный браузер доступен в приложении NotCode Desktop.</p></div></div>;
  function go(target?: string) {
    const next = normalizeAddress(target ?? address);
    if (!next) return;
    setAddress(next);
    setLoaded(next);
    setFrameKey(key => key + 1);
  }
  return <div className="browser-panel panel">
    <form className="browser-bar" onSubmit={event => { event.preventDefault(); go(); }}>
      <Globe />
      <input
        value={address}
        onChange={event => setAddress(event.target.value)}
        onFocus={event => event.currentTarget.select()}
        placeholder="localhost:5173, адрес сайта или поисковый запрос"
        aria-label="Адрес"
        spellCheck={false}
        autoComplete="off"
      />
      <button type="button" className="icon-button" title="Перезагрузить" onClick={() => setFrameKey(key => key + 1)}><RotateCw /></button>
      <button type="submit" className="icon-button" title="Перейти"><ArrowUp /></button>
      <button type="button" className="icon-button" title="Открыть в системном браузере" onClick={() => { if (loaded) void openUrl(loaded).catch(report); }}><ExternalLink /></button>
    </form>
    <div className="browser-viewport">
      {isLocal(loaded) && <div className="browser-hint"><Lock /><span>Локальный адрес — страница проекта. Агент тоже может её прочитать: попросите его «посмотри http://127.0.0.1:порт».</span></div>}
      <iframe
        key={frameKey}
        className="browser-frame"
        src={loaded}
        title="Встроенный браузер"
        sandbox="allow-scripts allow-forms allow-popups allow-same-origin"
      />
    </div>
    <p className="browser-note">Страницы — недоверенные данные: не входите в важные аккаунты внутри NotCode. Если сайт запретил встраивание (X-Frame-Options), он останется пустым — откройте его по кнопке ↗ или попросите агента прочитать страницу.<button className="icon-button" title="Сбросить к локальному превью" onClick={() => go(HOME_URL)}><X /></button></p>
  </div>;
}
