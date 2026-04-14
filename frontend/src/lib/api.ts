import type {
  SetupStatus,
  SetScanPathsRequest,
  SaveApiKeyRequest,
  ApiKeyDisplay,
  ApiKeysResponse,
  Project,
  DetectedRepo,
  McpDefinition,
  McpOverview,
  McpConfigDisplay,
  McpEnvEntry,
  CreateMcpConfigRequest,
  UpdateMcpConfigRequest,
  LinkMcpConfigRequest,
  Discussion,
  CreateDiscussionRequest,
  SendMessageRequest,
  OrchestrationRequest,
  AgentDetection,
  AgentType,
  Contact,
  NetworkInfo,
  TokenUsageSummary,
  DbInfo,
  SetAgentAccessRequest,
  AgentsConfig,
  McpContextEntry,
  AiAuditStatus,
  LaunchAuditRequest,
  BootstrapProjectRequest,
  BootstrapProjectResponse,
  CloneProjectRequest,
  CloneProjectResponse,
  DiscoverReposRequest,
  DiscoverReposResponse,
  Workflow,
  WorkflowStep,
  WorkflowSummary,
  WorkflowRun,
  BatchRunSummary,
  StepResult,
  CreateWorkflowRequest,
  UpdateWorkflowRequest,
  AgentUsageSummary,
  Skill,
  CreateSkillRequest,
  AgentProfile,
  CreateProfileRequest,
  Directive,
  CreateDirectiveRequest,
  ServerConfigPublic,
  AiFileNode,
  AiFileContent,
  AiSearchResult,
  ModelTier,
  ModelTiersConfig,
  DriftCheckResponse,
  AddContactResult,
  WorkflowSuggestion,
  ContextFile,
  UploadContextFileResponse,
  ImportResult,
  TestStepRequest,
  QuickPrompt,
  CreateQuickPromptRequest,
} from '../types/generated';
import type { DiscoverKeysResponse } from '../types/extensions';

// ─── Auth token ──────────────────────────────────────────────────────────────

// Security note: localStorage is accessible to any JS on the page (XSS risk).
// For self-hosted/Tauri desktop deployments this is acceptable.
// For public-facing deployments, consider httpOnly cookies instead.
let _authToken: string | null = localStorage.getItem('kronn_auth_token');

export function setAuthToken(token: string | null) {
  _authToken = token;
  if (token) {
    localStorage.setItem('kronn_auth_token', token);
  } else {
    localStorage.removeItem('kronn_auth_token');
  }
}

export function getAuthToken(): string | null {
  return _authToken;
}

/** Build auth headers for fetch/EventSource */
export function authHeaders(): Record<string, string> {
  const h: Record<string, string> = {};
  if (_authToken) h['Authorization'] = `Bearer ${_authToken}`;
  return h;
}

// ─── API base URL (empty = same origin, set for Tauri desktop mode) ─────────

/** Resolved once: Tauri injects the backend URL, web mode uses relative paths */
let _apiBase = '';

export function setApiBase(base: string) {
  _apiBase = base.replace(/\/$/, ''); // strip trailing slash
}

export function getApiBase(): string {
  return _apiBase;
}

// ─── Shared SSE stream parser ────────────────────────────────────────────────

/**
 * Parse an SSE response body and dispatch events to handlers.
 * Extracts the duplicated ReadableStream SSE parsing loop used by
 * auditStream, partialAuditStream, fullAuditStream, _streamSSE, etc.
 */
async function parseSSEStream(
  res: Response,
  handlers: {
    onEvent: (type: string, data: unknown) => void;
    onDone: () => void;
    onError: (error: string) => void;
  },
) {
  if (!res.ok || !res.body) {
    handlers.onError(`HTTP ${res.status}`);
    return;
  }

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  try {
    while (true) {
      const { done: streamDone, value } = await reader.read();
      if (streamDone) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() ?? '';

      let eventType = '';
      for (const line of lines) {
        if (line.startsWith('event:')) {
          eventType = line.slice(6).trim();
        } else if (line.startsWith('data:')) {
          const data = line.slice(5).trim();
          try {
            const parsed = JSON.parse(data);
            handlers.onEvent(eventType, parsed);
          } catch { /* ignore non-JSON */ }
        }
      }
    }
    // Process any remaining data in buffer
    if (buffer.trim()) {
      const lines = buffer.split('\n');
      let eventType = '';
      for (const line of lines) {
        if (line.startsWith('event:')) {
          eventType = line.slice(6).trim();
        } else if (line.startsWith('data:')) {
          const data = line.slice(5).trim();
          try {
            const parsed = JSON.parse(data);
            handlers.onEvent(eventType, parsed);
          } catch { /* ignore non-JSON */ }
        }
      }
    }
  } catch (e: unknown) {
    if (e instanceof DOMException && e.name === 'AbortError') { handlers.onDone(); return; }
    throw e;
  }

  handlers.onDone();
}

/**
 * Initiate a fetch for SSE and parse the stream. Handles AbortSignal and common error patterns.
 * Returns null if aborted before response.
 */
async function fetchAndParseSSE(
  url: string,
  options: { method: string; headers: Record<string, string>; body?: string; signal?: AbortSignal },
  handlers: {
    onEvent: (type: string, data: unknown) => void;
    onDone: () => void;
    onError: (error: string) => void;
  },
  onResponse?: (res: Response) => void,
) {
  const res = await fetch(url, options).catch(e => {
    if (e.name === 'AbortError') { handlers.onDone(); return null; }
    throw e;
  });
  if (!res) return;

  if (onResponse) onResponse(res);

  await parseSSEStream(res, handlers);
}

// ─── Generic API wrapper ────────────────────────────────────────────────────

interface ApiResponse<T> {
  success: boolean;
  data: T | null;
  error: string | null;
}

async function api<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = { ...authHeaders() };
  if (body) headers['Content-Type'] = 'application/json';

  const res = await fetch(`${_apiBase}/api${path}`, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  });

  const contentType = res.headers.get('content-type') ?? '';
  if (!contentType.includes('application/json')) {
    throw new Error(`Server error (HTTP ${res.status})`);
  }

  const json: ApiResponse<T> = await res.json();

  if (!json.success) {
    throw new Error(json.error ?? 'Unknown API error');
  }

  return json.data as T;
}

// ─── Setup ──────────────────────────────────────────────────────────────────

export const setup = {
  getStatus: () => api<SetupStatus>('GET', '/setup/status'),
  setScanPaths: (req: SetScanPathsRequest) => api<void>('POST', '/setup/scan-paths', req),
  installAgent: (agentType: AgentType) => api<string>('POST', '/setup/install-agent', agentType),
  complete: () => api<void>('POST', '/setup/complete'),
  reset: () => api<void>('POST', '/setup/reset'),
};

// ─── Config ─────────────────────────────────────────────────────────────────

export const config = {
  getTokens: () => api<ApiKeysResponse>('GET', '/config/tokens'),
  saveApiKey: (req: SaveApiKeyRequest) => api<ApiKeyDisplay>('POST', '/config/api-keys', req),
  deleteApiKey: (id: string) => api<void>('DELETE', `/config/api-keys/${id}`),
  activateApiKey: (id: string) => api<void>('POST', `/config/api-keys/${id}/activate`),
  syncAgentTokens: () => api<string[]>('POST', '/config/sync-agent-tokens'),
  discoverKeys: () => api<DiscoverKeysResponse>('POST', '/config/discover-keys'),
  toggleTokenOverride: (provider: string) => api<boolean>('POST', '/config/toggle-token-override', provider),
  getLanguage: () => api<string>('GET', '/config/language'),
  saveLanguage: (lang: string) => api<void>('POST', '/config/language', lang),
  /** UI locale of the React frontend — persisted backend-side so it survives
   *  Tauri WebView2 localStorage wipes. */
  getUiLanguage: () => api<string>('GET', '/config/ui-language'),
  saveUiLanguage: (lang: string) => api<void>('POST', '/config/ui-language', lang),
  /** STT model ("onnx-community/whisper-tiny" etc.). null = never set. */
  getSttModel: () => api<string | null>('GET', '/config/stt-model'),
  saveSttModel: (modelId: string) => api<void>('POST', '/config/stt-model', modelId),
  /** TTS voices keyed by output language code. */
  getTtsVoices: () => api<Record<string, string>>('GET', '/config/tts-voices'),
  saveTtsVoice: (lang: string, voiceId: string) =>
    api<void>('POST', '/config/tts-voice', { lang, voice_id: voiceId }),
  /** Global context (markdown) injected into discussions. */
  getGlobalContext: () => api<string>('GET', '/config/global-context'),
  saveGlobalContext: (content: string) => api<void>('POST', '/config/global-context', content),
  /** When to inject: "always" | "no_project" | "never". */
  getGlobalContextMode: () => api<string>('GET', '/config/global-context-mode'),
  saveGlobalContextMode: (mode: string) => api<void>('POST', '/config/global-context-mode', mode),
  getScanPaths: () => api<string[]>('GET', '/config/scan-paths'),
  setScanPaths: (paths: string[]) => api<void>('POST', '/config/scan-paths', { paths }),
  getScanIgnore: () => api<string[]>('GET', '/config/scan-ignore'),
  setScanIgnore: (patterns: string[]) => api<void>('POST', '/config/scan-ignore', patterns),
  getScanDepth: () => api<number>('GET', '/config/scan-depth'),
  setScanDepth: (depth: number) => api<number>('POST', '/config/scan-depth', depth),
  getAgentAccess: () => api<AgentsConfig>('GET', '/config/agent-access'),
  setAgentAccess: (req: SetAgentAccessRequest) => api<void>('POST', '/config/agent-access', req),
  getModelTiers: () => api<ModelTiersConfig>('GET', '/config/model-tiers'),
  setModelTiers: (tiers: ModelTiersConfig) => api<void>('POST', '/config/model-tiers', tiers),
  dbInfo: () => api<DbInfo>('GET', '/config/db-info'),
  exportData: async (): Promise<Blob> => {
    const res = await fetch(`${_apiBase}/api/config/export`, {
      headers: authHeaders(),
    });
    if (!res.ok) throw new Error(`Export failed: ${res.status}`);
    return res.blob();
  },
  importData: async (file: File): Promise<ImportResult> => {
    const form = new FormData();
    form.append('file', file);
    const res = await fetch(`${_apiBase}/api/config/import`, {
      method: 'POST',
      headers: authHeaders(),
      body: form,
    });
    if (!res.ok) throw new Error(`Import failed: ${res.status}`);
    const json = await res.json();
    if (json.error) throw new Error(json.error);
    return json.data;
  },
  getServerConfig: () => api<ServerConfigPublic>('GET', '/config/server'),
  setServerConfig: (req: { domain?: string; max_concurrent_agents?: number; agent_stall_timeout_min?: number; pseudo?: string; avatar_email?: string; bio?: string }) => api<void>('POST', '/config/server', req),
  regenerateAuthToken: () => api<string>('POST', '/config/auth-token/regenerate'),
};

// ─── Contacts ───────────────────────────────────────────────────────────────

export const contacts = {
  list: () => api<Contact[]>('GET', '/contacts'),
  add: (invite_code: string) => api<AddContactResult>('POST', '/contacts', { invite_code }),
  delete: (id: string) => api<void>('DELETE', `/contacts/${id}`),
  inviteCode: () => api<string>('GET', '/contacts/invite-code'),
  ping: (id: string) => api<boolean>('GET', `/contacts/${id}/ping`),
  networkInfo: () => api<NetworkInfo>('GET', '/contacts/network-info'),
};

// ─── Projects ───────────────────────────────────────────────────────────────

export const projects = {
  list: () => api<Project[]>('GET', '/projects'),
  get: (id: string) => api<Project>('GET', `/projects/${id}`),
  scan: () => api<DetectedRepo[]>('POST', '/projects/scan'),
  create: (repo: DetectedRepo) => api<Project>('POST', '/projects', repo),
  addFolder: (req: { path: string; name?: string }) => api<Project>('POST', '/projects/add-folder', req),
  bootstrap: (req: BootstrapProjectRequest) => api<BootstrapProjectResponse>('POST', '/projects/bootstrap', req),
  delete: (id: string, hard?: boolean) => api<void>('DELETE', `/projects/${id}${hard ? '?hard=true' : ''}`),
  clone: (req: CloneProjectRequest) => api<CloneProjectResponse>('POST', '/projects/clone', req),
  discoverRepos: (req?: DiscoverReposRequest) => api<DiscoverReposResponse>('POST', '/projects/discover-repos', req ?? { source_ids: [] }),
  installTemplate: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/install-template`),
  auditInfo: (id: string) => api<{ files: { path: string; filled: boolean }[]; todos: { file: string; line: number; text: string }[] }>('GET', `/projects/${id}/audit-info`),
  validateAudit: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/validate-audit`),
  markBootstrapped: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/mark-bootstrapped`),
  cancelAudit: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/cancel-audit`),
  checkDrift: (id: string) => api<DriftCheckResponse>('GET', `/projects/${id}/drift`),
  getBriefing: (id: string) => api<string | null>('GET', `/projects/${id}/briefing`),
  setBriefing: (id: string, notes: string | null) => api<void>('PUT', `/projects/${id}/briefing`, { notes }),
  startBriefing: (id: string, agent: string) => api<{ discussion_id: string }>('POST', `/projects/${id}/start-briefing`, { agent }),
  setDefaultSkills: (id: string, skillIds: string[]) => api<boolean>('PUT', `/projects/${id}/default-skills`, skillIds),
  setDefaultProfile: (id: string, profileId: string | null) => api<boolean>('PUT', `/projects/${id}/default-profile`, { profile_id: profileId }),
  listAiFiles: (id: string) => api<AiFileNode[]>('GET', `/projects/${id}/ai-files`),
  readAiFile: (id: string, path: string) => api<AiFileContent>('GET', `/projects/${id}/ai-file?path=${encodeURIComponent(path)}`),
  searchAiFiles: (id: string, q: string) => api<AiSearchResult[]>('GET', `/projects/${id}/ai-search?q=${encodeURIComponent(q)}`),
  gitStatus: (id: string) => api<{ branch: string; default_branch: string; is_default_branch: boolean; files: { path: string; status: string; staged: boolean }[]; ahead: number; behind: number; has_upstream: boolean; provider: string; pr_url?: string | null }>('GET', `/projects/${id}/git-status`),
  gitDiff: (id: string, path: string) => api<{ path: string; diff: string }>('GET', `/projects/${id}/git-diff?path=${encodeURIComponent(path)}`),
  gitCreateBranch: (id: string, req: { name: string }) => api<{ branch: string }>('POST', `/projects/${id}/git-branch`, req),
  gitCommit: (id: string, req: { files: string[]; message: string; amend?: boolean; sign?: boolean }) => api<{ hash: string; message: string }>('POST', `/projects/${id}/git-commit`, req),
  gitPush: (id: string) => api<{ success: boolean; message: string }>('POST', `/projects/${id}/git-push`, {}),
  createPr: (id: string, req: { title: string; body?: string; base?: string }) => api<{ url: string }>('POST', `/projects/${id}/git-pr`, req),
  prTemplate: (id: string) => api<{ template: string; source: string }>('GET', `/projects/${id}/pr-template`),
  exec: (id: string, command: string) => api<{ stdout: string; stderr: string; exit_code: number }>('POST', `/projects/${id}/exec`, { command }),
  remapPath: (id: string, path: string) => api<void>('POST', `/projects/${id}/remap-path`, { path }),

  /** Stream the AI audit progress via SSE */
  auditStream: async (
    id: string,
    req: LaunchAuditRequest,
    handlers: {
      onStepStart: (step: number, total: number, file: string) => void;
      onChunk: (text: string, step: number) => void;
      onStepDone: (step: number, success: boolean) => void;
      onDone: () => void;
      onError: (error: string) => void;
    },
    signal?: AbortSignal,
  ) => {
    let finished = false;
    const done = () => { if (!finished) { finished = true; handlers.onDone(); } };

    await fetchAndParseSSE(
      `${_apiBase}/api/projects/${id}/ai-audit`,
      { method: 'POST', headers: { 'Content-Type': 'application/json', ...authHeaders() }, body: JSON.stringify(req), signal },
      {
        onEvent: (type, p: any) => {
          switch (type) {
            case 'step_start': handlers.onStepStart(p.step, p.total, p.file); break;
            case 'chunk': handlers.onChunk(p.text, p.step); break;
            case 'step_done': handlers.onStepDone(p.step, p.success); break;
            case 'step_error': handlers.onError(p.error ?? 'Step error'); break;
            case 'done': done(); break;
            case 'error': handlers.onError(p.error ?? 'Unknown error'); break;
          }
        },
        onDone: done,
        onError: handlers.onError,
      },
    );
  },
  /** Stream a partial re-audit for stale sections via SSE */
  partialAuditStream: async (
    id: string,
    req: { agent: AgentType; steps: number[] },
    handlers: {
      onStepStart: (step: number, total: number, file: string) => void;
      onChunk: (text: string, step: number) => void;
      onStepDone: (step: number, success: boolean) => void;
      onDone: () => void;
      onError: (error: string) => void;
    },
    signal?: AbortSignal,
  ) => {
    let finished = false;
    const done = () => { if (!finished) { finished = true; handlers.onDone(); } };

    await fetchAndParseSSE(
      `${_apiBase}/api/projects/${id}/partial-audit`,
      { method: 'POST', headers: { 'Content-Type': 'application/json', ...authHeaders() }, body: JSON.stringify(req), signal },
      {
        onEvent: (type, p: any) => {
          switch (type) {
            case 'step_start': handlers.onStepStart(p.step, p.total, p.file); break;
            case 'chunk': handlers.onChunk(p.text, p.step); break;
            case 'step_done': handlers.onStepDone(p.step, p.success); break;
            case 'step_error': handlers.onError(p.error ?? 'Step error'); break;
            case 'done': done(); break;
            case 'error': handlers.onError(p.error ?? 'Unknown error'); break;
          }
        },
        onDone: done,
        onError: handlers.onError,
      },
    );
  },
  /** Stream the full audit (template + audit + validation discussion) via SSE */
  fullAuditStream: async (
    id: string,
    req: LaunchAuditRequest,
    handlers: {
      onTemplateInstalled: (installed: boolean) => void;
      onStepStart: (step: number, total: number, file: string) => void;
      onChunk: (text: string, step: number) => void;
      onStepDone: (step: number, success: boolean) => void;
      onValidationCreated: (discussionId: string) => void;
      onDone: (discussionId: string | null, templateWasInstalled: boolean) => void;
      onError: (error: string) => void;
    },
    signal?: AbortSignal,
  ) => {
    let finished = false;
    const done = (discId: string | null, tmpl: boolean) => {
      if (!finished) { finished = true; handlers.onDone(discId, tmpl); }
    };

    await fetchAndParseSSE(
      `${_apiBase}/api/projects/${id}/full-audit`,
      { method: 'POST', headers: { 'Content-Type': 'application/json', ...authHeaders() }, body: JSON.stringify(req), signal },
      {
        onEvent: (type, p: any) => {
          switch (type) {
            case 'template_installed': handlers.onTemplateInstalled(p.installed); break;
            case 'step_start': handlers.onStepStart(p.step, p.total, p.file); break;
            case 'chunk': handlers.onChunk(p.text, p.step); break;
            case 'step_done': handlers.onStepDone(p.step, p.success); break;
            case 'step_error': handlers.onError(p.error ?? 'Step error'); break;
            case 'validation_created': handlers.onValidationCreated(p.discussion_id); break;
            case 'done': done(p.discussion_id ?? null, p.template_was_installed ?? false); break;
            case 'error': handlers.onError(p.error ?? 'Unknown error'); break;
          }
        },
        onDone: () => done(null, false),
        onError: handlers.onError,
      },
    );
  },
};

// ─── Agents ─────────────────────────────────────────────────────────────────

export const agents = {
  detect: () => api<AgentDetection[]>('GET', '/agents'),
  install: (agentType: AgentType) => api<string>('POST', '/agents/install', agentType),
  uninstall: (agentType: AgentType) => api<string>('POST', '/agents/uninstall', agentType),
  toggle: (agentType: AgentType) => api<boolean>('POST', '/agents/toggle', agentType),
};

// ─── MCPs ───────────────────────────────────────────────────────────────────

export const mcps = {
  overview: () => api<McpOverview>('GET', '/mcps'),
  registry: (q?: string) => api<McpDefinition[]>('GET', `/mcps/registry${q ? `?q=${encodeURIComponent(q)}` : ''}`),
  refresh: () => api<McpOverview>('POST', '/mcps/refresh'),
  createConfig: (req: CreateMcpConfigRequest) => api<McpConfigDisplay>('POST', '/mcps/configs', req),
  updateConfig: (id: string, req: UpdateMcpConfigRequest) => api<McpConfigDisplay>('PATCH', `/mcps/configs/${id}`, req),
  deleteConfig: (id: string) => api<void>('DELETE', `/mcps/configs/${id}`),
  setConfigProjects: (id: string, req: LinkMcpConfigRequest) => api<void>('PATCH', `/mcps/configs/${id}/projects`, req),
  revealSecrets: (id: string) => api<McpEnvEntry[]>('POST', `/mcps/configs/${id}/reveal`),
  // MCP context files
  listContexts: (projectId: string) => api<McpContextEntry[]>('GET', `/mcps/context/${projectId}`),
  getContext: (projectId: string, slug: string) => api<McpContextEntry>('GET', `/mcps/context/${projectId}/${slug}`),
  updateContext: (projectId: string, slug: string, content: string) => api<void>('PUT', `/mcps/context/${projectId}/${slug}`, { content }),
};

// ─── Discussions ────────────────────────────────────────────────────────────

export const discussions = {
  list: () => api<Discussion[]>('GET', '/discussions'),
  get: (id: string) => api<Discussion>('GET', `/discussions/${id}`),
  create: (req: CreateDiscussionRequest) => api<Discussion>('POST', '/discussions', req),
  delete: (id: string) => api<void>('DELETE', `/discussions/${id}`),
  update: (id: string, body: { title?: string; archived?: boolean; skill_ids?: string[]; profile_ids?: string[]; directive_ids?: string[]; project_id?: string | null; tier?: ModelTier; agent?: AgentType }) => api<void>('PATCH', `/discussions/${id}`, body),
  share: (id: string, contactIds: string[]) => api<string>('POST', `/discussions/${id}/share`, { contact_ids: contactIds }),
  /** Abort the currently-running agent on this discussion. Backend kills the
   *  child process and saves a partial response with an "⏹️ Interrompu" footer. */
  stop: (id: string) => api<{ cancelled: boolean }>('POST', `/discussions/${id}/stop`, {}),
  /** Force-recover a pending partial_response (the recovered Agent message
   *  is inserted with the in-flight start timestamp). Used by the "Dismiss
   *  partial" CTA when the user wants to retype on a still-recovering disc
   *  without waiting for the WS event. */
  dismissPartial: (id: string) =>
    api<{ recovered: boolean }>('POST', `/discussions/${id}/dismiss-partial`, {}),

  /** Stream SSE helper shared by sendMessage and run. */
  _streamSSE: async (
    url: string,
    body: unknown | null,
    onChunk: (text: string) => void,
    onDone: () => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
    onStart?: () => void,
    onLog?: (text: string) => void,
  ) => {
    let finished = false;
    const done = () => { if (!finished) { finished = true; onDone(); } };

    const hdrs: Record<string, string> = { ...authHeaders() };
    if (body) hdrs['Content-Type'] = 'application/json';

    await fetchAndParseSSE(
      url,
      { method: 'POST', headers: hdrs, body: body ? JSON.stringify(body) : undefined, signal },
      {
        onEvent: (type, parsed: any) => {
          if (type === 'chunk' && parsed.text !== undefined) {
            onChunk(parsed.text);
          } else if (type === 'log' && parsed.text !== undefined) {
            if (onLog) onLog(parsed.text);
          } else if (type === 'done') {
            done();
          } else if (type === 'error') {
            onError(parsed.error ?? 'Unknown error');
          }
        },
        onDone: done,
        onError,
      },
      // Response received — backend has processed the request (user message added)
      () => { if (onStart) onStart(); },
    );
  },

  // ── Discussion-scoped git operations ──
  gitStatus: (id: string) => api<{ branch: string; default_branch: string; is_default_branch: boolean; files: { path: string; status: string; staged: boolean }[]; ahead: number; behind: number; has_upstream: boolean; provider: string; pr_url?: string | null }>('GET', `/discussions/${id}/git-status`),
  gitDiff: (id: string, path: string) => api<{ path: string; diff: string }>('GET', `/discussions/${id}/git-diff?path=${encodeURIComponent(path)}`),
  gitCommit: (id: string, req: { files: string[]; message: string; amend?: boolean; sign?: boolean }) => api<{ hash: string; message: string }>('POST', `/discussions/${id}/git-commit`, req),
  gitPush: (id: string) => api<{ success: boolean; message: string }>('POST', `/discussions/${id}/git-push`, {}),
  createPr: (id: string, req: { title: string; body?: string; base?: string }) => api<{ url: string }>('POST', `/discussions/${id}/git-pr`, req),
  prTemplate: (id: string) => api<{ template: string; source: string }>('GET', `/discussions/${id}/pr-template`),
  exec: (id: string, command: string) => api<{ stdout: string; stderr: string; exit_code: number }>('POST', `/discussions/${id}/exec`, { command }),
  worktreeUnlock: (id: string) => api<string>('POST', `/discussions/${id}/worktree-unlock`, {}),
  worktreeLock: (id: string) => api<string>('POST', `/discussions/${id}/worktree-lock`, {}),

  // ── Context Files ──
  listContextFiles: (id: string) => api<ContextFile[]>('GET', `/discussions/${id}/context-files`),
  deleteContextFile: (id: string, fileId: string) => api<void>('DELETE', `/discussions/${id}/context-files/${fileId}`),
  uploadContextFile: async (id: string, file: File): Promise<UploadContextFileResponse> => {
    const form = new FormData();
    form.append('file', file);
    const res = await fetch(`${_apiBase}/api/discussions/${id}/context-files`, {
      method: 'POST',
      headers: { ...authHeaders() },
      body: form,
    });
    const json = await res.json();
    if (!json.success) throw new Error(json.error ?? 'Upload failed');
    return json.data;
  },

  /** Delete trailing agent/system messages (for retry/edit). */
  deleteLastAgentMessages: (id: string) => api<void>('DELETE', `/discussions/${id}/messages/last`),

  /** Edit the last user message content. */
  editLastUserMessage: (id: string, content: string) => api<void>('PATCH', `/discussions/${id}/messages/last`, { content } as SendMessageRequest),

  /** Send a user message then stream the agent response. */
  sendMessageStream: (
    id: string,
    req: SendMessageRequest,
    onChunk: (text: string) => void,
    onDone: () => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
    onStart?: () => void,
    onLog?: (text: string) => void,
  ) => discussions._streamSSE(`${_apiBase}/api/discussions/${id}/messages`, req, onChunk, onDone, onError, signal, onStart, onLog),

  /** Trigger agent on existing messages (used after create). */
  runAgent: (
    id: string,
    onChunk: (text: string) => void,
    onDone: () => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
    onLog?: (text: string) => void,
  ) => discussions._streamSSE(`${_apiBase}/api/discussions/${id}/run`, null, onChunk, onDone, onError, signal, undefined, onLog),

  /** Launch multi-agent orchestration debate. */
  orchestrate: async (
    id: string,
    req: OrchestrationRequest,
    handlers: {
      onSystem: (text: string) => void;
      onRound: (round: number, total: number) => void;
      onAgentStart: (agent: string, agentType: string, round: number | string) => void;
      onChunk: (text: string, agent: string, agentType: string, round: number | string) => void;
      onAgentDone: (agent: string, agentType: string, round: number | string) => void;
      onDone: () => void;
      onError: (error: string) => void;
    },
    signal?: AbortSignal,
  ) => {
    let finished = false;
    const done = () => { if (!finished) { finished = true; handlers.onDone(); } };

    const res = await fetch(`${_apiBase}/api/discussions/${id}/orchestrate`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...authHeaders() },
      body: JSON.stringify(req),
      signal,
    }).catch(e => {
      if (e.name === 'AbortError') { done(); return null; }
      throw e;
    });
    if (!res) return;

    if (!res.ok || !res.body) {
      handlers.onError(`HTTP ${res.status}`);
      return;
    }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    try {
      while (true) {
        const { done: streamDone, value } = await reader.read();
        if (streamDone) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() ?? '';

        let eventType = '';
        for (const line of lines) {
          if (line.startsWith('event:')) {
            eventType = line.slice(6).trim();
          } else if (line.startsWith('data:')) {
            const data = line.slice(5).trim();
            try {
              const p = JSON.parse(data);
              switch (eventType) {
                case 'system': handlers.onSystem(p.text); break;
                case 'round': handlers.onRound(p.round, p.total); break;
                case 'agent_start': handlers.onAgentStart(p.agent, p.agent_type, p.round); break;
                case 'chunk': handlers.onChunk(p.text, p.agent, p.agent_type, p.round); break;
                case 'agent_done': handlers.onAgentDone(p.agent, p.agent_type, p.round); break;
                case 'done': done(); break;
                case 'error': handlers.onError(p.error ?? 'Unknown error'); break;
              }
            } catch { /* ignore non-JSON */ }
          }
        }
      }
    } catch (e: unknown) {
      if (e instanceof DOMException && e.name === 'AbortError') { done(); return; }
      throw e;
    }

    done();
  },
};

// ─── Workflows ─────────────────────────────────────────────────────────────

export const workflows = {
  list: () => api<WorkflowSummary[]>('GET', '/workflows'),
  get: (id: string) => api<Workflow>('GET', `/workflows/${id}`),
  create: (req: CreateWorkflowRequest) => api<Workflow>('POST', '/workflows', req),
  update: (id: string, req: UpdateWorkflowRequest) => api<Workflow>('PUT', `/workflows/${id}`, req),
  delete: (id: string) => api<void>('DELETE', `/workflows/${id}`),
  trigger: (id: string) => api<WorkflowRun>('POST', `/workflows/${id}/trigger`),

  /** Trigger with SSE streaming for real-time progress. */
  triggerStream: async (
    id: string,
    onStepStart: (data: { step_name: string; step_index: number; total_steps: number }) => void,
    onStepDone: (data: StepResult) => void,
    onRunDone: (data: { status: string }) => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
  ) => {
    const res = await fetch(`${_apiBase}/api/workflows/${id}/trigger`, {
      method: 'POST',
      headers: authHeaders(),
      signal,
    }).catch(e => {
      if (e.name !== 'AbortError') onError(String(e));
      return null;
    });
    if (!res) return;
    if (!res.ok || !res.body) { onError(`HTTP ${res.status}`); return; }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() ?? '';

        let eventType = '';
        for (const line of lines) {
          if (line.startsWith('event:')) {
            eventType = line.slice(6).trim();
          } else if (line.startsWith('data:')) {
            const data = line.slice(5).trim();
            try {
              const parsed = JSON.parse(data);
              if (eventType === 'step_start') onStepStart(parsed);
              else if (eventType === 'step_done') onStepDone(parsed);
              else if (eventType === 'run_done') onRunDone(parsed);
              else if (eventType === 'error') onError(parsed.error ?? 'Unknown error');
            } catch { /* ignore */ }
          }
        }
      }
    } catch (e: unknown) {
      if (e instanceof DOMException && e.name === 'AbortError') return;
      onError(String(e));
    }
  },

  listRuns: (id: string) => api<WorkflowRun[]>('GET', `/workflows/${id}/runs`),
  getRun: (id: string, runId: string) => api<WorkflowRun>('GET', `/workflows/${id}/runs/${runId}`),
  deleteRun: (id: string, runId: string) => api<void>('DELETE', `/workflows/${id}/runs/${runId}`),
  deleteAllRuns: (id: string) => api<void>('DELETE', `/workflows/${id}/runs`),
  /** Stop a Running workflow run. Cascades to child batch discs (each running
   *  agent gets its own cancel token triggered). Idempotent. */
  cancelRun: (id: string, runId: string) =>
    api<{ run_cancelled: boolean; child_discs_cancelled: number }>(
      'POST', `/workflows/${id}/runs/${runId}/cancel`, {}
    ),
  /** Dry-run preview of a BatchQuickPrompt step: returns parsed items, sample
   *  rendered prompt, QP info, errors. NO discussion is created. */
  testBatchStep: (req: { step: WorkflowStep; mock_previous_output?: string | null; previous_step_name?: string | null }) =>
    api<BatchPreview>('POST', '/workflows/test-batch-step', req),
  suggestions: (projectId: string) => api<WorkflowSuggestion[]>('GET', `/projects/${projectId}/workflow-suggestions`),
  /** Batch runs with parent workflow meta (name + run sequence) — feeds the
   *  sidebar pastille that jumps from a batch group back to its spawning workflow. */
  listBatchRunSummaries: () => api<BatchRunSummary[]>('GET', '/workflow-runs/batch-summaries'),
  /** Delete a batch workflow run AND all its child discussions atomically.
   *  Returns the count of discussions actually deleted. */
  deleteBatchRun: (runId: string) =>
    api<{ run_id: string; discussions_deleted: number }>('DELETE', `/workflow-runs/${runId}`),

  /** Test a single step with mock context (SSE streaming, dry-run by default) */
  testStepStream: async (
    req: TestStepRequest,
    onStepStart: (data: { step_name: string; step_index: number; total_steps: number }) => void,
    onStepDone: (data: StepResult) => void,
    onRunDone: (data: { status: string }) => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
    onProgress?: (text: string) => void,
  ) => {
    const res = await fetch(`${_apiBase}/api/workflows/test-step`, {
      method: 'POST',
      headers: { ...authHeaders(), 'Content-Type': 'application/json' },
      body: JSON.stringify(req),
      signal,
    }).catch(e => {
      if (e.name !== 'AbortError') onError(String(e));
      return null;
    });
    if (!res) return;
    if (!res.ok || !res.body) { onError(`HTTP ${res.status}`); return; }

    await parseSSEStream(res, {
      onEvent: (type, data) => {
        if (type === 'step_start') onStepStart(data as { step_name: string; step_index: number; total_steps: number });
        else if (type === 'step_progress' && onProgress) onProgress((data as { text: string }).text);
        else if (type === 'step_done') onStepDone(data as StepResult);
        else if (type === 'run_done') onRunDone(data as { status: string });
        else if (type === 'error') onError((data as { error: string }).error ?? 'Unknown error');
      },
      onDone: () => {},
      onError,
    });
  },
};

// ─── Quick Prompts ─────────────────────────────────────────────────────────

export interface BatchItem {
  title: string;
  prompt: string;
}

export interface BatchPreview {
  sample_items: string[];
  total_items: number;
  capped_at: number;
  max_items_allowed: number;
  quick_prompt_id: string | null;
  quick_prompt_name: string | null;
  quick_prompt_icon: string | null;
  quick_prompt_agent: string | null;
  first_variable_name: string | null;
  /** First sample item's rendered prompt — kept for backward compat.
   *  Prefer `sample_rendered_prompts` for the per-item view. */
  sample_rendered_prompt: string | null;
  /** Rendered prompt for each sample item (same length & order as sample_items). */
  sample_rendered_prompts: string[];
  workspace_mode: string;
  wait_for_completion: boolean;
  errors: string[];
  /** Non-blocking warnings: dry-run succeeded but config would bite at runtime
   *  (e.g. {{steps.X.data}} against a FreeText step). Shown in orange. */
  warnings: string[];
}

export interface BatchRunRequest {
  items: BatchItem[];
  batch_name: string;
  project_id?: string | null;
  /** "Direct" (default) or "Isolated" — per-child git worktree. */
  workspace_mode?: string | null;
}

export interface BatchRunResponse {
  run_id: string;
  discussion_ids: string[];
  batch_total: number;
}

export const quickPrompts = {
  list: () => api<QuickPrompt[]>('GET', '/quick-prompts'),
  create: (req: CreateQuickPromptRequest) => api<QuickPrompt>('POST', '/quick-prompts', req),
  update: (id: string, req: CreateQuickPromptRequest) => api<QuickPrompt>('PUT', `/quick-prompts/${id}`, req),
  delete: (id: string) => api<void>('DELETE', `/quick-prompts/${id}`),
  /**
   * Create N child discussions from a Quick Prompt + list of rendered prompts.
   * The frontend pre-renders each template (via the existing renderTemplate
   * helper) and posts a flat list of {title, prompt} tuples. Returns the
   * batch run id + the list of child discussion ids — the caller then fires
   * `POST /discussions/:id/run` on each one to start the agents.
   */
  batchRun: (qpId: string, req: BatchRunRequest) =>
    api<BatchRunResponse>('POST', `/quick-prompts/${qpId}/batch`, req),
};

// ─── Skills ─────────────────────────────────────────────────────────────────

export const skills = {
  list: () => api<Skill[]>('GET', '/skills'),
  create: (req: CreateSkillRequest) => api<Skill>('POST', '/skills', req),
  update: (id: string, req: CreateSkillRequest) => api<Skill>('PUT', `/skills/${id}`, req),
  delete: (id: string) => api<boolean>('DELETE', `/skills/${id}`),
};

// ─── Profiles ────────────────────────────────────────────────────────────────

export const profiles = {
  list: () => api<AgentProfile[]>('GET', '/profiles'),
  get: (id: string) => api<AgentProfile>('GET', `/profiles/${id}`),
  create: (req: CreateProfileRequest) => api<AgentProfile>('POST', '/profiles', req),
  update: (id: string, req: CreateProfileRequest) => api<AgentProfile>('PUT', `/profiles/${id}`, req),
  delete: (id: string) => api<boolean>('DELETE', `/profiles/${id}`),
  updatePersonaName: (id: string, personaName: string) => api<AgentProfile>('PUT', `/profiles/${id}/persona-name`, { persona_name: personaName }),
};

// ─── Directives ─────────────────────────────────────────────────────────────

export const directives = {
  list: () => api<Directive[]>('GET', '/directives'),
  create: (req: CreateDirectiveRequest) => api<Directive>('POST', '/directives', req),
  update: (id: string, req: CreateDirectiveRequest) => api<Directive>('PUT', `/directives/${id}`, req),
  delete: (id: string) => api<boolean>('DELETE', `/directives/${id}`),
};

// ─── Stats ──────────────────────────────────────────────────────────────────

export const stats = {
  tokenUsage: () => api<TokenUsageSummary>('GET', '/stats/tokens'),
  agentUsage: () => api<AgentUsageSummary[]>('GET', '/stats/agent-usage'),
};
