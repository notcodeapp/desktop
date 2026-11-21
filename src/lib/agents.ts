import { api, event } from './native';
import type { AgentAdapter, AgentEvent, AgentStatus } from './types';
export class NotCodeAdapter implements AgentAdapter {
  readonly available = true;
  constructor(readonly id = 'ollama', readonly label = 'Ollama') {}
  private status: AgentStatus = 'disconnected';
  private active = new Set<string>();
  async connect() {
    this.status = 'connecting';
    try {
      const models = this.id === 'compatible'
        ? await api.compatibleGet().then(c => { if (!c.model) throw new Error('Укажите модель и API-ключ в настройках.'); return c.model.split(/[\n,]+/).map(v=>v.trim()).filter(Boolean); })
        : this.id === 'opencode'
        ? await api.opencodeModels().then(list => list.map(m => m.id))
        : isCliProvider(this.id) ? await api.cliModels(this.id)
        : await api.agentModels();
      this.status = 'ready'; return models;
    }
    catch (error) { this.status = 'error'; throw error; }
  }
  startSession(model: string) { return api.createSession(this.id, model); }
  resumeSession(id: string) { return api.messages(id); }
  async sendMessage(id: string, content: string, contextPath: string | null) {
    this.active.add(id); this.status = 'running';
    try { await api.agentSend(id, content, contextPath ?? undefined); }
    catch (error) { this.active.delete(id); this.status = 'error'; throw error; }
  }
  getStatus() { return this.status; }
  interrupt(id: string) { return api.agentInterrupt(id); }
  getEvents(handler: (e: AgentEvent) => void) {
    return event<AgentEvent>('agent:event', e => {
      if (e.type === 'done' || e.type === 'error') {
        this.active.delete(e.sessionId); this.status = this.active.size ? 'running' : e.type === 'error' ? 'error' : 'ready';
      }
      handler(e);
    });
  }
  async disconnect() {
    await Promise.all([...this.active].map(id => api.agentInterrupt(id)));
    this.active.clear(); this.status = 'disconnected';
  }
}
export const ollama = new NotCodeAdapter();
export const compatible = new NotCodeAdapter('compatible','API');
export const codex = new NotCodeAdapter('codex','Codex');
export const claude = new NotCodeAdapter('claude','Claude Code');
export type CliProvider = 'claude' | 'codex' | 'opencode';
export const cliLabels: Record<CliProvider, string> = { claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode' };
export function isCliProvider(id: string): id is CliProvider { return id === 'claude' || id === 'codex' || id === 'opencode'; }
export const opencode = new NotCodeAdapter('opencode', cliLabels.opencode);
export const adapters: Record<string, NotCodeAdapter> = { ollama, compatible, codex, claude, opencode };
export const agentCatalog = [
  { id: 'ollama', label: 'Ollama', available: true, note: 'Локальные модели · реальный потоковый API' },
  { id: 'compatible', label: 'OpenAI-compatible API', available: true, note: 'HTTPS API · потоковые ответы · ключ в Windows Credential Manager' },
  { id: 'codex', label: 'OpenAI Codex', available: true, note: 'codex app-server · вход и модель берутся из вашего Codex CLI' },
  { id: 'claude', label: 'Claude Code', available: true, note: 'claude --print stream-json · вход берётся из вашего Claude Code' },
  { id: 'opencode', label: 'OpenCode Zen', available: true, note: 'Шлюз opencode.ai · бесплатные модели · ключ с opencode.ai/console в Настройках → Модели' },
] as const;