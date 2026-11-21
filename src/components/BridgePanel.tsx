import { useCallback, useEffect, useRef, useState } from 'react';
import { Copy, QrCode as QrCodeIcon, RefreshCw, ShieldCheck, Smartphone, Unplug } from 'lucide-react';
import QRCode from 'qrcode';
import { api, event, nativeAvailable } from '../lib/native';
import type { BridgePairRequest, BridgeStatus } from '../lib/types';
import { Empty, SpinnerLabel, type Confirm, type Report } from './UI';

const stopped: BridgeStatus = { running: false, address: '', fingerprint: '', invitation: '', projectId: '', projectName: '', devices: [], previewPorts: [] };

export function useBridgePairing(confirm: Confirm, report: Report) {
  const queue = useRef(Promise.resolve());
  useEffect(() => {
    if (!nativeAvailable) return;
    let live = true;
    let stop: (() => void) | undefined;
    event<BridgePairRequest>('bridge:pair-request', request => {
      queue.current = queue.current.then(async () => {
        let approved = false;
        if (live) {
          approved = await confirm('Подключить телефон к этому ПК?', `Устройство: ${request.deviceName}${request.address ? `\nАдрес: ${request.address}` : ''}\n\nТелефон получит доступ к открытому проекту, его чатам и моделям этого компьютера. Он сможет отправлять запросы встроенному агенту Ollama/API и сохранять файлы. Создание и изменения файлов агентом требуют подтверждения на телефоне. Приватные файлы и ссылки на них не передаются; CLI-агенты и shell-команды удалённо недоступны.\n\nПодтвердите только своё устройство, которому вы передали приглашение.`, false);
        }
        await api.bridgePairRespond(request.requestId, approved && live);
        if (live) window.dispatchEvent(new Event('notcode:bridge-changed'));
      }).catch(report);
    }).then(unlisten => { if (live) stop = unlisten; else unlisten(); }).catch(report);
    return () => { live = false; stop?.(); };
  }, [confirm, report]);
}

export function BridgePanel({ projectId, projectName, confirm, report }: { projectId?: string; projectName?: string; confirm: Confirm; report: Report }) {
  const [status, setStatus] = useState<BridgeStatus>(stopped);
  const [addresses, setAddresses] = useState<string[]>([]);
  const [address, setAddress] = useState('');
  const [port, setPort] = useState('47178');
  const [previewPort, setPreviewPort] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [qr, setQr] = useState('');
  const [qrError, setQrError] = useState(false);
  const mounted = useRef(false);
  const operation = useRef(false);
  const refresh = useCallback(async () => {
    const value = await api.bridgeStatus();
    if (mounted.current) setStatus(value);
  }, []);
  useEffect(() => {
    mounted.current = true;
    if (!nativeAvailable) return () => { mounted.current = false; };
    void api.bridgeAddresses().then(list => {
      if (!mounted.current) return;
      setAddresses(list);
      setAddress(previous => list.includes(previous) ? previous : list.find(item => item !== '127.0.0.1') || list[0] || '');
    }).catch(report);
    const update = () => { void refresh().catch(report); };
    update();
    window.addEventListener('notcode:bridge-changed', update);
    const timer = window.setInterval(update, 5000);
    return () => { mounted.current = false; window.clearInterval(timer); window.removeEventListener('notcode:bridge-changed', update); };
  }, [refresh, report]);
  async function run(action: () => Promise<void>) {
    if (operation.current) return;
    operation.current = true;
    setBusy(true); setError(''); setNotice('');
    try { await action(); if (mounted.current) await refresh(); }
    catch (value) { if (mounted.current) setError(value instanceof Error ? value.message : String(value)); }
    finally { operation.current = false; if (mounted.current) setBusy(false); }
  }
  async function start() {
    const selectedPort = Number(port);
    if (!Number.isInteger(selectedPort) || selectedPort < 1 || selectedPort > 65535) { setError('Порт должен быть целым числом от 1 до 65535.'); return; }
    if (!await confirm('Разрешить подключение телефона?', `Проект: ${projectName || 'текущий проект'}\nАдрес: ${address}:${selectedPort}\n\nБудет включён зашифрованный сервер только на выбранном сетевом интерфейсе. Сопряжение требует приглашения и отдельного подтверждения на этом ПК. Не публикуйте приглашение.\n\nФайлы, чаты и вычисления останутся на этом компьютере. Подключённый телефон сможет работать с выбранным проектом.`, false)) return;
    await run(async () => { const value = await api.bridgeStart(address, selectedPort); if (mounted.current) setStatus(value); });
  }
  async function stop() {
    if (!await confirm('Отключить связь с телефонами?', 'Удалённые генерации и ожидающие подтверждения будут остановлены. Подключения и предпросмотр закроются.', false)) return;
    await run(() => api.bridgeStop());
  }
  async function allowPreview() {
    const value = Number(previewPort);
    if (!Number.isInteger(value) || value < 1 || value > 65535) { setError('Укажите порт приложения от 1 до 65535.'); return; }
    if (!await confirm('Разрешить предпросмотр с телефона?', `Будет доступен HTTP-сервис 127.0.0.1:${value}.\n\nРазрешайте только свой сервер разрабатываемого приложения, не административную панель и не сервис с секретами. Это web-предпросмотр, а не трансляция рабочего стола Windows. Разрешение действует до отключения связки.`, false)) return;
    await run(async () => { await api.bridgeAllowPreview(value, true); if (mounted.current) setPreviewPort(''); });
  }
  const projectChanged = status.running && status.projectId !== projectId;
  useEffect(() => {
    setQr(''); setQrError(false);
    if (!status.invitation) return;
    let live = true;
    QRCode.toString(status.invitation, { type: 'svg', margin: 1, width: 240, errorCorrectionLevel: 'L' })
      .then(svg => { if (live) setQr(svg); })
      .catch(() => { if (live) setQrError(true); });
    return () => { live = false; };
  }, [status.invitation]);
  return <section className="panel bridge-panel" aria-label="Подключённые устройства">
    <div className="panel-heading">
      <span><Smartphone/> Телефон и Desktop</span>
      {nativeAvailable && <span className={`bridge-pill ${status.running ? 'on' : ''}`}>{status.running ? 'Связь включена' : 'Выключена'}</span>}
      <button className="icon-button" title="Обновить состояние" disabled={!nativeAvailable || busy} onClick={() => void run(refresh)}><RefreshCw/></button>
    </div>
    <div className="scroll-area bridge-content">
      {!nativeAvailable ? <Empty title="Доступно в приложении Desktop"><p>В браузерном предпросмотре сервер не запускается. Откройте Tauri-приложение, чтобы подключить телефон.</p></Empty> : <>
        {projectChanged && <p className="warning" role="alert">На ПК открыт другой проект. Отключите связку и включите её снова, чтобы явно разрешить доступ к новому проекту.</p>}
        {!status.running ? <>
          <div className="bridge-hero">
            <span className="bridge-hero-icon"><Smartphone/></span>
            <strong>Работайте с проектом с телефона</strong>
            <p>Чаты, файлы и агент этого компьютера — в кармане. Телефон увидит проект как свой: диалоги переносятся одним нажатием.</p>
          </div>
          <form className="bridge-form" onSubmit={e => { e.preventDefault(); void start(); }}>
            <div className="bridge-form-row">
              <label>Сеть<select value={address} onChange={e => setAddress(e.target.value)} disabled={busy}>{addresses.map(item => <option key={item} value={item}>{item}{item === '127.0.0.1' ? ' · только этот ПК' : ''}</option>)}</select></label>
              <label>Порт<input type="number" min="1" max="65535" step="1" value={port} onChange={e => setPort(e.target.value)} disabled={busy}/></label>
            </div>
            {!projectId && <p className="hint">Сначала откройте проект, который хотите использовать с телефона.</p>}
            {address === '127.0.0.1' && <p className="hint">Для телефона выберите адрес локальной сети. Loopback доступен только этому ПК или через заранее настроенный SSH-туннель.</p>}
            <button className="primary" disabled={busy || !projectId || !address}><Smartphone/> Включить связь</button>
          </form>
        </> : <div className="bridge-layout">
          <div className="bridge-card bridge-invite">
            <h3>Приглашение</h3>
            <p className="hint">Отсканируйте на телефоне в разделе «ПК». Приглашение одноразовое.</p>
            {qr && <div className="bridge-qr" role="img" aria-label="QR-код приглашения" dangerouslySetInnerHTML={{ __html: qr }}/>}
            {!qr && qrError && <p className="warning" role="alert">Не удалось построить QR-код. Скопируйте приглашение вручную.</p>}
            <div className="toolbar">
              <button disabled={busy || !status.invitation} onClick={() => void run(async () => { await navigator.clipboard.writeText(status.invitation); if (mounted.current) setNotice('Приглашение скопировано.'); })}><Copy/> Копировать</button>
              <button disabled={busy || !status.invitation} onClick={() => void run(async () => { const svg = await QRCode.toString(status.invitation, { type: 'svg', margin: 1, width: 480, errorCorrectionLevel: 'L' }); const blob = new Blob([svg], { type: 'image/svg+xml' }); const url = URL.createObjectURL(blob); try { const link = document.createElement('a'); link.href = url; link.download = 'notcode-invitation.svg'; link.click(); } finally { URL.revokeObjectURL(url); } if (mounted.current) setNotice('QR-код сохранён.'); })}><QrCodeIcon/> QR-файл</button>
            </div>
            <details className="bridge-manual"><summary>Вставить приглашение на другом устройстве вручную</summary>
              <label className="sr-only" htmlFor="desktop-invitation">Одноразовое приглашение</label><textarea id="desktop-invitation" readOnly value={status.invitation} spellCheck={false} className="code-input" placeholder="Создайте новое приглашение"/>
              <button disabled={busy || projectChanged} onClick={() => void run(async () => { const value = await api.bridgeInvite(); if (mounted.current) setStatus(value); })}>Новое приглашение</button>
            </details>
          </div>
          <div className="bridge-card">
            <h3>{status.projectName || 'Проект'}</h3>
            <code>{status.address}</code>
            <p className="hint fingerprint-line">Отпечаток сертификата: <code className="fingerprint">{status.fingerprint.slice(0, 16)}…</code></p>
            <button onClick={() => void stop()} disabled={busy}><Unplug/> Отключить связь</button>
            <h3>Устройства <span className="bridge-count">{status.devices.length}</span></h3>
            {!status.devices.length && <p className="hint">Пока никто не подключён. Покажите QR-код и подтвердите запрос.</p>}
            {status.devices.map(device => <div className="device-card" key={device.id}>
              <Smartphone/>
              <div className="device-meta"><strong>{device.name}</strong><small>с {new Date(device.createdAt).toLocaleDateString('ru-RU')} · имеет доступ к этому проекту</small></div>
              <button className="icon-button" title="Отозвать доступ" disabled={busy} onClick={() => { void (async () => { if (await confirm('Отозвать доступ устройства?', `«${device.name}» больше не сможет подключаться. Его удалённые генерации и подтверждения будут остановлены.`, false)) await run(() => api.bridgeRevoke(device.id)); })().catch(report); }}><Unplug/></button>
            </div>)}
            <h3>Превью приложений</h3>
            <p className="hint">Запущенный на ПК HTTP-сервер открывается на телефоне через защищённый канал.</p>
            <form className="bridge-form" onSubmit={e => { e.preventDefault(); void allowPreview(); }}>
              <div className="bridge-form-row">
                <label>Порт<input type="number" min="1" max="65535" value={previewPort} onChange={e => setPreviewPort(e.target.value)} placeholder="3000" disabled={busy || projectChanged}/></label>
                <button disabled={busy || projectChanged || !previewPort}>Разрешить</button>
              </div>
            </form>
            {!!status.previewPorts.length && <div className="toolbar">{status.previewPorts.map(port => <span key={port} className="bridge-port-chip">:{port}</span>)}</div>}
          </div>
        </div>}
        <p className="bridge-footnote"><ShieldCheck/> Сопряжение вручную, TLS с закреплённым сертификатом. Ключи компьютера не покидают его: телефон получает чаты и файлы выбранного проекта, агент — только Ollama/настроенный API, без CLI и shell ПК.</p>
      </>}
      {busy && <SpinnerLabel>Выполняю…</SpinnerLabel>}
      {error && <p className="warning" role="alert">{error}</p>}
      {notice && <p className="hint" role="status">{notice}</p>}
    </div>
  </section>;
}
