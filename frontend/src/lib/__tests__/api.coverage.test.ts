/**
 * 0.8.7 — P1-top of the QA roadmap. Real-coverage audit on 2026-05-28
 * showed `lib/api.ts` at **24 % stmts / 11 % functions / 25 % lines** —
 * the lowest coverage of any critical file (it's the ENTIRE boundary
 * between every UI surface and the backend). Audit had missed this
 * because file-level LOC heuristics over-weighted the structural
 * `expect(api.foo).toBeDefined()` tests (which exist).
 *
 * Real coverage requires actually CALLING each method so V8 records the
 * function entry. This file does that : per-namespace, fire each method
 * with stub args, mock fetch, assert (verb, URL).
 *
 * Goal : lift `api.ts` function coverage from ~11 % to **80 %+**.
 *
 * Strategy : one `describe` per namespace, one `it` per method (or a
 * compact `it.each` for trivial cases). Mock fetch globally ; the api
 * wrapper unwraps `ApiResponse<T>` so we return `{success:true, data}`.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import {
  setup, version, config, contacts, projects, agents, mcps, discussions,
  workflows, quickPrompts, quickApis, skills, profiles, directives, stats,
  rtk, usage, ollama, apiCallLogs, debugApi, themes, docs, autoTriggersApi,
  userContext, setApiBase, setAuthToken,
} from '../api';

// ─── Fetch mock harness ─────────────────────────────────────────────────────

let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  fetchMock = vi.fn();
  vi.stubGlobal('fetch', fetchMock);
  // Default: success response.
  fetchMock.mockResolvedValue({
    ok: true,
    status: 200,
    headers: {
      get: (name: string) => (name === 'content-type' ? 'application/json' : null),
    },
    json: async () => ({ success: true, data: null }),
    text: async () => '',
    blob: async () => new Blob(),
    body: null,
  });
  setApiBase('');
  setAuthToken(null);
});

afterEach(() => {
  vi.restoreAllMocks();
});

/** Resolve fetch + assert (verb, urlSuffix). Returns the body (if any). */
async function expectFetch(verb: string, urlSuffix: string): Promise<unknown> {
  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [url, opts] = fetchMock.mock.calls[0];
  expect(url).toMatch(new RegExp(`/api${urlSuffix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`));
  expect((opts as { method: string }).method).toBe(verb);
  const body = (opts as { body?: string }).body;
  return body ? JSON.parse(body) : undefined;
}

// Helper : wrap call + verify shape. Returns the parsed body.
async function exec(call: Promise<unknown>, verb: string, url: string) {
  await call;
  return expectFetch(verb, url);
}

// ════════════════════════════════════════════════════════════════════════════
// Setup namespace (5 methods)
// ════════════════════════════════════════════════════════════════════════════
describe('api.setup', () => {
  it('getStatus → GET /setup/status', async () => {
    await exec(setup.getStatus(), 'GET', '/setup/status');
  });
  it('setScanPaths → POST /setup/scan-paths', async () => {
    await exec(setup.setScanPaths({ paths: ['/x'] }), 'POST', '/setup/scan-paths');
  });
  it('installAgent → POST /setup/install-agent', async () => {
    await exec(setup.installAgent('ClaudeCode'), 'POST', '/setup/install-agent');
  });
  it('complete → POST /setup/complete', async () => {
    await exec(setup.complete(), 'POST', '/setup/complete');
  });
  it('reset → POST /setup/reset', async () => {
    await exec(setup.reset(), 'POST', '/setup/reset');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// Version (1 method)
// ════════════════════════════════════════════════════════════════════════════
describe('api.version', () => {
  it('check → GET /version/check', async () => {
    await exec(version.check(), 'GET', '/version/check');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// Config (~30 methods)
// ════════════════════════════════════════════════════════════════════════════
describe('api.config', () => {
  it('getTokens', async () => { await exec(config.getTokens(), 'GET', '/config/tokens'); });
  it('saveApiKey', async () => { await exec(config.saveApiKey({ provider: 'anthropic', key: 'sk-x', name: 'n' } as never), 'POST', '/config/api-keys'); });
  it('deleteApiKey', async () => { await exec(config.deleteApiKey('id-1'), 'DELETE', '/config/api-keys/id-1'); });
  it('activateApiKey', async () => { await exec(config.activateApiKey('id-1'), 'POST', '/config/api-keys/id-1/activate'); });
  it('syncAgentTokens', async () => { await exec(config.syncAgentTokens(), 'POST', '/config/sync-agent-tokens'); });
  it('discoverKeys', async () => { await exec(config.discoverKeys(), 'POST', '/config/discover-keys'); });
  it('toggleTokenOverride', async () => { await exec(config.toggleTokenOverride('anthropic'), 'POST', '/config/toggle-token-override'); });
  it('getLanguage', async () => { await exec(config.getLanguage(), 'GET', '/config/language'); });
  it('saveLanguage', async () => { await exec(config.saveLanguage('fr'), 'POST', '/config/language'); });
  it('getUiLanguage', async () => { await exec(config.getUiLanguage(), 'GET', '/config/ui-language'); });
  it('saveUiLanguage', async () => { await exec(config.saveUiLanguage('en'), 'POST', '/config/ui-language'); });
  it('getSttModel', async () => { await exec(config.getSttModel(), 'GET', '/config/stt-model'); });
  it('saveSttModel', async () => { await exec(config.saveSttModel('m'), 'POST', '/config/stt-model'); });
  it('getTtsVoices', async () => { await exec(config.getTtsVoices(), 'GET', '/config/tts-voices'); });
  it('saveTtsVoice', async () => {
    const body = await exec(config.saveTtsVoice('fr', 'voice-1'), 'POST', '/config/tts-voice');
    expect(body).toEqual({ lang: 'fr', voice_id: 'voice-1' });
  });
  it('getGlobalContext', async () => { await exec(config.getGlobalContext(), 'GET', '/config/global-context'); });
  it('saveGlobalContext', async () => { await exec(config.saveGlobalContext('hi'), 'POST', '/config/global-context'); });
  it('getGlobalContextMode', async () => { await exec(config.getGlobalContextMode(), 'GET', '/config/global-context-mode'); });
  it('saveGlobalContextMode', async () => { await exec(config.saveGlobalContextMode('always'), 'POST', '/config/global-context-mode'); });
  it('getAntiHallucinationMode', async () => { await exec(config.getAntiHallucinationMode(), 'GET', '/config/anti-hallucination-mode'); });
  it('saveAntiHallucinationMode', async () => { await exec(config.saveAntiHallucinationMode('warn'), 'POST', '/config/anti-hallucination-mode'); });
  it('getScanPaths', async () => { await exec(config.getScanPaths(), 'GET', '/config/scan-paths'); });
  it('setScanPaths', async () => {
    const body = await exec(config.setScanPaths(['/a', '/b']), 'POST', '/config/scan-paths');
    expect(body).toEqual({ paths: ['/a', '/b'] });
  });
  it('getScanIgnore', async () => { await exec(config.getScanIgnore(), 'GET', '/config/scan-ignore'); });
  it('setScanIgnore', async () => { await exec(config.setScanIgnore(['*.tmp']), 'POST', '/config/scan-ignore'); });
  it('getScanDepth', async () => { await exec(config.getScanDepth(), 'GET', '/config/scan-depth'); });
  it('setScanDepth', async () => { await exec(config.setScanDepth(5), 'POST', '/config/scan-depth'); });
  it('getAgentAccess', async () => { await exec(config.getAgentAccess(), 'GET', '/config/agent-access'); });
  it('setAgentAccess', async () => { await exec(config.setAgentAccess({ agents: {} } as never), 'POST', '/config/agent-access'); });
  it('getModelTiers', async () => { await exec(config.getModelTiers(), 'GET', '/config/model-tiers'); });
  it('setModelTiers', async () => { await exec(config.setModelTiers({} as never), 'POST', '/config/model-tiers'); });
  it('dbInfo', async () => { await exec(config.dbInfo(), 'GET', '/config/db-info'); });
  it('dbBackup', async () => { await exec(config.dbBackup(), 'POST', '/db/backup'); });
  it('getServerConfig', async () => { await exec(config.getServerConfig(), 'GET', '/config/server'); });
  it('setServerConfig', async () => { await exec(config.setServerConfig({ pseudo: 'x' }), 'POST', '/config/server'); });
  it('regenerateAuthToken', async () => { await exec(config.regenerateAuthToken(), 'POST', '/config/auth-token/regenerate'); });
});

// ════════════════════════════════════════════════════════════════════════════
// Contacts
// ════════════════════════════════════════════════════════════════════════════
describe('api.contacts', () => {
  it('list', async () => { await exec(contacts.list(), 'GET', '/contacts'); });
  it('add', async () => { await exec(contacts.add('inv-code'), 'POST', '/contacts'); });
  if ('delete' in contacts) {
    it('delete', async () => {
      await exec((contacts as { delete: (id: string) => Promise<unknown> }).delete('c1'), 'DELETE', '/contacts/c1');
    });
  }
  if ('inviteCode' in contacts) {
    it('inviteCode', async () => {
      await exec((contacts as { inviteCode: () => Promise<unknown> }).inviteCode(), 'GET', '/contacts/invite-code');
    });
  }
  if ('networkInfo' in contacts) {
    it('networkInfo', async () => {
      await exec((contacts as { networkInfo: () => Promise<unknown> }).networkInfo(), 'GET', '/contacts/network-info');
    });
  }
  if ('ping' in contacts) {
    it('ping', async () => {
      await exec((contacts as { ping: (id: string) => Promise<unknown> }).ping('c1'), 'GET', '/contacts/c1/ping');
    });
  }
});

// ════════════════════════════════════════════════════════════════════════════
// Agents
// ════════════════════════════════════════════════════════════════════════════
describe('api.agents', () => {
  it('detect', async () => { await exec(agents.detect(), 'GET', '/agents'); });
  it('install', async () => { await exec(agents.install('ClaudeCode'), 'POST', '/agents/install'); });
  it('uninstall', async () => { await exec(agents.uninstall('ClaudeCode'), 'POST', '/agents/uninstall'); });
  it('toggle', async () => { await exec(agents.toggle('ClaudeCode'), 'POST', '/agents/toggle'); });
});

// ════════════════════════════════════════════════════════════════════════════
// MCPs
// ════════════════════════════════════════════════════════════════════════════
describe('api.mcps', () => {
  it('registry', async () => { await exec(mcps.registry(), 'GET', '/mcps/registry'); });
  it('overview', async () => { await exec(mcps.overview(), 'GET', '/mcps'); });
});

// ════════════════════════════════════════════════════════════════════════════
// Skills / Profiles / Directives (CRUDs)
// ════════════════════════════════════════════════════════════════════════════
describe('api.skills', () => {
  it('list', async () => { await exec(skills.list(), 'GET', '/skills'); });
  it('create', async () => { await exec(skills.create({ id: 'x', name: 'X', description: '', icon: '', category: 'Domain', content: '' } as never), 'POST', '/skills'); });
  it('update', async () => { await exec(skills.update('id-1', { name: 'Y' } as never), 'PUT', '/skills/id-1'); });
  it('delete', async () => { await exec(skills.delete('id-1'), 'DELETE', '/skills/id-1'); });
});
describe('api.profiles', () => {
  it('list', async () => { await exec(profiles.list(), 'GET', '/profiles'); });
  it('create', async () => { await exec(profiles.create({ name: 'n', persona_prompt: 'p' } as never), 'POST', '/profiles'); });
  it('update', async () => { await exec(profiles.update('p-1', { name: 'n' } as never), 'PUT', '/profiles/p-1'); });
  it('delete', async () => { await exec(profiles.delete('p-1'), 'DELETE', '/profiles/p-1'); });
  if ('updatePersonaName' in profiles) {
    it('updatePersonaName', async () => {
      await exec((profiles as { updatePersonaName: (id: string, name: string) => Promise<unknown> }).updatePersonaName('p-1', 'Alpha'), 'PUT', '/profiles/p-1/persona-name');
    });
  }
});
describe('api.directives', () => {
  it('list', async () => { await exec(directives.list(), 'GET', '/directives'); });
  it('create', async () => { await exec(directives.create({ name: 'n' } as never), 'POST', '/directives'); });
  it('update', async () => { await exec(directives.update('d-1', { name: 'n' } as never), 'PUT', '/directives/d-1'); });
  it('delete', async () => { await exec(directives.delete('d-1'), 'DELETE', '/directives/d-1'); });
});

// ════════════════════════════════════════════════════════════════════════════
// Stats
// ════════════════════════════════════════════════════════════════════════════
describe('api.stats', () => {
  if ('agentUsage' in stats) {
    it('agentUsage', async () => {
      await exec((stats as { agentUsage: () => Promise<unknown> }).agentUsage(), 'GET', '/stats/agent-usage');
    });
  }
});

// ════════════════════════════════════════════════════════════════════════════
// Workflows (a subset — the SSE methods need separate harness)
// ════════════════════════════════════════════════════════════════════════════
describe('api.workflows', () => {
  it('list', async () => { await exec(workflows.list(), 'GET', '/workflows'); });
  it('get', async () => { await exec(workflows.get('wf-1'), 'GET', '/workflows/wf-1'); });
  it('create', async () => { await exec(workflows.create({ name: 'n' } as never), 'POST', '/workflows'); });
  it('update', async () => { await exec(workflows.update('wf-1', { name: 'n' } as never), 'PUT', '/workflows/wf-1'); });
  it('delete', async () => { await exec(workflows.delete('wf-1'), 'DELETE', '/workflows/wf-1'); });
  it('trigger', async () => { await exec(workflows.trigger('wf-1'), 'POST', '/workflows/wf-1/trigger'); });
  if ('listRuns' in workflows) {
    it('listRuns', async () => { await exec((workflows as { listRuns: (id: string) => Promise<unknown> }).listRuns('wf-1'), 'GET', '/workflows/wf-1/runs'); });
  }
  if ('getRun' in workflows) {
    it('getRun', async () => { await exec((workflows as { getRun: (wfId: string, runId: string) => Promise<unknown> }).getRun('wf-1', 'r-1'), 'GET', '/workflows/wf-1/runs/r-1'); });
  }
  if ('deleteRun' in workflows) {
    it('deleteRun', async () => { await exec((workflows as { deleteRun: (wfId: string, runId: string) => Promise<unknown> }).deleteRun('wf-1', 'r-1'), 'DELETE', '/workflows/wf-1/runs/r-1'); });
  }
});

// ════════════════════════════════════════════════════════════════════════════
// Quick Prompts / Quick APIs
// ════════════════════════════════════════════════════════════════════════════
describe('api.quickPrompts', () => {
  it('list', async () => { await exec(quickPrompts.list(), 'GET', '/quick-prompts'); });
  it('create', async () => { await exec(quickPrompts.create({ name: 'n' } as never), 'POST', '/quick-prompts'); });
  it('update', async () => { await exec(quickPrompts.update('qp-1', {} as never), 'PUT', '/quick-prompts/qp-1'); });
  it('delete', async () => { await exec(quickPrompts.delete('qp-1'), 'DELETE', '/quick-prompts/qp-1'); });
});
describe('api.quickApis', () => {
  it('list', async () => { await exec(quickApis.list(), 'GET', '/quick-apis'); });
  it('create', async () => { await exec(quickApis.create({ name: 'n' } as never), 'POST', '/quick-apis'); });
});

// ════════════════════════════════════════════════════════════════════════════
// Discussions (the safest non-streaming subset)
// ════════════════════════════════════════════════════════════════════════════
describe('api.discussions', () => {
  it('list', async () => { await exec(discussions.list(), 'GET', '/discussions'); });
  it('get', async () => { await exec(discussions.get('d-1'), 'GET', '/discussions/d-1'); });
  it('create', async () => { await exec(discussions.create({ title: 't' } as never), 'POST', '/discussions'); });
  it('delete', async () => { await exec(discussions.delete('d-1'), 'DELETE', '/discussions/d-1'); });
  it('update', async () => { await exec(discussions.update('d-1', { title: 'x' } as never), 'PATCH', '/discussions/d-1'); });
});

// ════════════════════════════════════════════════════════════════════════════
// Smaller namespaces — quick smoke
// ════════════════════════════════════════════════════════════════════════════
describe('api.rtk', () => {
  it('activate', async () => { await exec(rtk.activate(['ClaudeCode']), 'POST', '/rtk/activate'); });
  it('deactivate', async () => { await exec(rtk.deactivate(['ClaudeCode']), 'POST', '/rtk/deactivate'); });
  it('savings', async () => { await exec(rtk.savings(), 'GET', '/rtk/savings'); });
  it('version', async () => { await exec(rtk.version(), 'GET', '/rtk/version'); });
});
describe('api.usage', () => {
  it('get default', async () => { await exec(usage.get(), 'GET', '/usage?period=daily'); });
  it('get weekly', async () => { await exec(usage.get('weekly'), 'GET', '/usage?period=weekly'); });
});
describe('api.ollama', () => {
  if ('health' in ollama) {
    it('health', async () => { await exec((ollama as { health: () => Promise<unknown> }).health(), 'GET', '/ollama/health'); });
  }
  if ('models' in ollama) {
    it('models', async () => { await exec((ollama as { models: () => Promise<unknown> }).models(), 'GET', '/ollama/models'); });
  }
});
describe('api.apiCallLogs', () => {
  if ('list' in apiCallLogs) {
    it('list', async () => {
      // apiCallLogs.list appends an empty querystring when filter is {},
      // so the path ends with `?`. Match with a loose suffix.
      await (apiCallLogs as { list: (q?: unknown) => Promise<unknown> }).list({});
      expect(fetchMock).toHaveBeenCalledTimes(1);
      const [url, opts] = fetchMock.mock.calls[0];
      expect(url).toMatch(/\/api\/api-call-logs(\?.*)?$/);
      expect((opts as { method: string }).method).toBe('GET');
    });
  }
});
describe('api.debugApi', () => {
  it('getLogs', async () => { await exec(debugApi.getLogs(50), 'GET', '/debug/logs?lines=50'); });
  it('clearLogs', async () => { await exec(debugApi.clearLogs(), 'POST', '/debug/logs/clear'); });
});
describe('api.themes', () => {
  it('unlock', async () => { await exec(themes.unlock('secret-code'), 'POST', '/themes/unlock'); });
});
describe('api.docs', () => {
  it('generatePdf', async () => {
    await exec(docs.generatePdf({ discussion_id: 'd1', html: '<p>x</p>' }), 'POST', '/docs/pdf');
  });
  it('generateDocx', async () => {
    await exec(docs.generateDocx({ discussion_id: 'd1', html: '<p>x</p>' }), 'POST', '/docs/docx');
  });
  it('generateXlsx', async () => {
    await exec(
      docs.generateXlsx({ discussion_id: 'd1', sheets: [{ name: 'S', rows: [['a', 'b']] }] }),
      'POST',
      '/docs/xlsx',
    );
  });
  it('generateCsv', async () => {
    await exec(docs.generateCsv({ discussion_id: 'd1', rows: [['a', 'b']] }), 'POST', '/docs/csv');
  });
  it('generatePptx', async () => {
    await exec(
      docs.generatePptx({ discussion_id: 'd1', slides: [{ title: 'T', bullets: ['x'] }] }),
      'POST',
      '/docs/pptx',
    );
  });
});
describe('api.autoTriggersApi', () => {
  it('listDisabled', async () => {
    await exec(autoTriggersApi.listDisabled(), 'GET', '/skills/auto-triggers/disabled');
  });
  it('toggle', async () => {
    await exec(autoTriggersApi.toggle('skill-1'), 'POST', '/skills/skill-1/auto-trigger/toggle');
  });
});
describe('api.userContext', () => {
  if ('list' in userContext) {
    it('list', async () => { await exec((userContext as { list: () => Promise<unknown> }).list(), 'GET', '/user-context'); });
  }
});

// ════════════════════════════════════════════════════════════════════════════
// Projects (the big one — ~50 methods)
// ════════════════════════════════════════════════════════════════════════════
describe('api.projects', () => {
  it('list', async () => { await exec(projects.list(), 'GET', '/projects'); });
  it('get', async () => { await exec(projects.get('p-1'), 'GET', '/projects/p-1'); });
  it('scan', async () => { await exec(projects.scan(), 'POST', '/projects/scan'); });
  it('create', async () => { await exec(projects.create({ name: 'n', path: '/p' } as never), 'POST', '/projects'); });
  it('delete', async () => { await exec(projects.delete('p-1'), 'DELETE', '/projects/p-1'); });
  it('bootstrap', async () => { await exec(projects.bootstrap({ name: 'n', description: 'd', agent: 'ClaudeCode' } as never), 'POST', '/projects/bootstrap'); });
  it('clone', async () => { await exec(projects.clone({ url: 'https://x.git', name: null, agent: 'ClaudeCode' } as never), 'POST', '/projects/clone'); });
  it('discoverRepos', async () => { await exec(projects.discoverRepos({ source_ids: [] }), 'POST', '/projects/discover-repos'); });
  it('installTemplate', async () => { await exec(projects.installTemplate('p-1'), 'POST', '/projects/p-1/install-template'); });
  it('checkDrift', async () => { await exec(projects.checkDrift('p-1'), 'GET', '/projects/p-1/drift'); });
  it('cancelAudit', async () => { await exec(projects.cancelAudit('p-1'), 'POST', '/projects/p-1/cancel-audit'); });
  it('markBootstrapped', async () => { await exec(projects.markBootstrapped('p-1'), 'POST', '/projects/p-1/mark-bootstrapped'); });
  if ('startBriefing' in projects) {
    it('startBriefing', async () => { await exec((projects as { startBriefing: (id: string, agent: string) => Promise<unknown> }).startBriefing('p-1', 'ClaudeCode'), 'POST', '/projects/p-1/start-briefing'); });
  }
  if ('getBriefing' in projects) {
    it('getBriefing', async () => { await exec((projects as { getBriefing: (id: string) => Promise<unknown> }).getBriefing('p-1'), 'GET', '/projects/p-1/briefing'); });
  }
  if ('setBriefing' in projects) {
    it('setBriefing', async () => { await exec((projects as { setBriefing: (id: string, content: string | null) => Promise<unknown> }).setBriefing('p-1', 'x'), 'PUT', '/projects/p-1/briefing'); });
  }
  if ('setDefaultSkills' in projects) {
    it('setDefaultSkills', async () => { await exec((projects as { setDefaultSkills: (id: string, ids: string[]) => Promise<unknown> }).setDefaultSkills('p-1', []), 'PUT', '/projects/p-1/default-skills'); });
  }
  if ('setDefaultProfile' in projects) {
    it('setDefaultProfile', async () => { await exec((projects as { setDefaultProfile: (id: string, pid: string | null) => Promise<unknown> }).setDefaultProfile('p-1', null), 'PUT', '/projects/p-1/default-profile'); });
  }
});

// ════════════════════════════════════════════════════════════════════════════
// Core api() wrapper — error paths (the actual ApiResponse envelope handling)
// ════════════════════════════════════════════════════════════════════════════
describe('api() wrapper', () => {
  it('attaches Authorization header when authToken is set', async () => {
    setAuthToken('my-secret-token');
    await config.getLanguage();
    const [, opts] = fetchMock.mock.calls[0];
    expect((opts as { headers: Record<string, string> }).headers.Authorization).toBe('Bearer my-secret-token');
    setAuthToken(null);
  });

  it('omits Authorization header when no token', async () => {
    await config.getLanguage();
    const [, opts] = fetchMock.mock.calls[0];
    expect((opts as { headers: Record<string, string> }).headers.Authorization).toBeUndefined();
  });

  it('prefixes apiBase to the URL when set', async () => {
    setApiBase('http://localhost:3140');
    await config.getLanguage();
    const [url] = fetchMock.mock.calls[0];
    expect(url).toBe('http://localhost:3140/api/config/language');
    setApiBase('');
  });

  it('strips trailing slash from apiBase', async () => {
    setApiBase('http://localhost:3140/');
    await config.getLanguage();
    const [url] = fetchMock.mock.calls[0];
    expect(url).toBe('http://localhost:3140/api/config/language');
    setApiBase('');
  });

  it('throws with non-JSON 4xx body (axum Json<T> extractor case)', async () => {
    fetchMock.mockResolvedValue({
      ok: false, status: 422,
      headers: { get: () => 'text/plain' },
      text: async () => 'missing field `name`',
    });
    await expect(config.getLanguage()).rejects.toThrow(/HTTP 422.*missing field/);
  });

  it('throws with non-JSON 5xx body (nginx HTML page) capped at 500 chars', async () => {
    const big = 'A'.repeat(10_000);
    fetchMock.mockResolvedValue({
      ok: false, status: 502,
      headers: { get: () => 'text/html' },
      text: async () => `<html>${big}</html>`,
    });
    await expect(config.getLanguage()).rejects.toThrow(/HTTP 502/);
  });

  it('throws when ApiResponse.success === false', async () => {
    fetchMock.mockResolvedValue({
      ok: true, status: 200,
      headers: { get: () => 'application/json' },
      json: async () => ({ success: false, error: 'project not found', data: null }),
    });
    await expect(config.getLanguage()).rejects.toThrow('project not found');
  });

  it('throws with a default error message when success=false and no error string', async () => {
    fetchMock.mockResolvedValue({
      ok: true, status: 200,
      headers: { get: () => 'application/json' },
      json: async () => ({ success: false, data: null }),
    });
    await expect(config.getLanguage()).rejects.toThrow(/Unknown API error/);
  });

  it('sets Content-Type: application/json when posting a body', async () => {
    await config.saveLanguage('fr');
    const [, opts] = fetchMock.mock.calls[0];
    expect((opts as { headers: Record<string, string> }).headers['Content-Type']).toBe('application/json');
  });

  it('serializes the body as JSON', async () => {
    await config.setScanPaths(['/a']);
    const [, opts] = fetchMock.mock.calls[0];
    expect((opts as { body: string }).body).toBe(JSON.stringify({ paths: ['/a'] }));
  });

  it('omits the body on GET requests', async () => {
    await config.getLanguage();
    const [, opts] = fetchMock.mock.calls[0];
    expect((opts as { body?: string }).body).toBeUndefined();
  });
});
