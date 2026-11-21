// Temporary browser-only mock of the Tauri backend for UI verification. Not shipped.
type Cb = (data: { event: string; id: number; payload: unknown }) => void;
const callbacks = new Map<number, Cb>();
const listeners = new Map<string, Map<number, number>>();
let nextId = 1;
function emit(event: string, payload: unknown) {
  const subs = listeners.get(event);
  if (!subs) return;
  for (const [cbId, eventId] of subs) callbacks.get(cbId)?.({ event, id: eventId, payload });
}
const now = Date.now();
const base = now - 3600_000;
const project = { id: 'demo', name: 'notcode-demo', path: 'C:/Projects/notcode-demo' };
const profile = { username: 'notkovvik', avatarUrl: 'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" width="28" height="28"><rect width="28" height="28" rx="14" fill="%23B6E6A5"/><text x="14" y="19" font-size="14" text-anchor="middle" fill="%23191C20">N</text></svg>', expiresAt: new Date(now + 86400_000).toISOString() };
const files: Record<string, string> = {
  'package.json': '{\n  "name": "notcode-demo",\n  "version": "1.0.0",\n  "scripts": { "test": "node --test" }\n}\n',
  'README.md': '# notcode-demo\n\nДемо-проект для проверки интерфейса NotCode.\n',
  'src/cart.ts': 'export interface Item { price: number; qty: number }\nexport function total(items: Item[], discount = 0): number {\n  return items.reduce((sum, i) => sum + i.price * i.qty, 0) * (1 - discount);\n}\n',
  'src/util.ts': 'export const round = (n: number) => Math.round(n * 100) / 100;\n',
  'tests/cart.test.mjs': "import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('total', () => assert.equal(1, 1));\n",
};
const tree = [
  { name: 'src', path: 'src', isDir: true },
  { name: 'tests', path: 'tests', isDir: true },
  { name: 'package.json', path: 'package.json', isDir: false },
  { name: 'README.md', path: 'README.md', isDir: false },
];
const subdirs: Record<string, typeof tree> = {
  src: [{ name: 'cart.ts', path: 'src/cart.ts', isDir: false }, { name: 'util.ts', path: 'src/util.ts', isDir: false }],
  tests: [{ name: 'cart.test.mjs', path: 'tests/cart.test.mjs', isDir: false }],
};
const sessions = [
  { id: 's1', title: 'Промокод в корзине', provider: 'compatible', model: 'gpt-demo', updatedAt: base + 260_000, externalId: null },
  { id: 's2', title: 'Настройка тестов', provider: 'ollama', model: 'qwen2.5-coder:7b', updatedAt: base - 7200_000, externalId: null },
];
const messages: Record<string, { id: string; sessionId: string; role: string; content: string; createdAt: number }[]> = {
  s1: [
    { id: 'm1', sessionId: 's1', role: 'user', content: 'Посмотри корзину и добавь промокод NOTCODE10 на скидку 10%.', createdAt: base },
    { id: 'm2', sessionId: 's1', role: 'assistant', content: 'Готово. Добавил параметр `discount` в `total()` и применил промокод **NOTCODE10**:\n\n```ts\nexport function total(items: Item[], discount = 0): number {\n  return items.reduce((s, i) => s + i.price * i.qty, 0) * (1 - discount);\n}\n```\n\nСкидка 10% применяется при вводе промокода.', createdAt: base + 90_000 },
    { id: 'm3', sessionId: 's1', role: 'user', content: 'Теперь напиши тесты для скидки.', createdAt: base + 180_000 },
    { id: 'm4', sessionId: 's1', role: 'assistant', content: 'Добавил тесты в `tests/cart.test.mjs` и запустил их — всё зелёное.', createdAt: base + 260_000 },
  ],
  s2: [{ id: 'm5', sessionId: 's2', role: 'user', content: 'Привет', createdAt: base - 7200_000 }],
};
const audit: Record<string, { id: string; sessionId: string; name: string; arguments: string; result: string; status: string; createdAt: number }[]> = {
  s1: [
    { id: 'a1', sessionId: 's1', name: 'list_directory', arguments: JSON.stringify({ path: 'src' }), result: 'cart.ts\nutil.ts', status: 'ok', createdAt: base + 10_000 },
    { id: 'a2', sessionId: 's1', name: 'read_file', arguments: JSON.stringify({ path: 'src/cart.ts' }), result: 'export interface Item { price: number; qty: number }…', status: 'ok', createdAt: base + 20_000 },
    { id: 'a3', sessionId: 's1', name: 'write_file', arguments: JSON.stringify({ path: 'src/cart.ts', contentBytes: 184 }), result: 'Файл обновлён (184 байт).', status: 'ok', createdAt: base + 40_000 },
    { id: 'a4', sessionId: 's1', name: 'run_command', arguments: JSON.stringify({ command: 'npm test', cwd: 'C:/Projects/notcode-demo', timeoutSeconds: 120 }), result: 'tests 2\npass 2\nfail 0', status: 'ok', createdAt: base + 200_000 },
    { id: 'a5', sessionId: 's1', name: 'write_file', arguments: JSON.stringify({ path: 'tests/cart.test.mjs', contentBytes: 312 }), result: 'Файл обновлён (312 байт).', status: 'ok', createdAt: base + 220_000 },
  ],
  s2: [],
};
let skills = [
  { id: 'sk1', name: 'code-review', description: 'Ревью кода по чек-листу команды NotCode', body: 'Проверяй читаемость, тесты и безопасность. Отвечай по-русски.', enabled: true, scope: 'global' },
  { id: 'sk2', name: 'funpay-style', description: 'Стиль текстов для NotShop', body: 'Пиши коротко, дружелюбно, без канцелярита.', enabled: false, scope: 'project' },
];
let gitRepo = true;
let gitFiles = [
  { path: 'src/cart.ts', status: ' M' },
  { path: 'tests/cart.test.mjs', status: '??' },
  { path: 'README.md', status: 'M ' },
];
let gitLog = [
  { hash: 'f3a9c21d8e7b6a5c4d3e2f1a0b9c8d7e6f5a4b3c', summary: 'Корзина: базовый расчёт суммы', author: 'notkovvik', date: '2026-11-10 14:22' },
  { hash: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678', summary: 'Инициализация проекта', author: 'notkovvik', date: '2026-11-08 09:15' },
];
let mcpServers = [{ id: 'mcp1', name: 'filesystem', command: 'npx', args: ['-y', '@modelcontextprotocol/server-filesystem', '.'], connected: false }];
let voiceRunning = false;
let bridgeRunning = false;
const pendingApprovals = new Map<string, (approved: boolean) => void>();
const interrupted = new Set<string>();
let msgSeq = 100;
const delay = (ms: number) => new Promise(r => setTimeout(r, ms));
const action = (sessionId: string, name: string, status: string, detail: string) => emit('agent:event', { sessionId, type: 'action', text: JSON.stringify({ name, status, detail }) });

async function simulateAgent(sessionId: string, content: string) {
  const list = messages[sessionId] ?? (messages[sessionId] = []);
  const t0 = Date.now();
  list.push({ id: `m${++msgSeq}`, sessionId, role: 'user', content, createdAt: t0 });
  emit('agent:event', { sessionId, type: 'status', text: 'Думаю…' });
  await delay(400);
  if (interrupted.has(sessionId)) return finishInterrupted(sessionId);
  action(sessionId, 'read_file', 'running', 'src/cart.ts');
  await delay(700);
  action(sessionId, 'read_file', 'ok', 'src/cart.ts · 184 байт');
  audit[sessionId] ??= [];
  audit[sessionId].push({ id: `a${++msgSeq}`, sessionId, name: 'read_file', arguments: JSON.stringify({ path: 'src/cart.ts' }), result: files['src/cart.ts'], status: 'ok', createdAt: Date.now() });
  action(sessionId, 'write_file', 'approval', 'src/cart.ts · 210 байт');
  const approved = await new Promise<boolean>(resolve => {
    pendingApprovals.set(`req-${sessionId}`, resolve);
    emit('agent:tool-request', { requestId: `req-${sessionId}`, sessionId, name: 'write_file', arguments: { path: 'src/cart.ts', contentPreview: 'export const PROMO = "NOTCODE10";\nexport const DISCOUNT = 0.1;\n', contentBytes: 210 } });
  });
  if (interrupted.has(sessionId)) return finishInterrupted(sessionId);
  if (approved) {
    action(sessionId, 'write_file', 'running', 'src/cart.ts');
    await delay(600);
    action(sessionId, 'write_file', 'ok', 'src/cart.ts · 210 байт');
    audit[sessionId].push({ id: `a${++msgSeq}`, sessionId, name: 'write_file', arguments: JSON.stringify({ path: 'src/cart.ts', contentBytes: 210 }), result: 'Файл обновлён (210 байт).', status: 'ok', createdAt: Date.now() });
    emit('workspace:file-changed', { projectId: project.id, path: 'src/cart.ts' });
  } else {
    action(sessionId, 'write_file', 'denied', 'Отклонено пользователем');
    audit[sessionId].push({ id: `a${++msgSeq}`, sessionId, name: 'write_file', arguments: JSON.stringify({ path: 'src/cart.ts', contentBytes: 210 }), result: 'Отклонено пользователем.', status: 'denied', createdAt: Date.now() });
  }
  const text = approved ? 'Промокод **NOTCODE10** добавлен в `src/cart.ts`. Скидка 10% применится в `total()`.' : 'Хорошо, файл не менял. Могу предложить diff текстом, если нужно.';
  for (const chunk of text.match(/.{1,24}(\s|$)/g) ?? [text]) {
    if (interrupted.has(sessionId)) return finishInterrupted(sessionId);
    emit('agent:event', { sessionId, type: 'delta', text: chunk });
    await delay(90);
  }
  const assistantId = `m${++msgSeq}`;
  list.push({ id: assistantId, sessionId, role: 'assistant', content: text, createdAt: Date.now() });
  emit('agent:event', { sessionId, type: 'message', text: assistantId });
  emit('agent:event', { sessionId, type: 'done', text: 'Ответ сохранён.' });
  const s = sessions.find(x => x.id === sessionId);
  if (s) { s.updatedAt = Date.now(); if (s.title === 'Новый диалог') s.title = content.slice(0, 48); }
}
function finishInterrupted(sessionId: string) {
  interrupted.delete(sessionId);
  emit('agent:event', { sessionId, type: 'done', text: 'interrupted' });
}

const commands: Record<string, (args: Record<string, unknown>) => unknown> = {
  project_current: () => project,
  project_open: () => project,
  project_tree: args => (args.path ? subdirs[String(args.path)] ?? [] : tree),
  file_read: args => files[String(args.path)] ?? '',
  file_write: args => { files[String(args.path)] = String(args.content); },
  project_search: args => [{ path: 'src/cart.ts', line: 2, preview: `export function total(${String(args.query)})…` }],
  auth_me: () => profile,
  auth_login: () => {},
  auth_logout: () => {},
  settings_get: () => ({ ollamaUrl: 'http://127.0.0.1:11434' }),
  settings_set: () => {},
  compatible_get: () => ({ baseUrl: 'https://api.demo.notcode.app/v1', model: 'gpt-demo, gpt-demo-mini' }),
  compatible_set: () => {},
  compatible_models: () => ({ baseUrl: 'https://api.demo.notcode.app/v1', models: ['gpt-demo', 'gpt-demo-mini', 'gpt-demo-vision'] }),
  key_set: () => {},
  key_delete: () => {},
  agent_models: () => ['llama3.1:8b', 'qwen2.5-coder:7b'],
  cli_status: args => ({ provider: String(args.provider), available: true, path: `C:/Tools/${String(args.provider)}.exe`, version: '1.4.2', error: '' }),
  cli_models: args => (args.provider === 'codex' ? ['gpt-5-codex', 'gpt-5'] : args.provider === 'claude' ? ['sonnet', 'opus'] : ['opencode/big-pickle']),
  cli_import_preview: () => null,
  sessions_list: () => [...sessions].sort((a, b) => b.updatedAt - a.updatedAt),
  session_messages: args => messages[String(args.sessionId)] ?? [],
  session_audit: args => audit[String(args.sessionId)] ?? [],
  session_create: args => {
    const s = { id: `s${++msgSeq}`, title: 'Новый диалог', provider: String(args.provider), model: String(args.model), updatedAt: Date.now(), externalId: null };
    sessions.unshift(s); messages[s.id] = []; audit[s.id] = []; return s;
  },
  session_delete: args => { const i = sessions.findIndex(s => s.id === args.sessionId); if (i >= 0) sessions.splice(i, 1); },
  agent_send: args => { const id = String(args.sessionId); interrupted.delete(id); void simulateAgent(id, String(args.content)); },
  agent_interrupt: args => { interrupted.add(String(args.sessionId)); pendingApprovals.get(`req-${String(args.sessionId)}`)?.(false); },
  agent_tool_respond: args => { pendingApprovals.get(String(args.requestId))?.(Boolean(args.approved)); pendingApprovals.delete(String(args.requestId)); },
  git_status: () => { if (!gitRepo) throw new Error('not a git repository: C:/Projects/notcode-demo'); return { branch: 'main', files: gitFiles.map(f => ({ ...f })) }; },
  git_branches: () => ['main', 'feat/cart', 'fix/totals'],
  git_log: () => gitLog,
  git_remote: () => 'https://github.com/notcodeapp/notcode-demo.git',
  git_diff: args => `diff --git a/${String(args.path ?? '')} b/${String(args.path ?? '')}\n--- a/${String(args.path ?? '')}\n+++ b/${String(args.path ?? '')}\n@@ -1,3 +1,4 @@\n export interface Item { price: number; qty: number }\n+export const PROMO = 'NOTCODE10';\n export function total(items: Item[], discount = 0): number {\n   return items.reduce((sum, i) => sum + i.price * i.qty, 0) * (1 - discount);\n`,
  git_init: () => { gitRepo = true; },
  git_stage: args => {
    const paths = args.all ? gitFiles.map(f => f.path) : (args.paths as string[]);
    gitFiles = gitFiles.map(f => {
      if (!paths.includes(f.path)) return f;
      if (args.unstage) return { ...f, status: f.status[0] === 'A' ? '??' : ` ${f.status[1] === ' ' ? 'M' : f.status[1]}` };
      return { ...f, status: f.status === '??' ? 'A ' : `${f.status[1]} ` };
    });
  },
  git_commit: args => {
    const staged = gitFiles.filter(f => f.status !== '??' && f.status[0] !== ' ');
    if (!staged.length) throw new Error('Нет подготовленных изменений. Добавьте файлы в индекс.');
    gitFiles = gitFiles.flatMap(f => f.status === '??' || f.status[0] === ' ' ? [f] : f.status[1] === ' ' ? [] : [{ ...f, status: ` ${f.status[1]}` }]);
    const hash = Math.random().toString(16).slice(2).padEnd(40, '0');
    gitLog = [{ hash, summary: String(args.message), author: 'notkovvik', date: new Date().toISOString().slice(0, 16).replace('T', ' ') }, ...gitLog];
    return `[main ${hash.slice(0, 7)}] ${String(args.message)}\n ${staged.length} file(s) changed`;
  },
  git_sync: args => `${args.direction === 'pull' ? 'Already up to date.' : 'To https://github.com/notcodeapp/notcode-demo.git\n   f3a9c21..a1b2c3d  main -> main'}`,
  git_checkout: args => `Switched to branch '${String(args.branch)}'`,
  git_clone: args => ({ id: 'cloned', name: String(args.url).split('/').pop()?.replace(/\.git$/, '') ?? 'repo', path: 'C:/Projects/cloned' }),
  github_repos: () => [
    { fullName: 'notcodeapp/app', private: false, defaultBranch: 'main', updatedAt: '2026-11-12T18:40:00Z', htmlUrl: 'https://github.com/notcodeapp/app' },
    { fullName: 'notkovvik/notshop-tools', private: true, defaultBranch: 'main', updatedAt: '2026-11-11T10:05:00Z', htmlUrl: 'https://github.com/notkovvik/notshop-tools' },
    { fullName: 'notkovvik/funpay-scripts', private: true, defaultBranch: 'master', updatedAt: '2026-10-30T21:12:00Z', htmlUrl: 'https://github.com/notkovvik/funpay-scripts' },
  ],
  skills_list: () => skills.map(s => ({ ...s })),
  skill_save: args => {
    const name = String(args.name ?? '').trim();
    if (!name || name.length > 120) throw new Error('Название скилла: 1–120 символов.');
    const body = String(args.body ?? '');
    if (!body.trim() || body.length > 32768) throw new Error('Инструкция скилла: 1–32768 символов.');
    const existing = args.id ? skills.find(s => s.id === args.id) : undefined;
    const saved = { id: existing?.id ?? `sk${++msgSeq}`, name, description: String(args.description ?? '').slice(0, 500), body, enabled: Boolean(args.enabled), scope: String(args.scope) };
    if (existing) Object.assign(existing, saved); else skills.push(saved);
    return { ...saved };
  },
  skill_delete: args => { skills = skills.filter(s => !(s.id === args.id && s.scope === args.scope)); },
  voice_status: () => ({ available: true, engine: 'Windows Speech Recognition', running: voiceRunning }),
  voice_start: () => {
    voiceRunning = true;
    emit('voice:ready', { text: 'Слушаю…' });
    setTimeout(() => { if (voiceRunning) emit('voice:text', { text: 'добавь промокод на скидку' }); }, 1400);
    setTimeout(() => { if (voiceRunning) { voiceRunning = false; emit('voice:stopped', { text: 'добавь промокод на скидку' }); } }, 3200);
  },
  voice_stop: () => { voiceRunning = false; emit('voice:stopped', { text: 'добавь промокод на скидку' }); },
  mcp_list: () => mcpServers.map(s => ({ ...s })),
  mcp_add: args => { const s = { id: `mcp${++msgSeq}`, name: String(args.name), command: String(args.command), args: args.args as string[], connected: false }; mcpServers.push(s); return s; },
  mcp_remove: args => { mcpServers = mcpServers.filter(s => s.id !== args.id); },
  mcp_connect: args => { const s = mcpServers.find(x => x.id === args.id); if (s) s.connected = true; },
  mcp_disconnect: args => { const s = mcpServers.find(x => x.id === args.id); if (s) s.connected = false; },
  mcp_tools: () => [{ name: 'read_file', description: 'Прочитать файл из разрешённой папки', inputSchema: { type: 'object', properties: { path: { type: 'string' } } } }],
  mcp_call: () => ({ content: files['src/cart.ts'] }),
  bridge_addresses: () => ['192.168.1.50'],
  bridge_status: () => ({ running: bridgeRunning, address: bridgeRunning ? '192.168.1.50:8765' : '', fingerprint: bridgeRunning ? '9A:4C:11:E7:B2:0D' : '', invitation: bridgeRunning ? 'notcode://pair?addr=192.168.1.50:8765&fp=9A4C11E7B20D' : '', projectId: project.id, projectName: project.name, devices: bridgeRunning ? [{ id: 'd1', name: 'Pixel 8', createdAt: base }] : [], previewPorts: [] }),
  bridge_start: () => { bridgeRunning = true; return commands.bridge_status({}); },
  bridge_stop: () => { bridgeRunning = false; },
  bridge_invite: () => commands.bridge_status({}),
  bridge_revoke: () => {},
  bridge_pair_respond: () => {},
  bridge_allow_preview: () => {},
  terminal_create: () => { setTimeout(() => emit('terminal:data', { id: 'term1', data: 'PowerShell 7.4.6\r\nPS C:\\Projects\\notcode-demo> ' }), 200); return { id: 'term1', shell: 'PowerShell' }; },
  terminal_write: args => { setTimeout(() => emit('terminal:data', { id: String(args.id), data: String(args.data).replace(/\r/g, '\r\n') }), 60); },
  terminal_resize: () => {},
  terminal_close: () => {},
  ollama_pull: () => 'job1',
  ollama_import: () => null,
  ollama_cancel: () => {},
  'plugin:window|destroy': () => { console.log('[mock] window destroyed'); },
};

(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
  invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
    if (cmd === 'plugin:event|listen') {
      const event = String(args.event); const eventId = nextId++;
      if (!listeners.has(event)) listeners.set(event, new Map());
      listeners.get(event)!.set(Number(args.handler), eventId);
      return eventId;
    }
    if (cmd === 'plugin:event|unlisten') {
      listeners.get(String(args.event))?.forEach((eventId, cbId) => { if (eventId === args.eventId) listeners.get(String(args.event))!.delete(cbId); });
      return null;
    }
    const fn = commands[cmd];
    if (!fn) { console.warn('[mock] unknown command', cmd, args); throw new Error(`mock: неизвестная команда ${cmd}`); }
    await delay(30);
    return fn(args ?? {});
  },
  transformCallback: (cb: Cb) => { const id = nextId++; callbacks.set(id, cb); return id; },
  unregisterCallback: (id: number) => { callbacks.delete(id); },
  convertFileSrc: (p: string) => p,
  metadata: { currentWindow: { label: 'main' }, currentWebview: { windowLabel: 'main', label: 'main' } },
};
(window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
(window as unknown as { __mockEmit: typeof emit }).__mockEmit = emit;

export {};