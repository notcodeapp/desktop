import type { AuditEntry, FileTab, LiveAction, Message } from './types.ts';
export function languageFor(path: string): string {
  const extension = path.split('.').pop()?.toLowerCase() || '';
  const languages: Record<string, string> = { ts:'typescript', tsx:'typescript', js:'javascript', jsx:'javascript', rs:'rust', py:'python', json:'json', md:'markdown', css:'css', html:'html', toml:'ini', yaml:'yaml', yml:'yaml', ps1:'powershell', kt:'kotlin', java:'java', cs:'csharp', cpp:'cpp', c:'c', sh:'shell', sql:'sql', xml:'xml', txt:'plaintext' };
  return languages[extension] || 'plaintext';
}
export function isDirty(tab: FileTab): boolean { return tab.content !== tab.saved; }
export function replaceTab(tabs: FileTab[], path: string, content: string): FileTab[] {
  return tabs.map(tab => tab.path === path ? { ...tab, content } : tab);
}
export function savedTab(tabs: FileTab[], path: string, saved: string): FileTab[] {
  return tabs.map(tab => tab.path === path ? { ...tab, saved } : tab);
}
export function parseArgs(input: string): string[] {
  const value: unknown = JSON.parse(input);
  if (!Array.isArray(value) || value.some(arg => typeof arg !== 'string')) throw new Error('Аргументы должны быть JSON-массивом строк.');
  return value as string[];
}
export function parseToolArgs(input: string): Record<string, unknown> {
  const value: unknown = JSON.parse(input);
  if (value === null || Array.isArray(value) || typeof value !== 'object') throw new Error('Аргументы инструмента должны быть JSON-объектом.');
  return value as Record<string, unknown>;
}
export function clamp(value: number, min: number, max: number): number { return Math.min(max, Math.max(min, value)); }
export function basename(path: string): string { return path.replaceAll('\\', '/').split('/').filter(Boolean).pop() || path; }
export type TimelineGroup = { kind: 'message'; message: Message } | { kind: 'actions'; entries: AuditEntry[] };
export function buildTimeline(messages: Message[], audit: AuditEntry[]): TimelineGroup[] {
  const items = [...messages.map(message => ({ at: message.createdAt, action: null as AuditEntry | null, message })), ...audit.map(entry => ({ at: entry.createdAt, action: entry, message: null as Message | null }))].sort((a, b) => a.at - b.at);
  const groups: TimelineGroup[] = [];
  for (const item of items) {
    if (item.action) {
      const last = groups.at(-1);
      if (last?.kind === 'actions') last.entries.push(item.action);
      else groups.push({ kind: 'actions', entries: [item.action] });
    } else if (item.message) groups.push({ kind: 'message', message: item.message });
  }
  return groups;
}
export function actionCountLabel(count: number): string {
  const noun = count % 100 >= 11 && count % 100 <= 14 ? 'действий' : count % 10 === 1 ? 'действие' : count % 10 >= 2 && count % 10 <= 4 ? 'действия' : 'действий';
  return `${count} ${noun}`;
}
export function mergeLiveAction(list: LiveAction[], action: LiveAction): LiveAction[] {
  const active = action.status === 'approval' || action.status === 'running';
  let index = -1;
  if (!active) for (let i = list.length - 1; i >= 0; i--) { const item = list[i]; if (item.name === action.name && (item.status === 'approval' || item.status === 'running')) { index = i; break; } }
  const next = index >= 0 ? list.map((item, i) => i === index ? action : item) : [...list, action];
  return next.length > 500 ? next.slice(-500) : next;
}
