import type {
  SetupStatus,
  SetScanPathsRequest,
  SaveApiKeyRequest,
  ApiKeyDisplay,
  ApiKeysResponse,
  Project,
  LinkedRepo,
  DetectedRepo,
  McpDefinition,
  McpOverview,
  McpConfigDisplay,
  McpEnvEntry,
  CustomApiPayload,
  UpdateCustomSpecResponse,
  CleanupOrphanEnvResponse,
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
  DetectedIp,
  TokenUsageSummary,
  DbInfo,
  SetAgentAccessRequest,
  AgentsConfig,
  McpContextEntry,
  DiscoveredHostMcp,
  AdoptHostMcpRequest,
  AiAuditStatus,
  AuditProgress,
  LaunchAuditRequest,
  BootstrapProjectRequest,
  BootstrapProjectResponse,
  CloneProjectRequest,
  CloneProjectResponse,
  CloneAndRemapRequest,
  CloneAndRemapResponse,
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
  BundleResponse,
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
  QuickPromptVersion,
  QuickPromptVersionMetrics,
  QuickApi,
  CreateQuickApiRequest,
  RunQuickApiRequest,
  RunQuickApiResponse,
  ImportQuickApiRequest,
  BatchRunQuickApiRequest,
  BatchRunQuickApiResponse,
  OllamaHealthResponse,
  OllamaModelsResponse,
  DecideRunRequest,
  DecideRunResponse,
  ImportWorkflowRequest,
  ImportQuickPromptRequest,
  VersionCheck,
  DbBackupResponse,
  UsageReport,
  Learning,
  LearningStatus,
  LearningProposeRequest,
  ProposeResult,
} from '../types/generated';
import type { DiscoverKeysResponse, TestModeEnterResult, TestModeExitResponse } from '../types/extensions';

// ─── Auth token ──────────────────────────────────────────────────────────────

// Security note: localStorage is accessible to any JS on the page (XSS risk).
// For self-hosted/Tauri desktop deployments this is acceptable.
// For public-facing deployments, consider httpOnly cookies instead.
//
// localStorage isn't present in every environment this module is imported into
// (vitest's isolated runs before the DOM env attaches, SSR, etc.). Guard so a
// module-load read never throws — falls back to in-memory only. In a real
// browser / Tauri webview localStorage always exists, so behaviour is unchanged.
const _ls: Storage | undefined = typeof localStorage !== 'undefined' ? localStorage : undefined;

let _authToken: string | null = _ls?.getItem('kronn_auth_token') ?? null;

export function setAuthToken(token: string | null) {
  _authToken = token;
  if (token) {
    _ls?.setItem('kronn_auth_token', token);
  } else {
    _ls?.removeItem('kronn_auth_token');
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
/** Union of fields that any audit / partial-audit / full-audit SSE
 *  event might carry. Each field is optional because the same handler
 *  branches on `type` and only reads the subset relevant to that
 *  event. Using a typed union (instead of `any`) lets the callsites
 *  cast once at function entry and TS still complains if a typo
 *  invents a non-existent field. The runtime payload comes from
 *  `parseSSEStream` which JSON-parses the `data:` line — we don't
 *  validate shape, the cast trusts the backend (which we own).
 */
interface AuditSseEvent {
  step?: number;
  total?: number;
  total_steps?: number;
  file?: string;
  text?: string;
  success?: boolean;
  installed?: boolean;
  error?: string;
  discussion_id?: string;
  template_was_installed?: boolean;
  // 0.8.3 (#272) — legacy-docs migration report. Emitted ONCE in
  // Phase 1 when a user-curated docs/ was detected and moved to
  // docs/legacy/. Frontend renders a toast + the moved entries list.
  migrated?: boolean;
  skip_reason?: string;
  moved_entries?: string[];
  moved_count?: number;
  // 0.8.3 (#274) — per-step instrumentation (carried by `step_done`)
  // and audit-wide start timestamp (carried by `start`). Tokens are
  // the `.max(input + output)` for the step (Claude reports
  // cumulative usage); duration_ms is wallclock for the step;
  // total_tokens is the running sum across steps. started_at is an
  // ISO-8601 timestamp surfaced once on the `start` event so the
  // frontend can compute live elapsed without local-clock drift.
  tokens?: number;
  duration_ms?: number;
  total_tokens?: number;
  started_at?: string;
  // 0.8.3 (#281) — live step progress + tool-call events fired
  // mid-step (Claude stream-json only). step_tokens = current
  // step's max(input+output); total_tokens_so_far = running sum
  // including the current step's latest reading. tool = name of
  // the tool the agent just started calling.
  step_tokens?: number;
  total_tokens_so_far?: number;
  tool?: string;
  // 0.8.3 root-cause fix — `step_warning` is emitted when the CLI
  // exited 0 but the step's target_file is empty / suspiciously
  // small (e.g. agent crashed mid-Write). Backend auto-repairs from
  // template; frontend surfaces a per-step banner so the user knows
  // the audit "succeeded" but this step actually didn't produce
  // useful output.
  reason?: string;
  repaired_from_template?: boolean;
}

export interface LegacyDocsMigrationReport {
  migrated: boolean;
  skip_reason: string;
  moved_entries: string[];
  moved_count: number;
}

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
  // Distinguish "no body" (undefined — e.g. GET, or POST with no payload)
  // from a falsy-but-real body. `if (body)` dropped `false`/`0`/`""`, so a
  // `Json<bool>` endpoint received an empty body and 422'd — that's why
  // DISABLING the continual-learning toggle (POST `false`) failed while
  // enabling (POST `true`) worked. Only `undefined` means "send nothing".
  const hasBody = body !== undefined;
  if (hasBody) headers['Content-Type'] = 'application/json';

  const res = await fetch(`${_apiBase}/api${path}`, {
    method,
    headers,
    body: hasBody ? JSON.stringify(body) : undefined,
  });

  const contentType = res.headers.get('content-type') ?? '';
  if (!contentType.includes('application/json')) {
    // 0.8.5 — when axum's `Json<T>` extractor rejects a request
    // (missing field, unknown enum variant, type mismatch), it
    // returns 422 with `Content-Type: text/plain` and the actual
    // deserialization failure in the body. Pre-fix we threw away
    // the body and surfaced a bare "Server error (HTTP 422)" with
    // zero actionable info — exactly what tripped the QP-Improver
    // agent on the JIRA helper during 0.8.4 dogfooding. Same path
    // also covers gateway-style 5xx HTML bodies; we cap at 500
    // chars so a 10MB nginx error page doesn't drown the toast.
    const body = await res.text().catch(() => '');
    const trimmed = body.trim();
    const suffix = trimmed ? ` — ${trimmed.slice(0, 500)}` : '';
    throw new Error(`Server error (HTTP ${res.status})${suffix}`);
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

export const version = {
  /** `GET /api/version/check` — current+latest pair for the auto-update
   *  banner. Backend caches 6h so a UI tab burst doesn't fan out to
   *  GitHub; safe to call from `useEffect` on every mount. */
  check: () => api<VersionCheck>('GET', '/version/check'),
};

export interface HealthInfo {
  ok: boolean;
  version: string;
  host_os: string;
  /** True when the backend runs inside the Docker container. The UI uses it
   *  to gate the agent Install button: under Docker an install lands in the
   *  container (not the host), so the UI points to the host-side `kronn` CLI
   *  instead. Native (Tauri/CLI) → false → Install works on the host. */
  in_docker: boolean;
}

export const health = {
  /** `GET /api/health` — unauthed and NOT enveloped (raw JSON), so it bypasses
   *  the `api<T>()` `{success,data}` unwrap. */
  get: async (): Promise<HealthInfo> => {
    const res = await fetch(`${_apiBase}/api/health`, { headers: { ...authHeaders() } });
    return res.json() as Promise<HealthInfo>;
  },
};

// ─── Config ─────────────────────────────────────────────────────────────────

/** LAN/Tailscale exposure state (the "Allow connections from other devices"
 *  toggle). Mirrors the backend `NetworkExposure` (api/setup.rs) — defined here
 *  rather than generated because the struct carries a qualified-path field that
 *  ts-rs skips. */
export interface NetworkExposure {
  exposed: boolean;
  restart_required: boolean;
  port: number;
  reachable_ips: DetectedIp[];
}

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
  /** 0.8.7 anti-hallucination mode: "off" | "warn" | "enforce". */
  getAntiHallucinationMode: () => api<string>('GET', '/config/anti-hallucination-mode'),
  saveAntiHallucinationMode: (mode: string) => api<void>('POST', '/config/anti-hallucination-mode', mode),
  getContinualLearningEnabled: () => api<boolean>('GET', '/config/continual-learning-enabled'),
  saveContinualLearningEnabled: (enabled: boolean) =>
    api<void>('POST', '/config/continual-learning-enabled', enabled),
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
  /** SQLite online-backup snapshot. Backend writes to
   *  `<data_dir>/backups/kronn-YYYYMMDD-HHMMSS.db`. Returns the
   *  resulting path so the Settings UI can toast it. */
  dbBackup: () => api<DbBackupResponse>('POST', '/db/backup'),
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
  getNetworkExposure: () => api<NetworkExposure>('GET', '/config/network-exposure'),
  setNetworkExposure: (exposed: boolean) => api<NetworkExposure>('POST', '/config/network-exposure', { exposed }),
  /** P2 recovery passphrase — the encryption key wrapped under an Argon2id
   *  passphrase, so MCP secrets survive total machine/keychain loss. */
  getRecoveryStatus: () => api<{ configured: boolean }>('GET', '/config/recovery/status'),
  /** Returns the recovery code the user MUST save off-machine. */
  setRecovery: (passphrase: string) => api<{ recovery_code: string }>('POST', '/config/recovery/set', { passphrase }),
  /** Restores the encryption key when the token subsystem is locked. `recoveryCode`
   *  optional — omitted, the local recovery sidecar is used. */
  restoreRecovery: (passphrase: string, recoveryCode?: string) =>
    api<void>('POST', '/config/recovery/restore', { passphrase, recovery_code: recoveryCode || null }),
  getServerConfig: () => api<ServerConfigPublic>('GET', '/config/server'),
  setServerConfig: (req: { domain?: string; max_concurrent_agents?: number; agent_stall_timeout_min?: number; pseudo?: string; avatar_email?: string; bio?: string; debug_mode?: boolean; default_model_tier?: 'economy' | 'default' | 'reasoning'; default_summary_strategy?: 'Auto' | 'OnDemand' | 'Off' }) => api<void>('POST', '/config/server', req),
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

/** Response shape of `POST /api/projects/:id/migrate-docs`. Backend
 *  returns one of NotApplicable / AlreadyMigrated / Migrated / Failed,
 *  with optional counters (files moved, refs rewritten, symlink). */
export interface MigrateDocsResponse {
  status: 'NotApplicable' | 'AlreadyMigrated' | 'Migrated' | 'Failed';
  files_moved?: number;
  refs_rewritten?: number;
  symlink_created?: boolean;
  reason?: string;
}

export const projects = {
  list: () => api<Project[]>('GET', '/projects'),
  get: (id: string) => api<Project>('GET', `/projects/${id}`),
  scan: () => api<DetectedRepo[]>('POST', '/projects/scan'),
  create: (repo: DetectedRepo) => api<Project>('POST', '/projects', repo),
  addFolder: (req: { path: string; name?: string }) => api<Project>('POST', '/projects/add-folder', req),
  bootstrap: (req: BootstrapProjectRequest) => api<BootstrapProjectResponse>('POST', '/projects/bootstrap', req),
  /** Migrate the project's legacy `ai/` directory to the new `docs/`
   *  convention (`ai/index.md` → `docs/AGENTS.md`, internal refs rewritten,
   *  optional symlink for retro-compat). Idempotent — re-running on an
   *  already-migrated project returns `status: "AlreadyMigrated"`. */
  migrateDocs: (id: string, req: { create_symlink?: boolean }) =>
    api<MigrateDocsResponse>('POST', `/projects/${id}/migrate-docs`, req),
  delete: (id: string, hard?: boolean) => api<void>('DELETE', `/projects/${id}${hard ? '?hard=true' : ''}`),
  clone: (req: CloneProjectRequest) => api<CloneProjectResponse>('POST', '/projects/clone', req),
  discoverRepos: (req?: DiscoverReposRequest) => api<DiscoverReposResponse>('POST', '/projects/discover-repos', req ?? { source_ids: [] }),
  installTemplate: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/install-template`),
  /** 0.8.7 — check whether the anti-hallu canonical section is present
   *  in `docs/AGENTS.md`. Frontend uses this to decide between the
   *  "✓ Anti-hallu v1" badge (present) and the "⚠ inject" CTA. */
  antiHalluStatus: (id: string) =>
    api<{ present: boolean; audit_date?: string | null; file_exists: boolean }>(
      'GET',
      `/projects/${id}/anti-hallu/status`,
    ),
  /** 0.8.7 — inject the anti-hallu canonical section into `docs/AGENTS.md`.
   *  Idempotent. Returns `result: "inserted" | "refreshed" | "noop" | "missing"`. */
  injectAntiHallu: (id: string) =>
    api<{ status: 'ok' | 'error'; result: 'inserted' | 'refreshed' | 'noop' | 'missing'; error?: string }>(
      'POST',
      `/projects/${id}/anti-hallu/inject`,
    ),
  /** 0.8.7 — re-sync the redirector files (CLAUDE.md, GEMINI.md, …) from
   *  the binary templates into the project. Idempotent : already-present
   *  files are NOT overwritten. */
  syncRedirectors: (id: string) =>
    api<{ status: 'ok' | 'partial'; created: string[]; already_present: string[]; failed: string[] }>(
      'POST',
      `/projects/${id}/redirectors/sync`,
    ),
  auditInfo: (id: string) => api<{ files: { path: string; filled: boolean }[]; todos: { file: string; line: number; text: string }[] }>('GET', `/projects/${id}/audit-info`),
  validateAudit: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/validate-audit`),
  markBootstrapped: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/mark-bootstrapped`),
  cancelAudit: (id: string) => api<AiAuditStatus>('POST', `/projects/${id}/cancel-audit`),
  /**
   * Returns the live progress of an in-flight audit for this project, or
   * `null` when nothing is running. Polled by ProjectCard every 2 s when a
   * local checkpoint indicates an audit was in-flight before navigation —
   * the server-side process keeps running regardless of the SSE client.
   */
  auditStatus: (id: string) => api<AuditProgress | null>('GET', `/projects/${id}/audit-status`),
  /**
   * 0.8.3 (#288) — fleet-wide list of audits currently in progress
   * across every project. Powers the ActiveAuditsPopover on the
   * Projets nav button. Returns `[]` when nothing is running.
   */
  auditStatusAll: () => api<AuditProgress[]>('GET', '/audit-status'),
  /**
   * 0.8.3 (#311) — fetch the most-recent resumable audit run for the
   * project, or `null`. Resumable = status='Interrupted' AND
   * last_completed_step in 1..=9. Used by ProjectCard to flip the
   * "Lancer l'audit" button to "Reprendre Step N/10" + pass
   * `resume_from` in the launch request.
   */
  auditResumable: (id: string) =>
    api<{ id: string; last_completed_step: number; started_at: string } | null>(
      'GET', `/projects/${id}/audit-resumable`,
    ),
  /**
   * 0.8.4 (#298) — most-recent completed audit for the project, or
   * `null`. The ProjectCard recap panel uses this to find the
   * `audit_run_id` to feed `auditRunSteps`.
   */
  auditLatest: (id: string) =>
    api<{
      id: string;
      project_id: string;
      kind: string;
      agent_type: string;
      started_at: string;
      ended_at?: string | null;
      duration_ms?: number | null;
      status: string;
      td_total: number;
      health_score?: number | null;
    } | null>('GET', `/projects/${id}/audit-latest`),
  /**
   * 0.8.4 (#298) — recent audit history (Full + sub-audits combined),
   * newest first. Powers the chip strip on the recap panel so the user
   * can switch between past audits and see each one's per-step
   * breakdown. Server-capped at 20 entries.
   */
  auditHistory: (id: string) =>
    api<Array<{
      id: string;
      project_id: string;
      kind: string;
      agent_type: string;
      started_at: string;
      ended_at?: string | null;
      duration_ms?: number | null;
      status: string;
      td_total: number;
      health_score?: number | null;
    }>>('GET', `/projects/${id}/audit-history`),
  /**
   * 0.8.4 (#298) — per-step metrics for the audit recap panel.
   * Returns one row per step (1..N) with duration_ms, step_tokens,
   * cumulative_tokens, cli_success + the optional step_warning. Empty
   * Vec for legacy runs (pre-0.8.4) or runs with no recorded steps.
   */
  auditRunSteps: (runId: string) =>
    api<Array<{
      audit_run_id: string;
      step_index: number;
      file_label: string;
      started_at: string;
      ended_at?: string | null;
      duration_ms?: number | null;
      step_tokens?: number | null;
      cumulative_tokens?: number | null;
      cli_success: boolean;
      step_warning?: string | null;
      step_repaired_from_template: boolean;
    }>>('GET', `/audit-runs/${runId}/steps`),
  /**
   * 0.8.4 (#294) — cross-agent memory source bindings.
   * Returns every disc currently bound to a (source_agent, source_session_id)
   * pair. The DiscussionsPage sidebar fetches this once at mount to
   * decorate disc rows with an "imported from X" badge + drive the
   * source-filter dropdown.
   */
  discSources: () =>
    api<Array<{
      disc_id: string;
      source_agent: string;
      source_session_id: string;
      imported_at?: string | null;
      diverged_at?: string | null;
    }>>('GET', '/disc/sources'),
  /**
   * 0.8.4 (#294) — per-disc source binding + full history chain.
   * Used by ChatHeader tooltip to render "first owned by CC sess A,
   * then Cursor sess B, …".
   */
  discSourceDetail: (id: string) =>
    api<{
      current?: {
        disc_id: string;
        source_agent: string;
        source_session_id: string;
        imported_at?: string | null;
        diverged_at?: string | null;
      } | null;
      history: Array<{
        source_agent: string;
        source_session_id: string;
        linked_at: string;
        unlinked_at?: string | null;
      }>;
    }>('GET', `/discussions/${id}/source`),
  checkDrift: (id: string) => api<DriftCheckResponse>('GET', `/projects/${id}/drift`),
  getBriefing: (id: string) => api<string | null>('GET', `/projects/${id}/briefing`),
  setBriefing: (id: string, notes: string | null) => api<void>('PUT', `/projects/${id}/briefing`, { notes }),
  startBriefing: (id: string, agent: string) => api<{ discussion_id: string }>('POST', `/projects/${id}/start-briefing`, { agent }),
  /**
   * 0.8.4 (#285) — désagentified briefing. POST the 6 answers and
   * the server writes `docs/briefing.md` + persists DB notes without
   * spawning a discussion / LLM call. Coexists with `startBriefing`.
   */
  saveBriefing: (id: string, form: {
    purpose: string; team: string; maturity: string;
    dependencies: string; traps: string; additional: string;
  }) => api<boolean>('POST', `/projects/${id}/save-briefing`, form),
  setDefaultSkills: (id: string, skillIds: string[]) => api<boolean>('PUT', `/projects/${id}/default-skills`, skillIds),
  setDefaultProfile: (id: string, profileId: string | null) => api<boolean>('PUT', `/projects/${id}/default-profile`, { profile_id: profileId }),
  /** 0.8.3 — Replace the project's linked_repos list. The backend
   *  validates kind ∈ {api, iac, design, shared-lib, docs, other},
   *  non-empty name + location, max 20 entries. Atomic replace —
   *  no per-row CRUD. */
  setLinkedRepos: (id: string, repos: LinkedRepo[]) => api<boolean>('PUT', `/projects/${id}/linked-repos`, repos),
  /** 0.8.6 (#27) — autocomplete picker source. Returns OTHER
   *  Kronn-known projects (excluding the current one) sorted by
   *  proximity (same-parent dir first, then alphabetical). Free-text
   *  location entry stays supported for off-Kronn repos. */
  linkedReposCandidates: (id: string) =>
    api<Array<{ id: string; name: string; path: string; proximity_hint: string }>>('GET', `/projects/${id}/linked-repos/candidates`),
  listAiFiles: (id: string) => api<AiFileNode[]>('GET', `/projects/${id}/ai-files`),
  readAiFile: (id: string, path: string) => api<AiFileContent>('GET', `/projects/${id}/ai-file?path=${encodeURIComponent(path)}`),
  searchAiFiles: (id: string, q: string) => api<AiSearchResult[]>('GET', `/projects/${id}/ai-search?q=${encodeURIComponent(q)}`),
  gitStatus: (id: string) => api<{ branch: string; default_branch: string; is_default_branch: boolean; files: { path: string; status: string; staged: boolean }[]; ahead: number; behind: number; has_upstream: boolean; provider: string; pr_url?: string | null }>('GET', `/projects/${id}/git-status`),
  gitDiff: (id: string, path: string, committed = false) => api<{ path: string; diff: string }>('GET', `/projects/${id}/git-diff?path=${encodeURIComponent(path)}${committed ? '&committed=true' : ''}`),
  gitCreateBranch: (id: string, req: { name: string }) => api<{ branch: string }>('POST', `/projects/${id}/git-branch`, req),
  gitCommit: (id: string, req: { files: string[]; message: string; amend?: boolean; sign?: boolean }) => api<{ hash: string; message: string }>('POST', `/projects/${id}/git-commit`, req),
  gitPush: (id: string) => api<{ success: boolean; message: string }>('POST', `/projects/${id}/git-push`, {}),
  createPr: (id: string, req: { title: string; body?: string; base?: string }) => api<{ url: string }>('POST', `/projects/${id}/git-pr`, req),
  prTemplate: (id: string) => api<{ template: string; source: string }>('GET', `/projects/${id}/pr-template`),
  exec: (id: string, command: string) => api<{ stdout: string; stderr: string; exit_code: number }>('POST', `/projects/${id}/exec`, { command }),
  remapPath: (id: string, path: string) => api<void>('POST', `/projects/${id}/remap-path`, { path }),
  // Recover a project whose path no longer resolves: re-clone its repo_url
  // locally (using the linked Git credentials) and re-point the project at
  // the clone. `parent_dir` optional — the server picks an existing location.
  cloneAndRemap: (id: string, req: CloneAndRemapRequest = { parent_dir: null }) =>
    api<CloneAndRemapResponse>('POST', `/projects/${id}/clone-and-remap`, req),

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
        onEvent: (type, payload) => {
          const p = payload as AuditSseEvent;
          switch (type) {
            case 'step_start': handlers.onStepStart(p.step as number, p.total as number, p.file as string); break;
            case 'chunk': handlers.onChunk(p.text as string, p.step as number); break;
            case 'step_done': handlers.onStepDone(p.step as number, p.success as boolean); break;
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
        onEvent: (type, payload) => {
          const p = payload as AuditSseEvent;
          switch (type) {
            case 'step_start': handlers.onStepStart(p.step as number, p.total as number, p.file as string); break;
            case 'chunk': handlers.onChunk(p.text as string, p.step as number); break;
            case 'step_done': handlers.onStepDone(p.step as number, p.success as boolean); break;
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
      onLegacyDocsMigrated?: (report: LegacyDocsMigrationReport) => void;
      /**
       * 0.8.3 (#274) — first event in the audit loop. Carries the
       * total step count + the wallclock start so the frontend can
       * show a live elapsed counter. Optional for backwards compat —
       * callers that haven't upgraded just don't see the new UX.
       */
      onAuditStart?: (totalSteps: number, startedAt: string) => void;
      onStepStart: (step: number, total: number, file: string) => void;
      onChunk: (text: string, step: number) => void;
      /**
       * 0.8.3 (#274) — extended with per-step tokens, duration and
       * the running total. Existing callers (single-arg + step
       * only) keep working because the tuple is positional-extended.
       */
      onStepDone: (
        step: number,
        success: boolean,
        tokens?: number,
        durationMs?: number,
        totalTokens?: number,
      ) => void;
      /**
       * 0.8.3 (#281) — live token counter during a step. Fires every
       * time the agent emits a `Usage` event in its stream-json. The
       * frontend uses this to tick the `💬 X tk` chip in real time
       * rather than waiting for `step_done` (which can be 30-120s on
       * a heavy step). Optional for backwards compat.
       */
      onStepProgress?: (step: number, stepTokens: number, totalTokensSoFar: number) => void;
      /**
       * 0.8.3 (#281) — agent started calling a tool (Read, Glob,
       * Bash, mcp__...). Frontend surfaces the name as a chip so
       * the user knows what the agent is busy doing during the
       * step. Optional for backwards compat.
       */
      onToolCall?: (step: number, tool: string) => void;
      /**
       * 0.8.3 root-cause fix — backend detected that this step's
       * `target_file` is empty / truncated despite the CLI exiting 0.
       * Backend auto-repairs from template; frontend should surface
       * a warning so the user knows the step didn't produce useful
       * output and may want to re-audit. Optional for backwards-compat.
       */
      onStepWarning?: (step: number, file: string, reason: string, repaired: boolean) => void;
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
        onEvent: (type, payload) => {
          const p = payload as AuditSseEvent;
          switch (type) {
            case 'template_installed': handlers.onTemplateInstalled(p.installed as boolean); break;
            // 0.8.3 (#272) — pre-audit legacy docs migration. Emitted
            // at most once per run, BEFORE the 9-step audit loop.
            // The handler is optional so older callers compile without
            // changes; when present, the frontend shows a toast +
            // list of moved entries so the user sees that their
            // hand-curated docs survived and where they live now.
            case 'legacy_docs_migrated':
              handlers.onLegacyDocsMigrated?.({
                migrated: p.migrated ?? false,
                skip_reason: p.skip_reason ?? '',
                moved_entries: p.moved_entries ?? [],
                moved_count: p.moved_count ?? 0,
              });
              break;
            // 0.8.3 (#274) — first event in the audit loop. Carries
            // the wallclock start (ISO-8601) + total step count so
            // the frontend can compute a live elapsed counter without
            // local-clock drift. Optional handler keeps backwards-
            // compat with callers that don't show the new chip.
            case 'start':
              handlers.onAuditStart?.(
                (p.total_steps ?? p.total ?? 0) as number,
                (p.started_at ?? new Date().toISOString()) as string,
              );
              break;
            case 'step_start': handlers.onStepStart(p.step as number, p.total as number, p.file as string); break;
            case 'chunk': handlers.onChunk(p.text as string, p.step as number); break;
            case 'step_done':
              handlers.onStepDone(
                p.step as number,
                p.success as boolean,
                p.tokens,
                p.duration_ms,
                p.total_tokens,
              );
              break;
            case 'step_progress':
              // 0.8.3 (#281) — live token tick. Both fields are
              // numbers but we guard for type safety in case a
              // future backend version omits one.
              if (typeof p.step === 'number' && typeof p.step_tokens === 'number') {
                handlers.onStepProgress?.(
                  p.step,
                  p.step_tokens,
                  (p.total_tokens_so_far as number | undefined) ?? 0,
                );
              }
              break;
            case 'tool_call':
              if (typeof p.step === 'number' && typeof p.tool === 'string') {
                handlers.onToolCall?.(p.step, p.tool);
              }
              break;
            case 'step_warning':
              // 0.8.3 root-cause fix — emitted when the step's
              // target_file is empty/truncated. The backend has
              // already repaired the file from the template (if
              // available); the frontend just surfaces the alert so
              // the user sees the partial failure rather than a
              // silent green tick.
              if (typeof p.step === 'number' && typeof p.file === 'string') {
                handlers.onStepWarning?.(
                  p.step,
                  p.file,
                  p.reason ?? 'Step output looks incomplete',
                  p.repaired_from_template ?? false,
                );
              }
              break;
            case 'step_error': handlers.onError(p.error ?? 'Step error'); break;
            case 'validation_created': handlers.onValidationCreated(p.discussion_id as string); break;
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
  /** 0.8.6 — update an existing Custom API plugin's spec
   *  (name/base_url/description/docs_url/fields/endpoints). Server_id
   *  is preserved so configs and workflow `ApiCall` refs stay valid.
   *  Encrypted env per-config is NOT touched here — see Settings → APIs
   *  for the env-edit drawer. Backend rejects non-custom server_ids. */
  updateCustomSpec: (serverId: string, payload: CustomApiPayload) =>
    api<UpdateCustomSpecResponse>('PUT', `/mcps/custom/${encodeURIComponent(serverId)}`, payload),
  /** 0.8.6 (#60) — remove orphan env keys (left behind by a field
   *  rename / removal) from every config linked to this server. The
   *  list of keys to remove comes from the response of a preceding
   *  `updateCustomSpec` call. */
  cleanupOrphanEnv: (serverId: string, keys: string[]) =>
    api<CleanupOrphanEnvResponse>('POST', `/mcps/custom/${encodeURIComponent(serverId)}/cleanup-orphan-env`, { keys }),
  /** 0.8.6 (#63) — Path B export. Returns the path to call directly via
   *  `<a href="...">` for download — the route emits Content-Disposition
   *  attachment, the browser handles the rest. Auth header is added by
   *  the global `api()` helper, so callers should fetch + blob if they
   *  need to thread the token; here we return the URL for a direct link. */
  exportFileUrl: (serverId: string) =>
    `/api/mcps/custom/${encodeURIComponent(serverId)}/export-file`,
  /** 0.8.6 (#63) — Path B import. Frontend reads the user's `.json` file
   *  via `FileReader`, parses to JSON, POSTs the parsed payload. */
  importPluginFile: (payload: CustomApiPayload) =>
    api<McpConfigDisplay>('POST', '/mcps/custom/import-file', payload),
  deleteConfig: (id: string) => api<void>('DELETE', `/mcps/configs/${id}`),
  setConfigProjects: (id: string, req: LinkMcpConfigRequest) => api<void>('PATCH', `/mcps/configs/${id}/projects`, req),
  revealSecrets: (id: string) => api<McpEnvEntry[]>('POST', `/mcps/configs/${id}/reveal`),
  /** Scan host CLI config files for MCPs declared outside Kronn (Phase 1: read-only). */
  hostDiscovery: () => api<DiscoveredHostMcp[]>('GET', '/mcps/host-discovery'),
  /** Phase 2: adopt a host-declared MCP into the Kronn registry (no host-file mutation). */
  adoptHost: (req: AdoptHostMcpRequest) => api<McpConfigDisplay>('POST', '/mcps/host-discovery/adopt', req),
  // MCP context files
  listContexts: (projectId: string) => api<McpContextEntry[]>('GET', `/mcps/context/${projectId}`),
  getContext: (projectId: string, slug: string) => api<McpContextEntry>('GET', `/mcps/context/${projectId}/${slug}`),
  updateContext: (projectId: string, slug: string, content: string) => api<void>('PUT', `/mcps/context/${projectId}/${slug}`, { content }),
};

// ─── Discussions ────────────────────────────────────────────────────────────

/** Result of the unified "join by code" (`POST /discussions/peer-join`). The
 *  backend resolves the token LOCAL or cross-instance transparently. */
/** Mirror of the backend `RecentMessagePreview` (disc_invite.rs). `preview` is
 *  the body trimmed to ~400 chars; fetch full text via the disc itself. */
export interface RecentMessagePreview {
  sort_order: number;
  role: string;
  agent_type: string | null;
  timestamp: string;
  preview: string;
}

export interface PeerJoinResult {
  disc_id: string;
  session_pk: number;
  peer_count: number;
  disc_title: string;
  recent_messages: RecentMessagePreview[];
  next_steps: string;
}

/** Stable per-browser id used as the `session_id` when a human joins a disc by
 *  code from the web UI (the join API is shared with CLI agents, which supply
 *  their own session id). Persisted so re-joining the same disc is idempotent
 *  server-side rather than spawning phantom participants. */
function webSessionId(): string {
  const KEY = 'kronn:webSessionId';
  try {
    let id = localStorage.getItem(KEY);
    if (!id) {
      id = (crypto?.randomUUID?.() ?? `web-${Date.now()}-${Math.random().toString(16).slice(2)}`);
      localStorage.setItem(KEY, id);
    }
    return id;
  } catch {
    // No localStorage (SSR / tests): a per-call id is fine, just not idempotent.
    return crypto?.randomUUID?.() ?? `web-${Date.now()}`;
  }
}

export const discussions = {
  list: () => api<Discussion[]>('GET', '/discussions'),
  /** 2026-06-24 — disc ids with an in-flight agent run RIGHT NOW, server-side
   *  (incl. background/batch children). Polled so a run still working after you
   *  navigate away keeps showing as running, instead of looking dead. */
  getRunning: () => api<string[]>('GET', '/discussions/running'),
  get: (id: string) => api<Discussion>('GET', `/discussions/${id}`),
  create: (req: CreateDiscussionRequest) => api<Discussion>('POST', '/discussions', req),
  delete: (id: string) => api<void>('DELETE', `/discussions/${id}`),
  update: (id: string, body: { title?: string; archived?: boolean; pinned?: boolean; skill_ids?: string[]; profile_ids?: string[]; directive_ids?: string[]; project_id?: string | null; tier?: ModelTier; agent?: AgentType; summary_strategy?: 'Auto' | 'OnDemand' | 'Off' }) => api<void>('PATCH', `/discussions/${id}`, body),
  share: (id: string, contactIds: string[]) => api<string>('POST', `/discussions/${id}/share`, { contact_ids: contactIds }),
  /** Unified "join by code": paste any `kr-join-…` token and the backend
   *  resolves it LOCAL or cross-instance. If it isn't a local room, the backend
   *  asks our accepted contacts (claim-by-token) and mirrors the disc back over
   *  the WS federation (~0.5–8 s), then binds. The single await covers that
   *  whole resolution — the caller just shows a "resolving…" state until it
   *  returns the (mirrored) disc. */
  peerJoin: (token: string) =>
    api<PeerJoinResult>('POST', '/discussions/peer-join', {
      token: token.trim(),
      agent_type: 'Custom',
      session_id: webSessionId(),
    }),
  /** 0.8.6 phase 2 — list active+paused participants of a disc.
   *  Powers the header chips + `[+ Inviter]` button. `left` sessions
   *  are excluded server-side (audit history only). */
  participants: (id: string) =>
    api<Array<{ id: number; disc_id: string; agent_type: string; session_id: string | null; role: string; status: string; joined_at: string; left_at: string | null; last_seen: string | null }>>('GET', `/discussions/${id}/participants`),
  /** 0.8.6 phase 2 — mint a one-shot invite token bound to this disc.
   *  Returns the PLAIN token (only place it ever appears outside the
   *  agent's tool-call wire) + the human-readable instruction the
   *  copy-paste modal displays. Each call yields a fresh token, so
   *  inviting N peers = N calls. TTL 10 min. */
  invitePeer: (id: string) =>
    api<{ token: string; disc_id: string; expires_at: string; ttl_seconds: number; instruction_text: string }>('POST', `/discussions/${id}/invite-peer`, {}),
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
        onEvent: (type, payload) => {
          const parsed = payload as { text?: string; error?: string };
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
  gitDiff: (id: string, path: string, committed = false) => api<{ path: string; diff: string }>('GET', `/discussions/${id}/git-diff?path=${encodeURIComponent(path)}${committed ? '&committed=true' : ''}`),
  gitCommit: (id: string, req: { files: string[]; message: string; amend?: boolean; sign?: boolean }) => api<{ hash: string; message: string }>('POST', `/discussions/${id}/git-commit`, req),
  gitPush: (id: string) => api<{ success: boolean; message: string }>('POST', `/discussions/${id}/git-push`, {}),
  createPr: (id: string, req: { title: string; body?: string; base?: string }) => api<{ url: string }>('POST', `/discussions/${id}/git-pr`, req),
  prTemplate: (id: string) => api<{ template: string; source: string }>('GET', `/discussions/${id}/pr-template`),
  exec: (id: string, command: string) => api<{ stdout: string; stderr: string; exit_code: number }>('POST', `/discussions/${id}/exec`, { command }),
  worktreeUnlock: (id: string) => api<string>('POST', `/discussions/${id}/worktree-unlock`, {}),
  worktreeLock: (id: string) => api<string>('POST', `/discussions/${id}/worktree-lock`, {}),
  // High-level "try this version in my IDE" flow — orchestrates unlock +
  // checkout + optional stash. The response envelope carries either success
  // or a structured preflight blocker (worktree dirty, main dirty, detached
  // HEAD) that the UI matches on `status` to pick the right modal.
  testModeEnter: (id: string, opts?: { stash_dirty?: boolean; force?: boolean }) =>
    api<TestModeEnterResult>('POST', `/discussions/${id}/test-mode/enter`, opts ?? {}),
  testModeExit: (id: string) =>
    api<TestModeExitResponse>('POST', `/discussions/${id}/test-mode/exit`, {}),

  // ── Context Files ──
  listContextFiles: (id: string) => api<ContextFile[]>('GET', `/discussions/${id}/context-files`),
  deleteContextFile: (id: string, fileId: string) => api<void>('DELETE', `/discussions/${id}/context-files/${fileId}`),
  /** Pin all pending (composer-staged) files of a discussion to a message.
   *  Used by the creation popup to attach uploads to the first message —
   *  the in-disc composer links implicitly at send time. */
  linkPendingContextFiles: (id: string, messageId: string) =>
    api<number>('POST', `/discussions/${id}/context-files/link-pending`, { message_id: messageId }),
  /** Fetch an uploaded image's raw bytes (auth'd) so the UI can render a
   *  thumbnail via an object URL — `<img src>` can't carry auth headers. */
  contextFileBlob: async (id: string, fileId: string): Promise<Blob> => {
    const res = await fetch(`${_apiBase}/api/discussions/${id}/context-files/${fileId}/content`, {
      headers: { ...authHeaders() },
    });
    if (!res.ok) throw new Error(`Failed to load attachment (${res.status})`);
    return res.blob();
  },
  uploadContextFile: async (id: string, file: File): Promise<UploadContextFileResponse> => {
    const form = new FormData();
    form.append('file', file);
    const res = await fetch(`${_apiBase}/api/discussions/${id}/context-files`, {
      method: 'POST',
      headers: { ...authHeaders() },
      body: form,
    });
    // A too-large upload (413) or other framework-level rejection has no JSON
    // body — calling res.json() then throws an opaque parse error that surfaces
    // as a generic toast. Surface the status so userError() maps it ("413" →
    // "fichier trop volumineux").
    if (!res.ok) {
      let detail = '';
      try { const j = await res.json(); detail = j?.error ?? ''; } catch { /* non-JSON body */ }
      throw new Error(detail || `Upload failed (HTTP ${res.status})`);
    }
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
  /** 0.8.3 — atomic bundle creation. POSTs a payload with optional
   *  `quick_prompts` / `quick_apis` / `custom_apis` sections plus a
   *  `workflow` section that may reference them via `@bundle:<id>`
   *  sentinels. The server creates everything in a single SQLite
   *  transaction — rollback on any failure, no orphan rows. Drives
   *  the `KRONN:BUNDLE_READY` chat signal flow. */
  createBundle: (req: unknown) => api<BundleResponse>('POST', '/workflows/bundle', req),
  update: (id: string, req: UpdateWorkflowRequest) => api<Workflow>('PUT', `/workflows/${id}`, req),
  delete: (id: string) => api<void>('DELETE', `/workflows/${id}`),
  trigger: (id: string) => api<WorkflowRun>('POST', `/workflows/${id}/trigger`),

  /** Trigger with SSE streaming for real-time progress.
   *  0.6.0 UX pass — accepts optional `variables` payload for workflows
   *  declaring manual launch variables. Required-but-empty values
   *  surface as a "Variable « X » est obligatoire…" SSE error. */
  triggerStream: async (
    id: string,
    onStepStart: (data: { step_name: string; step_index: number; total_steps: number }) => void,
    onStepDone: (data: StepResult) => void,
    onRunDone: (data: { status: string }) => void,
    onError: (error: string) => void,
    signal?: AbortSignal,
    variables?: Record<string, string>,
    /** Live agent stdout chunks for the step currently in flight. Optional
     *  — older callers ignore them; the workflow live view uses them to
     *  render the in-progress step's output as it streams (no more
     *  60s of "spinner with no content" UX). */
    onStepProgress?: (text: string) => void,
    /** Fires once at the start with the freshly-created run_id. Optional.
     *  Used by the live view to surface a "⏹ Stop" button that calls
     *  `cancelRun(workflow_id, run_id)` — without the run_id the live
     *  view can't address the run it's watching. */
    onRunStart?: (runId: string) => void,
  ) => {
    const headers: Record<string, string> = { ...authHeaders() };
    let body: string | undefined;
    if (variables && Object.keys(variables).length > 0) {
      headers['Content-Type'] = 'application/json';
      body = JSON.stringify({ variables });
    }
    const res = await fetch(`${_apiBase}/api/workflows/${id}/trigger`, {
      method: 'POST',
      headers,
      body,
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
              if (eventType === 'run_start' && onRunStart) onRunStart(parsed.run_id ?? '');
              else if (eventType === 'step_start') onStepStart(parsed);
              else if (eventType === 'step_progress' && onStepProgress) onStepProgress(parsed.text ?? '');
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
  /** 0.7.0 Phase 4 — submit operator's decision on a paused (Gate) run.
   *  `decision` ∈ "approve" | "request_changes" | "reject".
   *  `comment` is required for request_changes (the agent needs feedback).
   *  Returns the new run status (Running on approve / request_changes,
   *  Failed on reject). The actual continuation runs in the background. */
  decideRun: (id: string, runId: string, payload: DecideRunRequest) =>
    api<DecideRunResponse>(
      'POST', `/workflows/${id}/runs/${runId}/decide`, payload
    ),
  /** 0.7.0 — create an isolated test worktree on a run's preserved branch.
   *  The agent committed locally but couldn't push (pre-push hook, no auth, …);
   *  this hands the operator a path they can `cd` into to verify the work
   *  without touching their main checkout. Idempotent — re-creating returns
   *  the same path. Pair with `deleteTestWorktree` to clean up. */
  createTestWorktree: (id: string, runId: string, branchIndex?: number) =>
    api<{ worktree_path: string; branch_name: string; head_sha: string }>(
      'POST', `/workflows/${id}/runs/${runId}/test-worktree`,
      branchIndex != null ? { branch_index: branchIndex } : {},
    ),
  deleteTestWorktree: (id: string, runId: string) =>
    api<void>('DELETE', `/workflows/${id}/runs/${runId}/test-worktree`),
  /** 0.7.0 UX pass — export a single workflow as a self-contained JSON.
   *  Triggers a browser file download via Content-Disposition. Different
   *  contract from the regular `api()` helper (binary-style response,
   *  not the {success, data} envelope), so we bypass it. */
  exportWorkflow: async (id: string): Promise<{ filename: string; blob: Blob }> => {
    const r = await fetch(`/api/workflows/${id}/export`, { credentials: 'same-origin' });
    if (!r.ok) throw new Error(`Export failed (${r.status})`);
    const cd = r.headers.get('content-disposition') || '';
    const m = cd.match(/filename="([^"]+)"/);
    const filename = m?.[1] || `workflow-${id}.kronn-workflow.json`;
    return { filename, blob: await r.blob() };
  },
  /** 0.7.0 UX pass — import a workflow from a previously-exported JSON. */
  importWorkflow: (payload: ImportWorkflowRequest) =>
    api<Workflow>('POST', '/workflows/import', payload),
  /** Dry-run preview of a BatchQuickPrompt step: returns parsed items, sample
   *  rendered prompt, QP info, errors. NO discussion is created. */
  testBatchStep: (req: { step: WorkflowStep; mock_previous_output?: string | null; previous_step_name?: string | null }) =>
    api<BatchPreview>('POST', '/workflows/test-batch-step', req),
  /** Pure JSONPath extraction on a client-provided JSON sample — no network,
   *  no DB. Drives the wizard's live preview box so users can refine their
   *  path without re-hitting the API. Returns a 200 even on invalid path
   *  (error goes into the `error` field for inline display). */
  testExtract: (req: { sample: unknown; path: string; fallback?: unknown; fail_on_empty?: boolean }) =>
    api<{ value: unknown; value_type: string; is_empty: boolean; error: string | null }>(
      'POST', '/workflow-steps/test-extract', req,
    ),
  /** Run an ApiCall step end-to-end (real HTTP, real auth) and return the
   *  structured envelope. Drives the wizard's "Tester" button. */
  testApiCall: (req: { step: WorkflowStep; project_id: string }) =>
    api<{ success: boolean; duration_ms: number; envelope: { data: unknown; status: string; summary: string } | null; error: string | null }>(
      'POST', '/workflow-steps/test-api-call', req,
    ),
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
  /**
   * Compare-agents fan-out — same prompt across N agents in parallel
   * (one disc per agent, all under one batch group). Mirrors `batchRun`
   * but the variation axis is the agent, not the input. Cf.
   * `project_qp_compare_agents` memory.
   */
  compareAgents: (
    qpId: string,
    req: { prompt: string; batch_name: string; agents: AgentType[]; tier?: ModelTier; project_id?: string },
  ) => api<BatchRunResponse>('POST', `/quick-prompts/${qpId}/compare-agents`, req),
  /** 0.7.0 UX pass — export a single QP as JSON file download. */
  exportQp: async (id: string): Promise<{ filename: string; blob: Blob }> => {
    const r = await fetch(`/api/quick-prompts/${id}/export`, { credentials: 'same-origin' });
    if (!r.ok) throw new Error(`Export failed (${r.status})`);
    const cd = r.headers.get('content-disposition') || '';
    const m = cd.match(/filename="([^"]+)"/);
    const filename = m?.[1] || `quick-prompt-${id}.kronn-qp.json`;
    return { filename, blob: await r.blob() };
  },
  importQp: (payload: ImportQuickPromptRequest) =>
    api<QuickPrompt>('POST', '/quick-prompts/import', payload),
  /** 0.8.5 — full version history, newest first. Empty array for legacy
   *  QPs that pre-date 0.8.5 (no snapshot was seeded). */
  history: (id: string) => api<QuickPromptVersion[]>('GET', `/quick-prompts/${id}/history`),
  /** 0.8.5 — per-version aggregated launch metrics (avg tokens, avg
   *  duration, avg cost). One row per version that has ≥ 1 launch. */
  metrics: (id: string) => api<QuickPromptVersionMetrics[]>('GET', `/quick-prompts/${id}/metrics`),
  /** 0.8.5 — delete an archived QP version. Refused on the current
   *  (highest) version_index by the backend. Discussions stamped with
   *  the deleted version have their lineage cleared (lost attribution,
   *  the disc itself stays). */
  deleteVersion: (id: string, versionIndex: number) =>
    api<boolean>('DELETE', `/quick-prompts/${id}/versions/${versionIndex}`),
};

// ─── Quick APIs (0.6.0) ─────────────────────────────────────────────────────
// Mirror of `quickPrompts` for HTTP call templates. Same CRUD shape; the
// `runQa` endpoint executes the saved QuickApi standalone with the user-supplied
// variables and returns the parsed envelope (or the error message on failure).

export const quickApis = {
  list: () => api<QuickApi[]>('GET', '/quick-apis'),
  create: (req: CreateQuickApiRequest) => api<QuickApi>('POST', '/quick-apis', req),
  update: (id: string, req: CreateQuickApiRequest) => api<QuickApi>('PUT', `/quick-apis/${id}`, req),
  delete: (id: string) => api<void>('DELETE', `/quick-apis/${id}`),
  runQa: (id: string, req: RunQuickApiRequest) =>
    api<RunQuickApiResponse>('POST', `/quick-apis/${id}/run`, req),
  batchRunQa: (id: string, req: BatchRunQuickApiRequest) =>
    api<BatchRunQuickApiResponse>('POST', `/quick-apis/${id}/batch`, req),
  exportQa: async (id: string): Promise<{ filename: string; blob: Blob }> => {
    const r = await fetch(`/api/quick-apis/${id}/export`, { credentials: 'same-origin' });
    if (!r.ok) throw new Error(`Export failed (${r.status})`);
    const cd = r.headers.get('content-disposition') || '';
    const m = cd.match(/filename="([^"]+)"/);
    const filename = m?.[1] || `quick-api-${id}.kronn-qa.json`;
    return { filename, blob: await r.blob() };
  },
  importQa: (payload: ImportQuickApiRequest) =>
    api<QuickApi>('POST', '/quick-apis/import', payload),
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

// ─── RTK (Rust Token Killer — host-side compression proxy) ────────────────

export interface RtkAgentActivation {
  agent_type: AgentType;
  success: boolean;
  stdout: string;
  stderr: string;
}

/** Response shape of `POST /api/rtk/activate` — kept here rather than in
 *  `types/generated.ts` because the backend struct isn't consumed by any
 *  generated type downstream, so maintaining it inline is cheaper. */
export interface RtkActivateResponse {
  success: boolean;
  stdout: string;
  stderr: string;
  per_agent: RtkAgentActivation[];
}

/** Response shape of `GET /api/rtk/savings`. `available: false` means RTK
 *  isn't readable and the frontend should hide the counter entirely. */
export interface RtkSavings {
  available: boolean;
  total_tokens_saved: number;
  ratio_percent: number;
  sample_count: number;
}

/** Response shape of `GET /api/rtk/version`. `update_available: true`
 *  drives the "update available" pill in the RTK Settings card. */
export interface RtkVersionInfo {
  available: boolean;
  installed: string | null;
  latest_known: string;
  update_available: boolean;
  update_command: string;
}

export const rtk = {
  /** Wire RTK hooks into each supported agent. The backend filters to
   *  agents RTK actually supports (Claude Code, Codex, Gemini CLI at the
   *  time of writing) and spawns one `rtk init -g ...` per agent. */
  activate: (agents: AgentType[]) =>
    api<RtkActivateResponse>('POST', '/rtk/activate', { agents }),
  /** Remove RTK hooks from the given agents. Mirrors `activate` shape. */
  deactivate: (agents: AgentType[]) =>
    api<RtkActivateResponse>('POST', '/rtk/deactivate', { agents }),
  /** Read the global savings counter RTK keeps in its own SQLite. */
  savings: () => api<RtkSavings>('GET', '/rtk/savings'),
  /** Read the installed-vs-latest RTK version and precomputed
   *  `update_available` flag — feeds the freshness pill. */
  version: () => api<RtkVersionInfo>('GET', '/rtk/version'),
};

// ─── Agent CLI usage / cost (via ccusage) ───────────────────────────────────

export const usage = {
  /** 0.8.7 — global usage/cost report across the detected CLIs (Claude /
   *  Codex / Gemini …). `period` = daily | weekly | monthly. Real token +
   *  cache breakdown, not Kronn's rough estimate. */
  get: (period: 'daily' | 'weekly' | 'monthly' = 'daily') =>
    api<UsageReport>('GET', `/usage?period=${period}`),
};

// ─── Ollama (local LLM) ────────────────────────────────────────────────────

export const ollama = {
  health: () => api<OllamaHealthResponse>('GET', '/ollama/health'),
  models: () => api<OllamaModelsResponse>('GET', '/ollama/models'),
};

// 0.8.6 (#24) — Unified API call logs.
export interface ApiCallLogRow {
  id: string;
  source: string;
  project_id: string | null;
  run_id: string | null;
  disc_id: string | null;
  agent: string | null;
  plugin_slug: string;
  config_id: string | null;
  endpoint_path: string;
  method: string;
  http_status: number | null;
  status: string;
  duration_ms: number;
  request_excerpt: string | null;
  response_excerpt: string | null;
  error_message: string | null;
  called_at: string;
}

export interface ApiCallLogsFilter {
  source?: 'workflow' | 'agent_broker' | 'manual_test';
  project_id?: string;
  plugin_slug?: string;
  status?: 'OK' | 'ERROR' | 'RateLimited' | 'TimedOut';
  limit?: number;
}

export const apiCallLogs = {
  list: (filter?: ApiCallLogsFilter) => {
    const qs = filter ? '?' + new URLSearchParams(
      Object.entries(filter)
        .filter(([, v]) => v !== undefined && v !== null && v !== '')
        .map(([k, v]) => [k, String(v)]),
    ).toString() : '';
    return api<ApiCallLogRow[]>('GET', `/api-call-logs${qs}`);
  },
  get: (id: string) => api<ApiCallLogRow | null>('GET', `/api-call-logs/${id}`),
  purge: (days?: number) => api<number>('POST', '/api-call-logs/purge', { days }),
};

/** Shape returned by `GET /api/debug/logs`. Not a `ts-rs`-generated type
 *  because this is a purely internal endpoint — the wrapper is enough. */
export interface DebugLogsResponse {
  /** Most-recent lines, oldest-first (ready for <pre>). */
  lines: string[];
  /** Total buffered across all levels — drives the "N events captured" hint. */
  buffered: number;
  /** Max capacity of the ringbuffer (backend-configured). */
  capacity: number;
  /** Current value of `config.server.debug_mode`. */
  debug_mode: boolean;
}

export const debugApi = {
  /** Fetch the last `lines` log entries from the backend ringbuffer. */
  getLogs: (lines: number = 200) =>
    api<DebugLogsResponse>('GET', `/debug/logs?lines=${Math.max(0, Math.floor(lines))}`),
  /** Empty the backend ringbuffer — useful for "clear, reproduce, capture". */
  clearLogs: () => api<void>('POST', '/debug/logs/clear'),
};

/** Shape of each entry returned by POST /api/themes/unlock. A single
 *  code may unlock several items in one shot (bundle codes, e.g.
 *  Batman unlocks a profile + a theme together), so the response
 *  `unlocks` is always an array. */
export interface UnlockedItem {
  kind: 'theme' | 'profile';
  name: string;
}

/** Secret-code unlock. Codes themselves never appear in this bundle —
 *  this call just shuttles a user-typed string to /api/themes/unlock and
 *  gets back `{ unlocks: [{ kind, name }, ...] }` on success. Throws
 *  (via api<>) if the backend rejects the code. */
export const themes = {
  unlock: (code: string) =>
    api<{ unlocks: UnlockedItem[] }>('POST', '/themes/unlock', { code }),
};

/** Document generation — proxies to the kronn-docs Python sidecar via
 *  the backend. Every call resolves to a relative `download_url` the
 *  frontend can link directly; don't build your own paths from `path`
 *  (which is the absolute disk location, useful only for logging). */
export interface GeneratePdfRequest {
  discussion_id: string;
  html: string;
  filename?: string;
  page_size?: string;
}
export interface GenerateDocxRequest {
  discussion_id: string;
  html: string;
  filename?: string;
}
export interface GenerateXlsxRequest {
  discussion_id: string;
  sheets: Array<{ name: string; rows: Array<Array<string | number | boolean | null>> }>;
  filename?: string;
}
export interface GenerateCsvRequest {
  discussion_id: string;
  rows: Array<Array<string | number | boolean | null>>;
  delimiter?: string;
  filename?: string;
}
export interface GeneratePptxRequest {
  discussion_id: string;
  slides: Array<{ title?: string; content?: string; bullets?: string[] }>;
  filename?: string;
}
export interface GeneratedDocInfo {
  path: string;
  download_url: string;
  size_bytes: number;
}
export const docs = {
  generatePdf: (req: GeneratePdfRequest) =>
    api<GeneratedDocInfo>('POST', '/docs/pdf', req),
  generateDocx: (req: GenerateDocxRequest) =>
    api<GeneratedDocInfo>('POST', '/docs/docx', req),
  generateXlsx: (req: GenerateXlsxRequest) =>
    api<GeneratedDocInfo>('POST', '/docs/xlsx', req),
  generateCsv: (req: GenerateCsvRequest) =>
    api<GeneratedDocInfo>('POST', '/docs/csv', req),
  generatePptx: (req: GeneratePptxRequest) =>
    api<GeneratedDocInfo>('POST', '/docs/pptx', req),
};

/** Auto-trigger opt-out — per-skill toggle. The `disabled` list is the
 *  skill IDs for which keyword-based auto-activation is OFF. A `toggle`
 *  returns the new `disabled` boolean for that specific skill. */
export const autoTriggersApi = {
  listDisabled: () => api<string[]>('GET', '/skills/auto-triggers/disabled'),
  toggle: (skillId: string) =>
    api<boolean>('POST', `/skills/${encodeURIComponent(skillId)}/auto-trigger/toggle`),
};

/** Shape of `/api/health` — the endpoint predates the `ApiResponse<T>`
 *  wrapper so it returns its payload directly. Used by the Debug > Report
 *  a bug flow to stamp version + host_os into the issue template. */
export interface HealthResponse {
  ok: boolean;
  version?: string;
  host_os?: string;
}

/** Direct fetch — bypasses the `api()` unwrapping because `/health` isn't
 *  wrapped in `ApiResponse<T>`. Kept private-ish (not on a big namespace)
 *  since only the bug-report button needs it today. */
export async function fetchHealth(): Promise<HealthResponse> {
  const res = await fetch(`${_apiBase}/api/health`, { headers: authHeaders() });
  if (!res.ok) throw new Error(`Health check failed: HTTP ${res.status}`);
  return res.json() as Promise<HealthResponse>;
}

// ─── User context (~/.kronn/user-context/) ──────────────────────────────────

export interface UserContextFile {
  name: string;
  size: number;
  content?: string;
}

/** 0.7.1 — cross-project user-scoped agent context. Markdown files in
 *  ~/.kronn/user-context/ are auto-injected into every agent's system
 *  prompt at spawn time, regardless of CLI. UI lets the operator manage
 *  them without ever opening a terminal. */
export const userContext = {
  list: () => api<UserContextFile[]>('GET', '/user-context'),
  get: (name: string) =>
    api<UserContextFile>('GET', `/user-context/${encodeURIComponent(name)}`),
  put: (name: string, content: string) =>
    api<UserContextFile>('PUT', `/user-context/${encodeURIComponent(name)}`, { content }),
  delete: (name: string) =>
    api<void>('DELETE', `/user-context/${encodeURIComponent(name)}`),
};

// ─── Continual Learning (0.9.0) ───────────────────────────────────────────────
export const learnings = {
  /** Run the validation pipeline (spec §6). Returns accept/reject + per-evidence
   *  Gate-1 checks + warnings. The agent path is the MCP tool; this is the HTTP face. */
  propose: (req: LearningProposeRequest) => api<ProposeResult>('POST', '/learnings/propose', req),
  list: (status?: LearningStatus, projectId?: string) => {
    const p = new URLSearchParams();
    if (status) p.set('status', status);
    if (projectId) p.set('project_id', projectId);
    const qs = p.toString();
    return api<Learning[]>('GET', `/learnings${qs ? `?${qs}` : ''}`);
  },
  pending: () => api<{ count: number }>('GET', '/learnings/pending'),
  /** Human gate: route scope → promote to the dedicated learnings file → mark promoted. */
  validate: (id: string) => api<Learning>('POST', `/learnings/${encodeURIComponent(id)}/validate`),
  reject: (id: string) => api<void>('POST', `/learnings/${encodeURIComponent(id)}/reject`),
  forDiscussion: (discId: string) =>
    api<Learning[]>('GET', `/discussions/${encodeURIComponent(discId)}/learnings`),
};
