// Shared API mock factory for frontend tests.
//
// Motivation: before this helper, each test file redeclared its own
// `vi.mock('../../lib/api', () => ({ ... }))` listing only the slices it
// needed. When a shared code path (e.g. I18nContext importing
// `config.getUiLanguage`) gained a new dependency, every indirect test
// broke because none of the inline mocks exposed the new method.
//
// Usage:
//   import { vi } from 'vitest';
//   import { buildApiMock } from '../../test/apiMock';
//
//   vi.mock('../../lib/api', () => buildApiMock({
//     // Override only what your test asserts on:
//     discussions: { list: vi.fn().mockResolvedValue([myDisc]) },
//   }));
//
// Every method returns a sensible empty-shape value so components render
// in their "no data" state without NPE / type errors.
//
// NOTE: the full list of namespaces is validated by the completeness test
// in `apiMock.complete.test.ts` — when you add a new namespace to
// `lib/api.ts`, that test will fail until the default mock covers it.

import { vi } from 'vitest';

type AnyFn = (...args: unknown[]) => unknown;
type PartialDeep<T> = { [K in keyof T]?: T[K] extends object ? PartialDeep<T[K]> : T[K] };

const resolve = <T>(value: T) => vi.fn().mockResolvedValue(value);

/** All top-level namespaces exposed by `lib/api.ts`. Kept in sync by the
 *  completeness test. */
export const API_NAMESPACES = [
  'setup',
  'config',
  'contacts',
  'projects',
  'agents',
  'mcps',
  'discussions',
  'workflows',
  'quickPrompts',
  'skills',
  'profiles',
  'directives',
  'stats',
  'ollama',
  'debugApi',
  'themes',
  'docs',
  'autoTriggersApi',
] as const;

/** Flat top-level helpers (non-namespace exports). */
export const API_TOP_LEVEL_FNS = [
  'setAuthToken',
  'getAuthToken',
  'authHeaders',
  'setApiBase',
  'getApiBase',
  'fetchHealth',
] as const;

interface DefaultMock {
  setAuthToken: AnyFn;
  getAuthToken: AnyFn;
  authHeaders: AnyFn;
  setApiBase: AnyFn;
  getApiBase: AnyFn;
  fetchHealth: AnyFn;
  setup: Record<string, AnyFn>;
  config: Record<string, AnyFn>;
  contacts: Record<string, AnyFn>;
  projects: Record<string, AnyFn>;
  agents: Record<string, AnyFn>;
  mcps: Record<string, AnyFn>;
  discussions: Record<string, AnyFn>;
  workflows: Record<string, AnyFn>;
  quickPrompts: Record<string, AnyFn>;
  skills: Record<string, AnyFn>;
  profiles: Record<string, AnyFn>;
  directives: Record<string, AnyFn>;
  stats: Record<string, AnyFn>;
  ollama: Record<string, AnyFn>;
  debugApi: Record<string, AnyFn>;
  themes: Record<string, AnyFn>;
  docs: Record<string, AnyFn>;
  autoTriggersApi: Record<string, AnyFn>;
}

/**
 * Build a default export-shape matching `lib/api.ts`. Every method is a
 * `vi.fn()` returning the empty/neutral value for its shape (empty arrays,
 * null strings, zero numbers). `overrides` is shallow-merged so callers
 * only need to specify what their test asserts on.
 */
export function buildApiMock(overrides: PartialDeep<DefaultMock> = {}): DefaultMock {
  const base: DefaultMock = {
    setAuthToken: vi.fn(),
    getAuthToken: vi.fn().mockReturnValue(null),
    authHeaders: vi.fn().mockReturnValue({}),
    setApiBase: vi.fn(),
    getApiBase: vi.fn().mockReturnValue(''),
    fetchHealth: vi.fn().mockResolvedValue({ ok: true }),

    setup: {
      getStatus: resolve({ is_first_run: false, current_step: 'done', agents_detected: [], repos_detected: [], scan_paths_set: true }),
    },

    config: {
      getTokens: resolve({ keys: [], active: {} }),
      saveApiKey: resolve({}),
      deleteApiKey: resolve(undefined),
      getLanguage: resolve('fr'),
      saveLanguage: resolve(undefined),
      getUiLanguage: resolve('fr'),
      saveUiLanguage: resolve(undefined),
      getSttModel: resolve(null),
      saveSttModel: resolve(undefined),
      getTtsVoices: resolve({}),
      saveTtsVoice: resolve(undefined),
      getScanPaths: resolve([]),
      getScanIgnore: resolve([]),
      getScanDepth: resolve(2),
      getAgentAccess: resolve({ agents: {} }),
      getModelTiers: resolve({ tiers: {} }),
      getGlobalContext: resolve(''),
      saveGlobalContext: resolve(undefined),
      getGlobalContextMode: resolve('always'),
      saveGlobalContextMode: resolve(undefined),
      getServerConfig: resolve({ pseudo: null, avatar_email: null, host: 'localhost', port: 3140 }),
    },

    contacts: {
      list: resolve([]),
      add: resolve({ contact: null, error: null }),
      delete: resolve(undefined),
    },

    projects: {
      list: resolve([]),
      get: resolve(null),
      scan: resolve([]),
      create: resolve({}),
      addFolder: resolve({}),
      update: resolve({}),
      delete: resolve(undefined),
      installTemplate: resolve({}),
      auditInfo: resolve({ files: [], todos: [] }),
      validateAudit: resolve('NoTemplate'),
      cancelAudit: resolve('NoTemplate'),
      checkDrift: resolve({ stale_steps: [], checksums_outdated: false }),
      getBriefing: resolve(null),
      startBriefing: resolve({ discussion_id: '' }),
      listAiFiles: resolve([]),
      createPr: resolve({ url: '' }),
      bootstrap: resolve({}),
      partialAudit: resolve({}),
      exportZip: resolve(new Blob()),
      importZip: resolve({ imported: 0 }),
      auditStream: vi.fn(),
    },

    agents: {
      listAll: resolve([]),
    },

    mcps: {
      listCatalog: resolve([]),
      listConfigs: resolve([]),
      createConfig: resolve({}),
      updateConfig: resolve({}),
      deleteConfig: resolve(undefined),
      link: resolve(undefined),
      unlink: resolve(undefined),
      listContexts: resolve([]),
      getContext: resolve({ slug: '', content: '' }),
      updateContext: resolve(undefined),
    },

    discussions: {
      list: resolve([]),
      get: resolve(null),
      create: resolve({}),
      delete: resolve(undefined),
      update: resolve(undefined),
      archive: resolve(undefined),
      unarchive: resolve(undefined),
      stop: resolve({ cancelled: false }),
      dismissPartial: resolve({ recovered: false }),
      createPr: resolve({ url: '' }),
      listContextFiles: resolve([]),
      deleteContextFile: resolve(undefined),
      uploadContextFile: resolve({}),
      deleteLastAgentMessages: resolve(undefined),
      sendMessageStream: vi.fn(),
      runAgent: vi.fn(),
    },

    workflows: {
      list: resolve([]),
      get: resolve(null),
      create: resolve({}),
      update: resolve({}),
      delete: resolve(undefined),
      trigger: resolve({}),
      triggerStream: vi.fn(),
      listRuns: resolve([]),
      getRun: resolve(null),
      deleteRun: resolve(undefined),
      deleteAllRuns: resolve(undefined),
      cancelRun: resolve({ run_cancelled: false, child_discs_cancelled: 0 }),
      testBatchStep: resolve({ eligible_items: [], sample_rendered_prompts: [], warnings: [] }),
      suggestions: resolve([]),
      listBatchRunSummaries: resolve([]),
      deleteBatchRun: resolve(undefined),
      testStepStream: vi.fn(),
    },

    quickPrompts: {
      list: resolve([]),
      create: resolve({}),
      update: resolve({}),
      delete: resolve(undefined),
    },

    skills: {
      list: resolve([]),
      create: resolve({}),
      update: resolve({}),
      delete: resolve(true),
    },

    profiles: {
      list: resolve([]),
      get: resolve(null),
      create: resolve({}),
      update: resolve({}),
      delete: resolve(true),
      updatePersonaName: resolve({}),
    },

    directives: {
      list: resolve([]),
      create: resolve({}),
      update: resolve({}),
      delete: resolve(true),
    },

    stats: {
      getTokens: resolve({ total_tokens: 0, by_provider: [], by_project: [] }),
      getAgentUsage: resolve([]),
    },

    ollama: {
      health: resolve({ status: 'not_installed', version: null, endpoint: 'http://localhost:11434', models_count: 0, hint: null }),
      models: resolve({ models: [] }),
    },

    debugApi: {
      getLogs: resolve({ lines: [], buffered: 0, capacity: 2000, debug_mode: false }),
      clearLogs: resolve(undefined),
    },

    themes: {
      unlock: resolve({ theme: 'matrix' }),
    },

    docs: {
      generatePdf: resolve({ path: '/tmp/x.pdf', download_url: '/api/docs/file/x/x.pdf', size_bytes: 1234 }),
      generateDocx: resolve({ path: '/tmp/x.docx', download_url: '/api/docs/file/x/x.docx', size_bytes: 1234 }),
      generateXlsx: resolve({ path: '/tmp/x.xlsx', download_url: '/api/docs/file/x/x.xlsx', size_bytes: 1234 }),
      generateCsv: resolve({ path: '/tmp/x.csv', download_url: '/api/docs/file/x/x.csv', size_bytes: 1234 }),
      generatePptx: resolve({ path: '/tmp/x.pptx', download_url: '/api/docs/file/x/x.pptx', size_bytes: 1234 }),
    },

    autoTriggersApi: {
      listDisabled: resolve([]),
      toggle: resolve(false),
    },
  };

  // Shallow-merge overrides onto base (namespace by namespace).
  const out = { ...base } as unknown as Record<string, unknown>;
  const baseRec = base as unknown as Record<string, unknown>;
  for (const [key, value] of Object.entries(overrides)) {
    const baseValue = baseRec[key];
    if (value && typeof value === 'object' && baseValue && typeof baseValue === 'object') {
      out[key] = { ...(baseValue as Record<string, unknown>), ...(value as Record<string, unknown>) };
    } else {
      out[key] = value;
    }
  }
  return out as unknown as DefaultMock;
}
