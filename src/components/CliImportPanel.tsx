import { useEffect, useRef, useState } from 'react';
import { api, nativeAvailable } from '../lib/native';
import { adapters, cliLabels, isCliProvider } from '../lib/agents';
import type { CliImportPreview, Session } from '../lib/types';
import { SpinnerLabel, type Confirm, type Report } from './UI';

export interface CliImportPanelProps {
  projectId?: string;
  onImported: (session: Session) => void;
  report: Report;
  confirm: Confirm;
}

export function CliImportPanel({ projectId, onImported, report, confirm }: CliImportPanelProps) {
  const [preview, setPreview] = useState<CliImportPreview | null>(null);
  const [model, setModel] = useState('');
  const [models, setModels] = useState<string[]>([]);
  const [busy, setBusy] = useState<'preview' | 'models' | 'confirm' | 'import' | null>(null);
  const request = useRef(0);
  const currentProject = useRef(projectId); currentProject.current = projectId;
  useEffect(() => {
    request.current++; setPreview(null); setModel(''); setModels([]); setBusy(null);
    return () => { request.current++; };
  }, [projectId]);

  function cancel() { request.current++; setPreview(null); setModel(''); setModels([]); setBusy(null); }
  async function select() {
    const sequence = ++request.current; setBusy('preview'); setPreview(null); setModels([]);
    try {
      const value = await api.cliImportPreview();
      if (sequence !== request.current) return;
      if (value && !isCliProvider(value.provider)) throw new Error('Неизвестный CLI-провайдер.');
      setPreview(value); setModel(value?.model ?? (value?.provider === 'claude' ? 'default' : ''));
    } catch (error) { if (sequence === request.current) report(error); }
    finally { if (sequence === request.current) setBusy(null); }
  }
  async function loadModels() {
    if (!preview) return;
    const sequence = request.current; setBusy('models');
    try {
      const values = await adapters[preview.provider].connect();
      if (sequence !== request.current) return;
      setModels(values); if (!model) setModel(values[0] ?? '');
    } catch (error) { if (sequence === request.current) report(error); }
    finally { if (sequence === request.current) setBusy(null); }
  }
  async function importChat() {
    if (!preview || !model.trim() || !projectId) return;
    const sequence = request.current; const project = projectId; setBusy('confirm');
    try {
      const approved = await confirm('Импортировать CLI-чат?',
        `${cliLabels[preview.provider]} · ${preview.title}\n${preview.messageCount} сообщений · модель ${model.trim()}\n\n` +
        (preview.projectMatches && preview.externalId
          ? 'Каталог источника совпадает с проектом. При последующей отправке разрешено продолжение исходной CLI-сессии.'
          : 'Каталог источника не совпадает с проектом или отсутствует. История будет импортирована без привязки к исходной CLI-сессии.') +
        '\n\nИнструменты не выполняются. Настройки и авторизация не копируются. Текст источника — недоверенные данные.', false);
      if (!approved || sequence !== request.current || currentProject.current !== project) return;
      setBusy('import');
      const session = await api.cliImportChat({ path: preview.path, digest: preview.digest, model: model.trim(), approved: true });
      if (sequence !== request.current || currentProject.current !== project) return;
      setPreview(null); setModels([]); onImported(session);
    } catch (error) { if (sequence === request.current) report(error); }
    finally { if (sequence === request.current) setBusy(null); }
  }

  return <section aria-label="Импорт CLI-чата" className="cli-import-panel">
    <h3>Импорт CLI-чата</h3>
    <p className="muted">Claude Code / Codex JSONL или OpenCode export JSON · до 8 MiB. Только история, без запуска инструментов.</p>
    <button type="button" disabled={!nativeAvailable || !projectId || busy !== null} onClick={() => void select()}>Выбрать JSON / JSONL</button>
    {!projectId && <p className="muted">Сначала откройте проект.</p>}
    {busy && <SpinnerLabel>{busy === 'import' ? 'Импортирую…' : busy === 'models' ? 'Получаю модели CLI…' : busy === 'confirm' ? 'Ожидаю подтверждение…' : 'Открываю предпросмотр…'}</SpinnerLabel>}
    {preview && <div>
      <dl>
        <dt>Провайдер</dt><dd>{cliLabels[preview.provider]}</dd>
        <dt>Название</dt><dd>{preview.title}</dd>
        <dt>ID источника</dt><dd>{preview.externalId ?? 'Отсутствует (дедупликация по SHA-256)'}</dd>
        <dt>Каталог источника</dt><dd>{preview.cwd ?? 'Не указан'}</dd>
        <dt>Сообщений</dt><dd>{preview.messageCount}</dd>
        <dt>Исходная модель</dt><dd>{preview.model ?? 'Не указана'}</dd>
      </dl>
      <p role="status">{preview.projectMatches && preview.externalId
        ? 'Каталог совпадает. Исходная сессия может быть продолжена после импорта.'
        : 'Продолжение исходной CLI-сессии отключено: каталог не совпадает, не указан или отсутствует ID. Импорт не запускает новую сессию.'}</p>
      <label>Модель для последующих сообщений
        <input value={model} maxLength={200} disabled={busy !== null} onChange={event => setModel(event.target.value)} placeholder={preview.provider === 'opencode' ? 'provider/model' : 'ID модели'} />
      </label>
      <button type="button" disabled={busy !== null} onClick={() => void loadModels()}>Получить модели установленного CLI</button>
      {models.length > 0 && <label>Модели CLI<select value={models.includes(model) ? model : ''} disabled={busy !== null} onChange={event => setModel(event.target.value)}>
        <option value="" disabled>Выберите модель или введите ID выше</option>
        {models.map(id => <option key={id} value={id}>{id}</option>)}
      </select></label>}
      <h4>Ограниченный текстовый предпросмотр</h4>
      {preview.sample.map((message, index) => <div key={index}><strong>{message.role}</strong><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: 180, overflow: 'auto' }}>{message.content}</pre></div>)}
      <p className="muted">В SQLite сохраняется весь поддерживаемый текст. Сообщение больше 128 KiB отклоняет импорт целиком, а не обрезается. Повторный импорт возвращает существующий чат.</p>
      <button type="button" disabled={busy !== null || !model.trim()} onClick={() => void importChat()}>Просмотреть подтверждение и импортировать</button>
    </div>}
    {(preview || busy) && <button type="button" disabled={busy === 'import'} onClick={cancel}>Отмена</button>}
  </section>;
}
