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
  TokenUsageSummary,
  DbInfo,
  DbExport,
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
  WorkflowSummary,
  WorkflowRun,
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
} from '../types/generated';
import type { DiscoverKeysResponse } from '../types/extensions';

// ─── Auth token ──────────────────────────────────────────────────────────────

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

  const res = await fetch(`/api${path}`, {
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
  getScanPaths: () => api<string[]>('GET', '/config/scan-paths'),
  setScanPaths: (paths: string[]) => api<void>('POST', '/config/scan-paths', { paths }),
  getScanIgnore: () => api<string[]>('GET', '/config/scan-ignore'),
  setScanIgnore: (patterns: string[]) => api<void>('POST', '/config/scan-ignore', patterns),
  getScanDepth: () => api<number>('GET', '/config/scan-depth'),
  setScanDepth: (depth: number) => api<number>('POST', '/config/scan-depth', depth),
  getAgentAccess: () => api<AgentsConfig>('GET', '/config/agent-access'),
  setAgentAccess: (req: SetAgentAccessRequest) => api<void>('POST', '/config/agent-access', req),
  dbInfo: () => api<DbInfo>('GET', '/config/db-info'),
  exportData: () => api<DbExport>('GET', '/config/export'),
  importData: (data: DbExport) => api<void>('POST', '/config/import', data),
  getServerConfig: () => api<ServerConfigPublic>('GET', '/config/server'),
  setServerConfig: (req: { domain?: string; max_concurrent_agents?: number }) => api<void>('POST', '/config/server', req),
  regenerateAuthToken: () => api<string>('POST', '/config/auth-token/regenerate'),
};

// ─── Projects ───────────────────────────────────────────────────────────────

export const projects = {
  list: () => api<Project[]>('GET', '/projects'),
  get: (id: string) => api<Project>('GET', `/projects/${id}`),
  scan: () => api<DetectedRepo[]>('POST', '/projects/scan'),
  create: (repo: DetectedRepo) => api<Project>('POST', '/projects', repo),
  bootstrap: (req: BootstrapProjectRequest) => api<BootstrapProjectResponse>('POST', '/projects/bootstrap', req),
  delete: (id: string, hard?: boolean) => api<void>('DELETE', `/projects/${id}${hard ? '?hard=true' : ''}`),
  clone: (req: CloneProjectRequest) => api<CloneProjectResponse>('POST', '/projects/clone', req),
  discoverRepos: (req?: DiscoverReposRequest) => api<DiscoverReposResponse>('POST', '/projects/discover-repos', req ?? { source_ids: [] }),
  installTemplate: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/install-template`),
  auditInfo: (id: string) => api<{ files: { path: string; filled: boolean }[]; todos: { file: string; line: number; text: string }[] }>('GET', `/projects/${id}/audit-info`),
  validateAudit: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/validate-audit`),
  markBootstrapped: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/mark-bootstrapped`),
  cancelAudit: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/cancel-audit`),
  setDefaultSkills: (id: string, skillIds: string[]) => api<boolean>('PUT', `/projects/${id}/default-skills`, skillIds),
  setDefaultProfile: (id: string, profileId: string | null) => api<boolean>('PUT', `/projects/${id}/default-profile`, { profile_id: profileId }),
  listAiFiles: (id: string) => api<AiFileNode[]>('GET', `/projects/${id}/ai-files`),
  readAiFile: (id: string, path: string) => api<AiFileContent>('GET', `/projects/${id}/ai-file?path=${encodeURIComponent(path)}`),
  searchAiFiles: (id: string, q: string) => api<AiSearchResult[]>('GET', `/projects/${id}/ai-search?q=${encodeURIComponent(q)}`),
  gitStatus: (id: string) => api<{ branch: string; default_branch: string; is_default_branch: boolean; files: { path: string; status: string; staged: boolean }[]; ahead: number; behind: number }>('GET', `/projects/${id}/git-status`),
  gitDiff: (id: string, path: string) => api<{ path: string; diff: string }>('GET', `/projects/${id}/git-diff?path=${encodeURIComponent(path)}`),
  gitCreateBranch: (id: string, req: { name: string }) => api<{ branch: string }>('POST', `/projects/${id}/git-branch`, req),
  gitCommit: (id: string, req: { files: string[]; message: string }) => api<{ hash: string; message: string }>('POST', `/projects/${id}/git-commit`, req),
  gitPush: (id: string) => api<{ success: boolean; message: string }>('POST', `/projects/${id}/git-push`, {}),
  exec: (id: string, command: string) => api<{ stdout: string; stderr: string; exit_code: number }>('POST', `/projects/${id}/exec`, { command }),

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

    const res = await fetch(`/api/projects/${id}/ai-audit`, {
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
                case 'step_start': handlers.onStepStart(p.step, p.total, p.file); break;
                case 'chunk': handlers.onChunk(p.text, p.step); break;
                case 'step_done': handlers.onStepDone(p.step, p.success); break;
                case 'step_error': handlers.onError(p.error ?? 'Step error'); break;
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

    const res = await fetch(`/api/projects/${id}/full-audit`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...authHeaders() },
      body: JSON.stringify(req),
      signal,
    }).catch(e => {
      if (e.name === 'AbortError') { done(null, false); return null; }
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

    const processLines = (lines: string[], eventType: { current: string }) => {
      for (const line of lines) {
        if (line.startsWith('event:')) {
          eventType.current = line.slice(6).trim();
        } else if (line.startsWith('data:')) {
          const data = line.slice(5).trim();
          try {
            const p = JSON.parse(data);
            switch (eventType.current) {
              case 'template_installed': handlers.onTemplateInstalled(p.installed); break;
              case 'step_start': handlers.onStepStart(p.step, p.total, p.file); break;
              case 'chunk': handlers.onChunk(p.text, p.step); break;
              case 'step_done': handlers.onStepDone(p.step, p.success); break;
              case 'step_error': handlers.onError(p.error ?? 'Step error'); break;
              case 'validation_created': handlers.onValidationCreated(p.discussion_id); break;
              case 'done': done(p.discussion_id ?? null, p.template_was_installed ?? false); break;
              case 'error': handlers.onError(p.error ?? 'Unknown error'); break;
            }
          } catch { /* ignore non-JSON */ }
        }
      }
    };

    const eventType = { current: '' };
    try {
      while (true) {
        const { done: streamDone, value } = await reader.read();
        if (streamDone) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() ?? '';
        processLines(lines, eventType);
      }
      // Process any remaining data in buffer (last chunk may lack trailing newline)
      if (buffer.trim()) {
        processLines(buffer.split('\n'), eventType);
      }
    } catch (e: unknown) {
      if (e instanceof DOMException && e.name === 'AbortError') { done(null, false); return; }
      throw e;
    }

    done(null, false);
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
  update: (id: string, body: { title?: string; archived?: boolean; skill_ids?: string[]; profile_ids?: string[]; directive_ids?: string[]; project_id?: string | null }) => api<void>('PATCH', `/discussions/${id}`, body),

  /** Stream SSE helper shared by sendMessage and run. */
  _streamSSE: async (
    url: string,
    body: unknown | null,
    onChunk: (text: string) => void,
    onDone: () => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
    onStart?: () => void,
  ) => {
    let finished = false;
    const done = () => { if (!finished) { finished = true; onDone(); } };

    const hdrs: Record<string, string> = { ...authHeaders() };
    if (body) hdrs['Content-Type'] = 'application/json';

    const res = await fetch(url, {
      method: 'POST',
      headers: hdrs,
      body: body ? JSON.stringify(body) : undefined,
      signal,
    }).catch(e => {
      if (e.name === 'AbortError') { done(); return null; }
      throw e;
    });
    if (!res) return;

    // Response received — backend has processed the request (user message added)
    if (onStart) onStart();

    if (!res.ok || !res.body) {
      onError(`HTTP ${res.status}`);
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
              if (eventType === 'chunk' && parsed.text !== undefined) {
                onChunk(parsed.text);
              } else if (eventType === 'done') {
                done();
              } else if (eventType === 'error') {
                onError(parsed.error ?? 'Unknown error');
              }
            } catch { /* ignore */ }
          }
        }
      }
    } catch (e: unknown) {
      if (e instanceof DOMException && e.name === 'AbortError') { done(); return; }
      throw e;
    }

    done();
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
  ) => discussions._streamSSE(`/api/discussions/${id}/messages`, req, onChunk, onDone, onError, signal, onStart),

  /** Trigger agent on existing messages (used after create). */
  runAgent: (
    id: string,
    onChunk: (text: string) => void,
    onDone: () => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
  ) => discussions._streamSSE(`/api/discussions/${id}/run`, null, onChunk, onDone, onError, signal),

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

    const res = await fetch(`/api/discussions/${id}/orchestrate`, {
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
    const res = await fetch(`/api/workflows/${id}/trigger`, {
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
