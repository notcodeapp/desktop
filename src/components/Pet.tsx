import { useEffect, useMemo, useRef, useState } from 'react';
import { Heart } from 'lucide-react';
import { api, nativeAvailable } from '../lib/native';
import type { ActivityStats } from '../lib/types';
import { SpinnerLabel, type Report } from './UI';

type Stage = 'egg' | 'hatchling' | 'companion';
type Mood = 'happy' | 'idle' | 'sleepy' | 'sad';
export type PetCommandFeedback = { ok: boolean; at: number };
export const PET_COMMAND_EVENT = 'notcode:pet-command';
/// How long the pet remembers the last build outcome, in milliseconds.
const COMMAND_MEMORY_MS = 120_000;

const NAME_KEY = 'notcode.pet.name';

const PHRASES: Record<Mood, string[]> = {
  happy: [
    'Мур-р! Код сегодня отличный!',
    'Ещё коммит — и я лопну от счастья!',
    'Ты в ударе, я в батарейках!',
    'Билд зелёный, настроение тоже!',
    'Я так люблю, когда тесты проходят!',
  ],
  idle: [
    'Напиши агенту пару строк — я оживу!',
    'Скучаю по твоим коммитам…',
    'Мур? Может, немного рефакторинга?',
    'Я подзаряжаюсь от диалогов с агентом.',
    'Говорят, у тебя идея. Проверяю антеннами!',
  ],
  sleepy: [
    'Zzz… позови меня кодом…',
    'Терминал тёплый, я сплю.',
    'Серия дней ждёт продолжения…',
    'Даже роботы мечтают о релизах…',
  ],
  sad: [
    'Билд упал… Ничего, мы это починим!',
    'Красные тесты — это просто подсказки.',
    'Ушки прижались. Запусти ещё раз?',
    'Ошибки боятся тех, кто их читает!',
    'Погладь меня, пока CI грустит…',
  ],
};

const PET_LINES: string[] = [
  'Мяу-бек! Всё под контролем.',
  'Не забудь сделать git commit!',
  'Мой любимый язык? Твой.',
  'Бесконечный цикл — это про мою привязанность.',
  'Если что сломается — я уже испугался заранее.',
  'Сохраняйся чаще, я нервный!',
  '404: кот найден, помогай!',
];

const COMMAND_LINES_OK = ['Сборка прошла — я гаже!', 'Зелёный билд, усы торчком!', 'exit 0 — лучший звук!'];
const COMMAND_LINES_FAIL = ['Ой… команда упала. Разберёмся!', 'Не паникуем: читаем вывод ошибки!', 'Упс. Я ненадолго прижал уши.'];

function stageFor(totalMessages: number): Stage {
  if (totalMessages >= 50) return 'companion';
  if (totalMessages >= 5) return 'hatchling';
  return 'egg';
}

function moodFor(stats: ActivityStats | null, feedback: PetCommandFeedback | null, now: number): Mood {
  if (feedback && now - feedback.at < COMMAND_MEMORY_MS) return feedback.ok ? 'happy' : 'sad';
  if (!stats) return 'sleepy';
  const today = stats.days.at(-1)?.messages ?? 0;
  if (today > 0) return 'happy';
  if (stats.streak > 0) return 'idle';
  return 'sleepy';
}

function PetFace({ stage, mood, blink }: { stage: Stage; mood: Mood; blink: boolean }) {
  const eye = blink ? 1 : 5.5;
  if (stage === 'egg') {
    return <svg viewBox="0 0 120 120" className="pet-svg" role="img" aria-label="Яйцо-питомец">
      <ellipse cx="60" cy="72" rx="30" ry="38" className="pet-shell" />
      <path d="M32 62 Q40 54 48 62 T64 62 T80 62 T92 66" className="pet-shell-line" fill="none" />
      <circle cx="52" cy="76" r={eye} className="pet-eye" />
      <circle cx="70" cy="76" r={eye} className="pet-eye" />
      {mood === 'sad'
        ? <path d="M54 92 Q61 86 68 92" className="pet-mouth" fill="none" />
        : <path d="M55 88 Q61 93 67 88" className="pet-mouth" fill="none" />}
      {mood === 'happy' && <text x="86" y="52" className="pet-sparkle">✦</text>}
    </svg>;
  }
  return <svg viewBox="0 0 120 120" className="pet-svg" role="img" aria-label="Питомец NotCode">
    {/* antenna + ears */}
    <line x1="60" y1="18" x2="60" y2="32" className={mood === 'sad' ? 'pet-antenna droop' : 'pet-antenna'} />
    <circle cx="60" cy="14" r="5" className={mood === 'happy' ? 'pet-antenna-bulb lit' : 'pet-antenna-bulb'} />
    <path d={mood === 'sad' ? 'M30 40 L26 26 L42 34 Z' : 'M30 40 L24 22 L44 32 Z'} className="pet-ear" />
    <path d={mood === 'sad' ? 'M90 40 L94 26 L78 34 Z' : 'M90 40 L96 22 L76 32 Z'} className="pet-ear" />
    {/* head */}
    <rect x="24" y="32" width="72" height="56" rx="20" className="pet-head" />
    {/* screen face */}
    <rect x="34" y="44" width="52" height="32" rx="10" className="pet-screen" />
    <circle cx="50" cy="58" r={blink ? 1 : 4.5} className="pet-eye" />
    <circle cx="70" cy="58" r={blink ? 1 : 4.5} className="pet-eye" />
    {mood === 'happy'
      ? <path d="M52 66 Q60 72 68 66" className="pet-mouth" fill="none" />
      : mood === 'sad'
        ? <path d="M52 69 Q60 62 68 69" className="pet-mouth" fill="none" />
        : mood === 'sleepy'
          ? <path d="M52 67 Q60 62 68 67" className="pet-mouth" fill="none" />
          : <path d="M54 67 L66 67" className="pet-mouth" fill="none" />}
    {/* paws */}
    <rect x="30" y="88" width="16" height="10" rx="5" className="pet-paw" />
    <rect x="74" y="88" width="16" height="10" rx="5" className="pet-paw" />
    {mood === 'sleepy' && <text x="88" y="30" className="pet-zzz">z</text>}
    {mood === 'sad' && <text x="92" y="46" className="pet-sparkle">💧</text>}
  </svg>;
}

export function PetSection({ report }: { report: Report }) {
  const [stats, setStats] = useState<ActivityStats | null>(null);
  const [name, setName] = useState(() => window.localStorage.getItem(NAME_KEY) || 'Нотик');
  const [blink, setBlink] = useState(false);
  const [hearts, setHearts] = useState<number[]>([]);
  const [speech, setSpeech] = useState('');
  const [feedback, setFeedback] = useState<PetCommandFeedback | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [statsFailed, setStatsFailed] = useState(false);
  const failedRef = useRef(false);
  useEffect(() => {
    if (!nativeAvailable) return;
    let live = true;
    // The pet must stay alive even when statistics are unavailable (older
    // database, migration hiccup): it just stays an egg until numbers return.
    void api.activityStats().then(value => { if (live) { setStats(value); setStatsFailed(false); } }).catch(() => { if (live) setStatsFailed(true); });
    return () => { live = false; };
  }, [report]);
  // run_command outcomes arrive from the agent panel as window events.
  useEffect(() => {
    function onCommand(event: Event) {
      const detail = (event as CustomEvent<{ ok?: boolean }>).detail;
      if (!detail || typeof detail.ok !== 'boolean') return;
      failedRef.current = !detail.ok;
      setFeedback({ ok: detail.ok, at: Date.now() });
      setNow(Date.now());
      setSpeech('');
      setPetLine(detail.ok ? COMMAND_LINES_OK : COMMAND_LINES_FAIL);
    }
    window.addEventListener(PET_COMMAND_EVENT, onCommand);
    return () => window.removeEventListener(PET_COMMAND_EVENT, onCommand);
  }, []);
  const setPetLine = (lines: string[]) => setSpeech(lines[Math.floor(Math.random() * lines.length)]);
  // Refresh "now" so the sad/happy memory window expires on its own.
  useEffect(() => {
    if (!feedback) return;
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [feedback]);
  useEffect(() => {
    const timer = window.setInterval(() => {
      setBlink(true);
      window.setTimeout(() => setBlink(false), 160);
    }, 3200 + Math.random() * 2600);
    return () => window.clearInterval(timer);
  }, []);
  const stage = stats ? stageFor(stats.messages) : 'egg';
  const mood = moodFor(stats, feedback, now);
  const phrase = useMemo(() => PHRASES[mood][Math.floor(Math.random() * PHRASES[mood].length)], [mood]);
  function pet() {
    const burst = [0, 120, 240].map(offset => Date.now() + offset);
    setHearts(old => [...old, ...burst].slice(-9));
    window.setTimeout(() => setHearts(old => old.filter(id => !burst.includes(id))), 1500);
    setSpeech(PET_LINES[Math.floor(Math.random() * PET_LINES.length)]);
  }
  function rename(value: string) {
    setName(value);
    window.localStorage.setItem(NAME_KEY, value);
  }
  const stageLabel = stage === 'egg' ? 'Яйцо' : stage === 'hatchling' ? 'Котёнок-бот' : 'Спутник';
  const moodLabel = mood === 'happy' ? 'доволен' : mood === 'sad' ? 'расстроен билдом' : mood === 'idle' ? 'ждёт' : 'дремлет';
  return <div className="form-stack">
    <div className="settings-section pet-section">
      <div className="pet-stage-wrap">
        <div className={`pet-stage mood-${mood}`} onClick={pet} role="button" tabIndex={0} aria-label="Погладить питомца" onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') pet(); }}>
          <PetFace stage={stage} mood={mood} blink={blink} />
          {hearts.map(id => <Heart key={id} className="pet-heart" size={16} />)}
        </div>
        <div className="pet-nameplate">
          <input value={name} maxLength={24} onChange={event => rename(event.target.value)} aria-label="Имя питомца" placeholder="Имя питомца" />
          <small>{stageLabel} · {moodLabel}</small>
        </div>
      </div>
      <div className="pet-dialogue">
        <p>{speech || phrase}</p>
        {!speech && <button type="button" className="icon-button" title="Сказать что-нибудь" onClick={() => setPetLine(PET_LINES)}>💬</button>}
      </div>
    </div>
    <div className="settings-section">
      <h3>Как это работает</h3>
      <p className="hint">Питомец растёт от вашей реальной локальной активности: 5 сообщений агенту — вылупляется, 50 — становится спутником. Он следит за сборками: упавшая команда расстраивает его на пару минут, зелёная — радует. Всё считается на этом компьютере; питомец никуда не отправляется.</p>
      {stats
        ? <div className="settings-stats">
            <article><strong>{stats.messages}</strong><span>Сообщений</span></article>
            <article><strong>{stats.streak}</strong><span>Дней подряд</span></article>
            <article><strong>{stage === 'egg' ? '0/5' : stage === 'hatchling' ? `${stats.messages}/50` : 'Максимум'}</strong><span>До следующей стадии</span></article>
          </div>
        : statsFailed
          ? <p className="hint">Статистика пока недоступна — питомец подождёт и появится, как только появятся данные. Погладить его можно уже сейчас.</p>
          : nativeAvailable ? <SpinnerLabel>Ищу питомца…</SpinnerLabel> : <p className="hint">Питомец живёт в приложении NotCode Desktop.</p>}
    </div>
  </div>;
}
