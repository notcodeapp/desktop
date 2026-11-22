export interface Project { id: string; name: string; path: string }
export interface FileEntry { name: string; path: string; isDir: boolean }
export interface FileHistoryEntry { timestamp: number; size: number }
export interface SearchHit { path: string; line: number; preview: string }
export interface ReplaceResult { filesChanged: number; replacements: number; skipped: number }
export interface VaultChatsSyncResult { pushed: number; pulled: number; skippedLarge: number; skippedExisting: number }
export interface FileTab { path: string; content: string; saved: string; external?: boolean }
export interface Session { id: string; title: string; provider: string; model: string; updatedAt: number; externalId?: string | null }
export interface CliStatus { provider: string; available: boolean; path: string; version: string; error: string }
export interface Message { id: string; sessionId: string; role: 'user' | 'assistant' | 'system'; content: string; createdAt: number }
export interface AgentEvent { sessionId: string; type: 'delta' | 'done' | 'error' | 'status' | 'tool' | 'action' | 'message'; text: string }
export interface PlanStep { title: string; status: 'pending' | 'in_progress' | 'done' }
export interface QueuedMessage { id: string; text: string }
export interface LiveAction { name: string; status: string; detail: string }
export interface AuditEntry { id: string; sessionId: string; name: string; arguments: string; result: string; status: string; createdAt: number }
export interface Skill { id: string; name: string; description: string; body: string; enabled: boolean; scope: string }
export interface VoiceStatus { available: boolean; engine: string; running: boolean }
export interface ActivityDay { date: string; messages: number }
export interface ActivityStats { sessions: number; messages: number; toolCalls: number; activeDays: number; streak: number; peakDay: string | null; peakDayMessages: number; days: ActivityDay[] }
export interface ProviderProfile { id: string; name: string; baseUrl: string; models: string[]; active: boolean }
export interface AgentToolRequest { requestId: string; sessionId: string; name: string; arguments: unknown }
export interface WorkspaceFileChange { projectId: string; path: string }
export interface GitStatus { branch: string; files: {path: string; status: string}[] }
export interface GitCommit { hash: string; summary: string; author: string; date: string }
export interface McpConfig { id: string; name: string; command: string; args: string[]; connected: boolean }
export interface McpTool { name: string; description: string; inputSchema: unknown }
export interface TerminalInfo { id: string; shell: string }
export interface ModelProgress { jobId: string; status: string; completed?: number; total?: number; done: boolean; error?: string; cancelled: boolean }
export interface CliImportPreview { path: string; digest: string; provider: 'claude' | 'codex' | 'opencode'; externalId: string | null; title: string; model: string | null; cwd: string | null; projectMatches: boolean; messageCount: number; sample: { role: 'user' | 'assistant' | 'system'; content: string }[] }
export interface BridgeDevice { id: string; name: string; createdAt: number }
export interface BridgeStatus { running: boolean; address: string; fingerprint: string; invitation: string; projectId: string; projectName: string; devices: BridgeDevice[]; previewPorts: number[] }
export interface BridgePairRequest { requestId: string; deviceName: string; address?: string }
export interface GithubRepo { fullName: string; private: boolean; defaultBranch: string; updatedAt: string; htmlUrl: string }
export type AgentStatus = 'disconnected' | 'connecting' | 'ready' | 'running' | 'error';
export interface AgentAdapter {
  readonly id: string; readonly label: string; readonly available: boolean;
  connect(): Promise<string[]>;
  startSession(model: string): Promise<Session>;
  resumeSession(id: string): Promise<Message[]>;
  sendMessage(id: string, content: string, contextPath: string | null): Promise<void>;
  getStatus(): AgentStatus;
  interrupt(id: string): Promise<void>;
  getEvents(handler: (event: AgentEvent) => void): Promise<() => void>;
  disconnect(): Promise<void>;
}
