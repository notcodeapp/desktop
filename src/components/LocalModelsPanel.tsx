import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { api, event, nativeAvailable } from '../lib/native';

import type { ModelProgress } from '../lib/types';

export const LOCAL_MODELS_CHANGED_EVENT = 'native:provider-changed';

interface PanelSession {
  live: boolean;
  listening: boolean;
  starting: boolean;
  activeId: string | null;
  cancelling: boolean;
  refreshSequence: number;
  early: Map<string, ModelProgress>;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function byteLabel(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  const units = ['Б', 'КиБ', 'МиБ', 'ГиБ', 'ТиБ', 'ПиБ'];
  let index = 0;
  while (bytes >= 1024 && index < units.length - 1) { bytes /= 1024; index++; }
  return `${bytes.toLocaleString('ru-RU', { maximumFractionDigits: index ? 1 : 0 })} ${units[index]}`;
}

export function LocalModelsPanel() {
  const nameId = useId();
  const headingId = useId();
  const session = useRef<PanelSession | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [listError, setListError] = useState('');
  const [actionError, setActionError] = useState('');
  const [notice, setNotice] = useState('');
  const [model, setModel] = useState('');
  const [ready, setReady] = useState(false);
  const [starting, setStarting] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [progress, setProgress] = useState<ModelProgress | null>(null);

  const isCurrent = useCallback((s: PanelSession) => s.live && session.current === s, []);
  const refresh = useCallback(async () => {
    const s = session.current;
    if (!s || !s.live || !nativeAvailable) return;
    const sequence = ++s.refreshSequence;
    setLoading(true);
    setListError('');
    try {
      const found = await api.agentModels();
      if (isCurrent(s) && sequence === s.refreshSequence) {
        setModels([...new Set(found)].sort((a, b) => a.localeCompare(b)));
      }
    } catch (error) {
      if (isCurrent(s) && sequence === s.refreshSequence) setListError(errorText(error));
    } finally {
      if (isCurrent(s) && sequence === s.refreshSequence) setLoading(false);
    }
  }, [isCurrent]);

  const consume = useCallback((s: PanelSession, value: ModelProgress) => {
    if (!isCurrent(s) || value.jobId !== s.activeId) return;
    setProgress(value);
    if (!value.done) return;
    s.activeId = null;
    s.cancelling = false;
    setCancelling(false);
    if (value.cancelled) {
      setNotice('Операция отменена. Уже загруженные blob-файлы остаются в общем кэше Ollama.');
    } else if (value.error) {
      setActionError(value.error);
    } else if (value.status === 'success') {
      setNotice('Модель готова. Список моделей обновляется…');
      window.dispatchEvent(new CustomEvent(LOCAL_MODELS_CHANGED_EVENT, { detail: { provider: 'ollama' } }));
    } else {
      setActionError('Операция завершилась без подтверждения успеха от Ollama.');
    }
  }, [isCurrent, refresh]);

  useEffect(() => {
    const s: PanelSession = {
      live: true, listening: false, starting: false, activeId: null,
      cancelling: false, refreshSequence: 0, early: new Map(),
    };
    session.current = s;
    let unlisten: (() => void) | undefined;
    setReady(false);
    if (nativeAvailable) {
      void refresh();
      void event<ModelProgress>('ollama:progress', value => {
        if (!isCurrent(s)) return;
        if (value.jobId === s.activeId) {
          consume(s, value);
        } else if (s.starting) {
          // A fast daemon can finish BEFORE invoke resolves with its job ID.
          // Only the latest event per ID is needed; keep this buffer bounded.
          if (s.early.get(value.jobId)?.done) return;
          if (!s.early.has(value.jobId) && s.early.size >= 32) {
            const oldest = s.early.keys().next().value;
            if (oldest !== undefined) s.early.delete(oldest);
          }
          s.early.set(value.jobId, value);
        }
      }).then(stop => {
        if (isCurrent(s)) { unlisten = stop; s.listening = true; setReady(true); }
        else stop();
      }).catch(error => {
        if (isCurrent(s)) setActionError(`Не удалось подписаться на прогресс: ${errorText(error)}`);
      });
    }
    const changed = (e: Event) => {
      if ((e as CustomEvent<{provider?: string}>).detail?.provider === 'ollama') void refresh();
    };
    window.addEventListener(LOCAL_MODELS_CHANGED_EVENT, changed);
    return () => {
      window.removeEventListener(LOCAL_MODELS_CHANGED_EVENT, changed);
      s.live = false;
      s.listening = false;
      s.early.clear();
      unlisten?.();
      // No job-status API exists to reattach on remount, so do not leave an
      // invisible job running. A pending invoke is cancelled when its ID arrives.
      if (s.activeId) void api.ollamaCancel(s.activeId).catch(() => {});
    };
  }, [consume, isCurrent, refresh]);

  const busy = starting || Boolean(progress && !progress.done);
  const disabled = !nativeAvailable || !ready || loading || busy || !model.trim();

  async function begin(kind: 'pull' | 'import') {
    const s = session.current;
    if (!s || !isCurrent(s) || !s.listening || s.starting || s.activeId || disabled) return;
    s.starting = true;
    s.early.clear();
    setStarting(true);
    setCancelling(false);
    setProgress(null);
    setActionError('');
    setNotice(kind === 'import' ? 'Выберите GGUF-файл или все шарды одной модели в системном диалоге.' : 'Запрос загрузки модели…');
    const name = model.trim();
    try {
      const id = kind === 'pull' ? await api.ollamaPull(name) : await api.ollamaImport(name);
      if (!isCurrent(s)) {
        if (id) void api.ollamaCancel(id).catch(() => {});
        return;
      }
      if (id === null) { setNotice('Выбор файлов отменён.'); return; }
      s.activeId = id;
      s.starting = false;
      setStarting(false);
      setNotice('');
      setProgress({ jobId: id, status: 'Ожидание прогресса Ollama…', done: false, cancelled: false });
      const early = s.early.get(id);
      if (early) consume(s, early);
    } catch (error) {
      if (isCurrent(s)) { setActionError(errorText(error)); setNotice(''); }
    } finally {
      s.starting = false;
      s.early.clear();
      if (isCurrent(s)) setStarting(false);
    }
  }

  async function cancel() {
    const s = session.current;
    if (!s || !isCurrent(s) || !s.activeId || s.cancelling) return;
    const id = s.activeId;
    s.cancelling = true;
    setCancelling(true);
    setActionError('');
    try {
      await api.ollamaCancel(id);
      // Acknowledgement is not completion: hold conflicting buttons disabled
      // until the terminal progress event confirms that the worker has stopped.
    } catch (error) {
      if (isCurrent(s) && s.activeId === id) {
        s.cancelling = false;
        setCancelling(false);
        setActionError(`Не удалось отменить операцию: ${errorText(error)}`);
      }
    }
  }

  const percentage = progress?.total !== undefined && progress.total > 0
    && progress.completed !== undefined && Number.isFinite(progress.total) && Number.isFinite(progress.completed)
    ? Math.min(100, Math.max(0, progress.completed / progress.total * 100)) : undefined;

  return <section className="form-stack local-models-panel" aria-labelledby={headingId}>
    <h3 id={headingId}>Локальные модели Ollama</h3>
    {!nativeAvailable && <p className="hint" role="status">Управление моделями доступно только в NotCode Desktop.</p>}
    <div className="toolbar">
      <strong>Установленные модели{models.length ? ` · ${models.length}` : ''}</strong>
      <button type="button" disabled={!nativeAvailable || loading || busy} onClick={() => void refresh()}>
        {loading ? 'Обновление…' : 'Обновить'}
      </button>
    </div>
    {listError && <p role="alert" style={{ color: 'var(--colors-error)', overflowWrap: 'anywhere' }}>
      Не удалось получить список моделей: {listError}. Проверьте, что Ollama запущен и локальный адрес сохранён в настройках.
    </p>}
    <div aria-busy={loading} style={{ maxHeight: 180, overflowY: 'auto' }}>
      {models.length ? <ul>{models.map(name => <li key={name} style={{ overflowWrap: 'anywhere' }}><code>{name}</code></li>)}</ul>
        : !loading && !listError && nativeAvailable ? <p className="hint">Локальных моделей пока нет.</p> : null}
    </div>
    <form className="form-stack" onSubmit={e => { e.preventDefault(); void begin('pull'); }}>
      <label htmlFor={nameId}>Имя модели
        <input id={nameId} required maxLength={512} autoComplete="off" spellCheck={false}
          disabled={!nativeAvailable || busy} value={model} onChange={e => setModel(e.target.value)}
          placeholder="qwen2.5:7b или my-model:latest" />
      </label>
      <p className="hint">Загрузить — получить модель из registry Ollama (нужны интернет и место на диске). Импорт GGUF — выбрать локальный файл; для split-модели выберите все шарды, не переименовывая их. Имя с существующим тегом может заменить модель.</p>
      <div className="toolbar">
        <button type="submit" className="primary" disabled={disabled}>Загрузить модель</button>
        <button type="button" disabled={disabled} onClick={() => void begin('import')}>Импорт GGUF…</button>
      </div>
    </form>
    {notice && <p className="hint" role="status">{notice}</p>}
    {progress && <div className="form-stack" aria-busy={!progress.done}>
      <p className="hint" role="status" style={{ overflowWrap: 'anywhere' }}>
        {cancelling ? 'Отмена — ожидаем остановки операции…' : progress.status === 'success' ? 'Готово' : progress.cancelled ? 'Отменено' : progress.status}
      </p>
      {!progress.done && <>
        <progress aria-label="Прогресс операции с моделью" max={100} value={percentage} style={{ width: '100%' }} />
        {progress.completed !== undefined && <small>
          {byteLabel(progress.completed)}{progress.total !== undefined ? ` / ${byteLabel(progress.total)}` : ''}
          {percentage !== undefined ? ` · ${Math.floor(percentage)}%` : ''}
        </small>}
        <button type="button" disabled={cancelling} onClick={() => void cancel()}>{cancelling ? 'Отмена…' : 'Отменить операцию'}</button>
      </>}
    </div>}
    {actionError && <p role="alert" style={{ color: 'var(--colors-error)', overflowWrap: 'anywhere' }}>{actionError}</p>}
  </section>;
}
