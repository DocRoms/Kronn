import type {
  SetupStatus,
  SetScanPathsRequest,
  SaveTokensRequest,
  Project,
  DetectedRepo,
  McpDefinition,
  McpOverview,
  McpConfigDisplay,
  McpEnvEntry,
  CreateMcpConfigRequest,
  UpdateMcpConfigRequest,
  LinkMcpConfigRequest,
  ScheduledTask,
  CreateTaskRequest,
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
  Workflow,
  WorkflowSummary,
  WorkflowRun,
  StepResult,
  CreateWorkflowRequest,
  UpdateWorkflowRequest,
} from '../types/generated';

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
  const res = await fetch(`/api${path}`, {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : {},
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
  getTokens: () => api<SaveTokensRequest>('GET', '/config/tokens'),
  saveTokens: (req: SaveTokensRequest) => api<void>('POST', '/config/tokens', req),
  getLanguage: () => api<string>('GET', '/config/language'),
  saveLanguage: (lang: string) => api<void>('POST', '/config/language', lang),
  getAgentAccess: () => api<AgentsConfig>('GET', '/config/agent-access'),
  setAgentAccess: (req: SetAgentAccessRequest) => api<void>('POST', '/config/agent-access', req),
  dbInfo: () => api<DbInfo>('GET', '/config/db-info'),
  exportData: () => api<DbExport>('GET', '/config/export'),
  importData: (data: DbExport) => api<void>('POST', '/config/import', data),
};

// ─── Projects ───────────────────────────────────────────────────────────────

export const projects = {
  list: () => api<Project[]>('GET', '/projects'),
  get: (id: string) => api<Project>('GET', `/projects/${id}`),
  scan: () => api<DetectedRepo[]>('POST', '/projects/scan'),
  create: (repo: DetectedRepo) => api<Project>('POST', '/projects', repo),
  delete: (id: string) => api<void>('DELETE', `/projects/${id}`),
  installTemplate: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/install-template`),
  validateAudit: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/validate-audit`),

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
      headers: { 'Content-Type': 'application/json' },
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
};

// ─── Agents ─────────────────────────────────────────────────────────────────

export const agents = {
  detect: () => api<AgentDetection[]>('GET', '/agents'),
  install: (agentType: AgentType) => api<string>('POST', '/agents/install', agentType),
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

// ─── Tasks ──────────────────────────────────────────────────────────────────

export const tasks = {
  list: (projectId: string) => api<ScheduledTask[]>('GET', `/projects/${projectId}/tasks`),
  create: (projectId: string, req: CreateTaskRequest) => api<ScheduledTask>('POST', `/projects/${projectId}/tasks`, req),
  delete: (projectId: string, taskId: string) => api<void>('DELETE', `/projects/${projectId}/tasks/${taskId}`),
  toggle: (projectId: string, taskId: string) => api<boolean>('PATCH', `/projects/${projectId}/tasks/${taskId}/toggle`),
};

// ─── Discussions ────────────────────────────────────────────────────────────

export const discussions = {
  list: () => api<Discussion[]>('GET', '/discussions'),
  get: (id: string) => api<Discussion>('GET', `/discussions/${id}`),
  create: (req: CreateDiscussionRequest) => api<Discussion>('POST', '/discussions', req),
  delete: (id: string) => api<void>('DELETE', `/discussions/${id}`),

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

    const res = await fetch(url, {
      method: 'POST',
      headers: body ? { 'Content-Type': 'application/json' } : {},
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
      headers: { 'Content-Type': 'application/json' },
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

// ─── Stats ──────────────────────────────────────────────────────────────────

export const stats = {
  tokenUsage: () => api<TokenUsageSummary>('GET', '/stats/tokens'),
};
