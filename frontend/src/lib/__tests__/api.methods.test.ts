/**
 * 2026-05-29 — api.ts is the single boundary between every UI surface and
 * the backend: a wrong verb, a typo'd path, or a botched query-string is a
 * production break that no page-level test would localize. This file makes
 * `lib/api.ts` the best-covered file in the front by exercising EVERY
 * remaining method (verb + URL + query/path encoding + body shape) that
 * `api.coverage.test.ts` did not already pin.
 *
 * Companion to `api.coverage.test.ts` (which holds the wrapper error-path
 * suite + the first wave of namespaces). Split into a second file purely to
 * keep the working suite untouched. The streaming/SSE methods
 * (auditStream, _streamSSE, triggerStream, …) live in `api.streaming.test.ts`
 * — they need a ReadableStream harness, not this fetch-shape one.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import {
  config, projects, mcps, discussions, workflows, quickPrompts, quickApis,
  profiles, stats, apiCallLogs, userContext, setApiBase, setAuthToken, health,
} from '../api';

let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  fetchMock = vi.fn();
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockResolvedValue({
    ok: true,
    status: 200,
    headers: { get: (name: string) => (name === 'content-type' ? 'application/json' : null) },
    json: async () => ({ success: true, data: null }),
    text: async () => '',
    blob: async () => new Blob(['x']),
    body: null,
  });
  setApiBase('');
  setAuthToken(null);
});

afterEach(() => {
  vi.restoreAllMocks();
});

/** Assert the single fetch call matched (verb, urlSuffix-ending). Returns parsed JSON body if any. */
async function expectFetch(verb: string, urlSuffix: string): Promise<unknown> {
  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [url, opts] = fetchMock.mock.calls[0];
  expect(url).toMatch(new RegExp(`/api${urlSuffix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`));
  expect((opts as { method: string }).method).toBe(verb);
  const body = (opts as { body?: string }).body;
  return typeof body === 'string' ? JSON.parse(body) : undefined;
}

async function exec(call: Promise<unknown>, verb: string, url: string) {
  await call;
  return expectFetch(verb, url);
}

/** For FormData / blob bodies where JSON.parse would throw — assert verb+url only, return raw body. */
async function execRaw(call: Promise<unknown>, verb: string, urlRe: RegExp): Promise<unknown> {
  await call;
  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [url, opts] = fetchMock.mock.calls[0];
  expect(url).toMatch(urlRe);
  if (verb !== 'GET') expect((opts as { method?: string }).method).toBe(verb);
  return (opts as { body?: unknown }).body;
}

// ════════════════════════════════════════════════════════════════════════════
// config — the 2 non-standard (blob export / FormData import) methods
// ════════════════════════════════════════════════════════════════════════════
describe('api.config (blob/form)', () => {
  it('exportData → GET /config/export, returns a Blob', async () => {
    const blob = await config.exportData();
    expect(blob).toBeInstanceOf(Blob);
    const [url, opts] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/config\/export$/);
    expect((opts as { method?: string }).method).toBeUndefined(); // defaults to GET
  });

  it('exportData throws on non-ok response', async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 500, blob: async () => new Blob() });
    await expect(config.exportData()).rejects.toThrow(/Export failed: 500/);
  });

  it('importData → POST /config/import with FormData body', async () => {
    const file = new File(['{}'], 'backup.db');
    const body = await execRaw(config.importData(file), 'POST', /\/api\/config\/import$/);
    expect(body).toBeInstanceOf(FormData);
  });

  it('importData throws when response carries an error field', async () => {
    fetchMock.mockResolvedValue({ ok: true, status: 200, json: async () => ({ error: 'corrupt archive' }) });
    await expect(config.importData(new File([''], 'x'))).rejects.toThrow(/corrupt archive/);
  });

  it('importData throws on non-ok response', async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 413, json: async () => ({}) });
    await expect(config.importData(new File([''], 'x'))).rejects.toThrow(/Import failed: 413/);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// Falsy-but-real bodies must still be sent (regression: the wrapper used to
// drop `false`/`0`/`""` via `if (body)`, so a `Json<bool>` endpoint 422'd —
// which broke DISABLING the continual-learning toggle (POST `false`)).
// ════════════════════════════════════════════════════════════════════════════
describe('api wrapper — falsy bodies', () => {
  it('saveContinualLearningEnabled(false) sends a body of `false` with JSON content-type', async () => {
    await config.saveContinualLearningEnabled(false);
    const [, opts] = fetchMock.mock.calls[0];
    const o = opts as { body?: string; headers?: Record<string, string> };
    expect(o.body).toBe('false');
    expect(o.headers?.['Content-Type']).toBe('application/json');
  });

  it('saveContinualLearningEnabled(true) sends a body of `true`', async () => {
    await config.saveContinualLearningEnabled(true);
    const [, opts] = fetchMock.mock.calls[0];
    expect((opts as { body?: string }).body).toBe('true');
  });

  it('a GET with no body sends no body and no JSON content-type', async () => {
    await config.getContinualLearningEnabled();
    const [, opts] = fetchMock.mock.calls[0];
    const o = opts as { body?: string; headers?: Record<string, string> };
    expect(o.body).toBeUndefined();
    expect(o.headers?.['Content-Type']).toBeUndefined();
  });
});

// ════════════════════════════════════════════════════════════════════════════
// projects — the long tail (audit info, anti-hallu, briefing, linked-repos,
// ai-files, git ops). The SSE audit streams are tested separately.
// ════════════════════════════════════════════════════════════════════════════
describe('api.projects (rest)', () => {
  it('addFolder', async () => { await exec(projects.addFolder({ path: '/p', name: 'n' }), 'POST', '/projects/add-folder'); });
  it('migrateDocs', async () => {
    const b = await exec(projects.migrateDocs('p-1', { create_symlink: true }), 'POST', '/projects/p-1/migrate-docs');
    expect(b).toEqual({ create_symlink: true });
  });
  it('delete (soft)', async () => { await exec(projects.delete('p-1'), 'DELETE', '/projects/p-1'); });
  it('delete (hard) appends ?hard=true', async () => {
    await projects.delete('p-1', true);
    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/projects\/p-1\?hard=true$/);
  });
  it('antiHalluStatus', async () => { await exec(projects.antiHalluStatus('p-1'), 'GET', '/projects/p-1/anti-hallu/status'); });
  it('injectAntiHallu', async () => { await exec(projects.injectAntiHallu('p-1'), 'POST', '/projects/p-1/anti-hallu/inject'); });
  it('syncRedirectors', async () => { await exec(projects.syncRedirectors('p-1'), 'POST', '/projects/p-1/redirectors/sync'); });
  it('auditInfo', async () => { await exec(projects.auditInfo('p-1'), 'GET', '/projects/p-1/audit-info'); });
  it('validateAudit', async () => { await exec(projects.validateAudit('p-1'), 'POST', '/projects/p-1/validate-audit'); });
  it('auditStatus', async () => { await exec(projects.auditStatus('p-1'), 'GET', '/projects/p-1/audit-status'); });
  it('auditStatusAll', async () => { await exec(projects.auditStatusAll(), 'GET', '/audit-status'); });
  it('auditResumable', async () => { await exec(projects.auditResumable('p-1'), 'GET', '/projects/p-1/audit-resumable'); });
  it('auditLatest', async () => { await exec(projects.auditLatest('p-1'), 'GET', '/projects/p-1/audit-latest'); });
  it('auditHistory', async () => { await exec(projects.auditHistory('p-1'), 'GET', '/projects/p-1/audit-history'); });
  it('auditRunSteps', async () => { await exec(projects.auditRunSteps('run-1'), 'GET', '/audit-runs/run-1/steps'); });
  it('discSources', async () => { await exec(projects.discSources(), 'GET', '/disc/sources'); });
  it('discSourceDetail', async () => { await exec(projects.discSourceDetail('d-1'), 'GET', '/discussions/d-1/source'); });
  it('saveBriefing', async () => {
    const form = { purpose: 'p', team: 't', maturity: 'm', dependencies: 'd', traps: 'x', additional: 'a' };
    const b = await exec(projects.saveBriefing('p-1', form), 'POST', '/projects/p-1/save-briefing');
    expect(b).toEqual(form);
  });
  it('setLinkedRepos', async () => { await exec(projects.setLinkedRepos('p-1', []), 'PUT', '/projects/p-1/linked-repos'); });
  it('linkedReposCandidates', async () => { await exec(projects.linkedReposCandidates('p-1'), 'GET', '/projects/p-1/linked-repos/candidates'); });
  it('listAiFiles', async () => { await exec(projects.listAiFiles('p-1'), 'GET', '/projects/p-1/ai-files'); });
  it('readAiFile encodes the path query', async () => {
    await projects.readAiFile('p-1', 'docs/A B.md');
    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/projects\/p-1\/ai-file\?path=docs%2FA%20B\.md$/);
  });
  it('searchAiFiles encodes the q query', async () => {
    await projects.searchAiFiles('p-1', 'a&b');
    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/projects\/p-1\/ai-search\?q=a%26b$/);
  });
  it('gitStatus', async () => { await exec(projects.gitStatus('p-1'), 'GET', '/projects/p-1/git-status'); });
  it('gitDiff encodes the path query', async () => {
    await projects.gitDiff('p-1', 'src/x.ts');
    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/projects\/p-1\/git-diff\?path=src%2Fx\.ts$/);
  });
  it('gitCreateBranch', async () => { await exec(projects.gitCreateBranch('p-1', { name: 'feat/x' }), 'POST', '/projects/p-1/git-branch'); });
  it('gitCommit', async () => { await exec(projects.gitCommit('p-1', { files: ['a'], message: 'm' }), 'POST', '/projects/p-1/git-commit'); });
  it('gitPush', async () => { await exec(projects.gitPush('p-1'), 'POST', '/projects/p-1/git-push'); });
  it('createPr', async () => { await exec(projects.createPr('p-1', { title: 't' }), 'POST', '/projects/p-1/git-pr'); });
  it('prTemplate', async () => { await exec(projects.prTemplate('p-1'), 'GET', '/projects/p-1/pr-template'); });
  it('exec', async () => {
    const b = await exec(projects.exec('p-1', 'ls -la'), 'POST', '/projects/p-1/exec');
    expect(b).toEqual({ command: 'ls -la' });
  });
  it('remapPath', async () => { await exec(projects.remapPath('p-1', '/new'), 'POST', '/projects/p-1/remap-path'); });
});

// ════════════════════════════════════════════════════════════════════════════
// mcps — config CRUD, custom-spec, host discovery, context files
// ════════════════════════════════════════════════════════════════════════════
describe('api.mcps (rest)', () => {
  it('refresh', async () => { await exec(mcps.refresh(), 'POST', '/mcps/refresh'); });
  it('registry with q encodes the query', async () => {
    await mcps.registry('git lab');
    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/mcps\/registry\?q=git%20lab$/);
  });
  it('createConfig', async () => { await exec(mcps.createConfig({} as never), 'POST', '/mcps/configs'); });
  it('updateConfig', async () => { await exec(mcps.updateConfig('c-1', {} as never), 'PATCH', '/mcps/configs/c-1'); });
  it('updateCustomSpec encodes serverId', async () => {
    await mcps.updateCustomSpec('my srv', {} as never);
    const [url, opts] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/mcps\/custom\/my%20srv$/);
    expect((opts as { method: string }).method).toBe('PUT');
  });
  it('cleanupOrphanEnv', async () => {
    const b = await exec(mcps.cleanupOrphanEnv('srv', ['OLD_KEY']), 'POST', '/mcps/custom/srv/cleanup-orphan-env');
    expect(b).toEqual({ keys: ['OLD_KEY'] });
  });
  it('exportFileUrl returns an encoded URL string (no fetch)', () => {
    expect(mcps.exportFileUrl('my srv')).toBe('/api/mcps/custom/my%20srv/export-file');
    expect(fetchMock).not.toHaveBeenCalled();
  });
  it('importPluginFile', async () => { await exec(mcps.importPluginFile({} as never), 'POST', '/mcps/custom/import-file'); });
  it('deleteConfig', async () => { await exec(mcps.deleteConfig('c-1'), 'DELETE', '/mcps/configs/c-1'); });
  it('setConfigProjects', async () => { await exec(mcps.setConfigProjects('c-1', {} as never), 'PATCH', '/mcps/configs/c-1/projects'); });
  it('revealSecrets', async () => { await exec(mcps.revealSecrets('c-1'), 'POST', '/mcps/configs/c-1/reveal'); });
  it('hostDiscovery', async () => { await exec(mcps.hostDiscovery(), 'GET', '/mcps/host-discovery'); });
  it('adoptHost', async () => { await exec(mcps.adoptHost({} as never), 'POST', '/mcps/host-discovery/adopt'); });
  it('listContexts', async () => { await exec(mcps.listContexts('p-1'), 'GET', '/mcps/context/p-1'); });
  it('getContext', async () => { await exec(mcps.getContext('p-1', 'slug'), 'GET', '/mcps/context/p-1/slug'); });
  it('updateContext', async () => {
    const b = await exec(mcps.updateContext('p-1', 'slug', 'hi'), 'PUT', '/mcps/context/p-1/slug');
    expect(b).toEqual({ content: 'hi' });
  });
});

// ════════════════════════════════════════════════════════════════════════════
// discussions — sharing, participants, git ops, test-mode, context files
// ════════════════════════════════════════════════════════════════════════════
describe('api.discussions (rest)', () => {
  it('share', async () => {
    const b = await exec(discussions.share('d-1', ['c1', 'c2']), 'POST', '/discussions/d-1/share');
    expect(b).toEqual({ contact_ids: ['c1', 'c2'] });
  });
  it('peerJoin trims the token and supplies a web session identity', async () => {
    const b = (await exec(discussions.peerJoin('  kr-join-abc  '), 'POST', '/discussions/peer-join')) as {
      token: string;
      agent_type: string;
      session_id: string;
    };
    expect(b.token).toBe('kr-join-abc');
    expect(b.agent_type).toBe('Custom');
    expect(typeof b.session_id).toBe('string');
    expect(b.session_id.length).toBeGreaterThan(0);
  });
  it('getRunning', async () => { await exec(discussions.getRunning(), 'GET', '/discussions/running'); });
  it('participants', async () => { await exec(discussions.participants('d-1'), 'GET', '/discussions/d-1/participants'); });
  it('invitePeer', async () => { await exec(discussions.invitePeer('d-1'), 'POST', '/discussions/d-1/invite-peer'); });
  it('stop', async () => { await exec(discussions.stop('d-1'), 'POST', '/discussions/d-1/stop'); });
  it('dismissPartial', async () => { await exec(discussions.dismissPartial('d-1'), 'POST', '/discussions/d-1/dismiss-partial'); });
  it('gitStatus', async () => { await exec(discussions.gitStatus('d-1'), 'GET', '/discussions/d-1/git-status'); });
  it('gitDiff', async () => {
    await discussions.gitDiff('d-1', 'a.ts');
    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/discussions\/d-1\/git-diff\?path=a\.ts$/);
  });
  it('gitCommit', async () => { await exec(discussions.gitCommit('d-1', { files: [], message: 'm' }), 'POST', '/discussions/d-1/git-commit'); });
  it('gitPush', async () => { await exec(discussions.gitPush('d-1'), 'POST', '/discussions/d-1/git-push'); });
  it('createPr', async () => { await exec(discussions.createPr('d-1', { title: 't' }), 'POST', '/discussions/d-1/git-pr'); });
  it('prTemplate', async () => { await exec(discussions.prTemplate('d-1'), 'GET', '/discussions/d-1/pr-template'); });
  it('exec', async () => { await exec(discussions.exec('d-1', 'pwd'), 'POST', '/discussions/d-1/exec'); });
  it('worktreeUnlock', async () => { await exec(discussions.worktreeUnlock('d-1'), 'POST', '/discussions/d-1/worktree-unlock'); });
  it('worktreeLock', async () => { await exec(discussions.worktreeLock('d-1'), 'POST', '/discussions/d-1/worktree-lock'); });
  it('testModeEnter (default opts)', async () => {
    const b = await exec(discussions.testModeEnter('d-1'), 'POST', '/discussions/d-1/test-mode/enter');
    expect(b).toEqual({});
  });
  it('testModeEnter (with opts)', async () => {
    const b = await exec(discussions.testModeEnter('d-1', { stash_dirty: true }), 'POST', '/discussions/d-1/test-mode/enter');
    expect(b).toEqual({ stash_dirty: true });
  });
  it('testModeExit', async () => { await exec(discussions.testModeExit('d-1'), 'POST', '/discussions/d-1/test-mode/exit'); });
  it('listContextFiles', async () => { await exec(discussions.listContextFiles('d-1'), 'GET', '/discussions/d-1/context-files'); });
  it('deleteContextFile', async () => { await exec(discussions.deleteContextFile('d-1', 'f-1'), 'DELETE', '/discussions/d-1/context-files/f-1'); });
  it('linkPendingContextFiles → POST with message_id', async () => {
    const b = await exec(discussions.linkPendingContextFiles('d-1', 'm-1'), 'POST', '/discussions/d-1/context-files/link-pending');
    expect(b).toEqual({ message_id: 'm-1' });
  });
  it('contextFileBlob → GET content URL, returns the blob', async () => {
    const blob = new Blob(['png'], { type: 'image/png' });
    fetchMock.mockResolvedValue({ ok: true, status: 200, blob: async () => blob });
    const out = await discussions.contextFileBlob('d-1', 'f-1');
    expect(out).toBe(blob);
    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/discussions\/d-1\/context-files\/f-1\/content$/);
  });
  it('contextFileBlob throws on non-ok', async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 403, blob: async () => new Blob([]) });
    await expect(discussions.contextFileBlob('d-1', 'f-1')).rejects.toThrow(/403/);
  });
  it('uploadContextFile → POST FormData', async () => {
    const body = await execRaw(
      discussions.uploadContextFile('d-1', new File(['x'], 'spec.md')),
      'POST', /\/api\/discussions\/d-1\/context-files$/,
    );
    expect(body).toBeInstanceOf(FormData);
  });
  it('uploadContextFile throws on success=false', async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ success: false, error: 'too big' }) });
    await expect(discussions.uploadContextFile('d-1', new File([''], 'x'))).rejects.toThrow(/too big/);
  });
  it('uploadContextFile surfaces HTTP status on a 413 with no JSON body', async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 413, json: async () => { throw new Error('not json'); } });
    await expect(discussions.uploadContextFile('d-1', new File([''], 'big.har'))).rejects.toThrow(/413/);
  });
  it('deleteLastAgentMessages', async () => { await exec(discussions.deleteLastAgentMessages('d-1'), 'DELETE', '/discussions/d-1/messages/last'); });
  it('editLastUserMessage', async () => {
    const b = await exec(discussions.editLastUserMessage('d-1', 'new text'), 'PATCH', '/discussions/d-1/messages/last');
    expect(b).toEqual({ content: 'new text' });
  });
});

// ════════════════════════════════════════════════════════════════════════════
// workflows — bundles, runs lifecycle, test worktree, dry-run helpers, batch
// ════════════════════════════════════════════════════════════════════════════
describe('api.workflows (rest)', () => {
  it('createBundle', async () => { await exec(workflows.createBundle({}), 'POST', '/workflows/bundle'); });
  it('deleteAllRuns', async () => { await exec(workflows.deleteAllRuns('wf-1'), 'DELETE', '/workflows/wf-1/runs'); });
  it('cancelRun', async () => { await exec(workflows.cancelRun('wf-1', 'r-1'), 'POST', '/workflows/wf-1/runs/r-1/cancel'); });
  it('decideRun', async () => { await exec(workflows.decideRun('wf-1', 'r-1', { decision: 'approve' } as never), 'POST', '/workflows/wf-1/runs/r-1/decide'); });
  it('createTestWorktree (no branchIndex)', async () => {
    const b = await exec(workflows.createTestWorktree('wf-1', 'r-1'), 'POST', '/workflows/wf-1/runs/r-1/test-worktree');
    expect(b).toEqual({});
  });
  it('createTestWorktree (with branchIndex)', async () => {
    const b = await exec(workflows.createTestWorktree('wf-1', 'r-1', 2), 'POST', '/workflows/wf-1/runs/r-1/test-worktree');
    expect(b).toEqual({ branch_index: 2 });
  });
  it('deleteTestWorktree', async () => { await exec(workflows.deleteTestWorktree('wf-1', 'r-1'), 'DELETE', '/workflows/wf-1/runs/r-1/test-worktree'); });
  it('exportWorkflow → GET (relative) /workflows/:id/export, returns filename+blob', async () => {
    const out = await workflows.exportWorkflow('wf-1');
    const [url] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/workflows/wf-1/export');
    expect(out.filename).toBe('workflow-wf-1.kronn-workflow.json'); // header absent → fallback name
    expect(out.blob).toBeInstanceOf(Blob);
  });
  it('exportWorkflow uses content-disposition filename when present', async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      headers: { get: (n: string) => (n === 'content-disposition' ? 'attachment; filename="custom.json"' : null) },
      blob: async () => new Blob(['x']),
    });
    const out = await workflows.exportWorkflow('wf-1');
    expect(out.filename).toBe('custom.json');
  });
  it('exportWorkflow throws on non-ok', async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 404 });
    await expect(workflows.exportWorkflow('wf-1')).rejects.toThrow(/Export failed \(404\)/);
  });
  it('importWorkflow', async () => { await exec(workflows.importWorkflow({} as never), 'POST', '/workflows/import'); });
  it('testBatchStep', async () => { await exec(workflows.testBatchStep({ step: {} as never }), 'POST', '/workflows/test-batch-step'); });
  it('testExtract', async () => { await exec(workflows.testExtract({ sample: {}, path: '$.x' }), 'POST', '/workflow-steps/test-extract'); });
  it('testApiCall', async () => { await exec(workflows.testApiCall({ step: {} as never, project_id: 'p-1' }), 'POST', '/workflow-steps/test-api-call'); });
  it('suggestions', async () => { await exec(workflows.suggestions('p-1'), 'GET', '/projects/p-1/workflow-suggestions'); });
  it('listBatchRunSummaries', async () => { await exec(workflows.listBatchRunSummaries(), 'GET', '/workflow-runs/batch-summaries'); });
  it('deleteBatchRun', async () => { await exec(workflows.deleteBatchRun('r-1'), 'DELETE', '/workflow-runs/r-1'); });
});

// ════════════════════════════════════════════════════════════════════════════
// quickPrompts — batch, compare, export/import, history/metrics, versions
// ════════════════════════════════════════════════════════════════════════════
describe('api.quickPrompts (rest)', () => {
  it('batchRun', async () => { await exec(quickPrompts.batchRun('qp-1', { items: [], batch_name: 'b' }), 'POST', '/quick-prompts/qp-1/batch'); });
  it('compareAgents', async () => {
    await exec(quickPrompts.compareAgents('qp-1', { prompt: 'p', batch_name: 'b', agents: ['ClaudeCode'] }), 'POST', '/quick-prompts/qp-1/compare-agents');
  });
  it('exportQp → GET (relative), filename fallback', async () => {
    const out = await quickPrompts.exportQp('qp-1');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/quick-prompts/qp-1/export');
    expect(out.filename).toBe('quick-prompt-qp-1.kronn-qp.json');
  });
  it('importQp', async () => { await exec(quickPrompts.importQp({} as never), 'POST', '/quick-prompts/import'); });
  it('history', async () => { await exec(quickPrompts.history('qp-1'), 'GET', '/quick-prompts/qp-1/history'); });
  it('metrics', async () => { await exec(quickPrompts.metrics('qp-1'), 'GET', '/quick-prompts/qp-1/metrics'); });
  it('deleteVersion', async () => { await exec(quickPrompts.deleteVersion('qp-1', 3), 'DELETE', '/quick-prompts/qp-1/versions/3'); });
});

// ════════════════════════════════════════════════════════════════════════════
// quickApis — full CRUD + run/batch + export/import
// ════════════════════════════════════════════════════════════════════════════
describe('api.quickApis (rest)', () => {
  it('update', async () => { await exec(quickApis.update('qa-1', {} as never), 'PUT', '/quick-apis/qa-1'); });
  it('delete', async () => { await exec(quickApis.delete('qa-1'), 'DELETE', '/quick-apis/qa-1'); });
  it('runQa', async () => { await exec(quickApis.runQa('qa-1', {} as never), 'POST', '/quick-apis/qa-1/run'); });
  it('batchRunQa', async () => { await exec(quickApis.batchRunQa('qa-1', {} as never), 'POST', '/quick-apis/qa-1/batch'); });
  it('exportQa → GET (relative), filename fallback', async () => {
    const out = await quickApis.exportQa('qa-1');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/quick-apis/qa-1/export');
    expect(out.filename).toBe('quick-api-qa-1.kronn-qa.json');
  });
  it('importQa', async () => { await exec(quickApis.importQa({} as never), 'POST', '/quick-apis/import'); });
});

// ════════════════════════════════════════════════════════════════════════════
// small namespaces — profiles.get, stats.tokenUsage, apiCallLogs, userContext
// ════════════════════════════════════════════════════════════════════════════
describe('api small namespaces (rest)', () => {
  it('profiles.get', async () => { await exec(profiles.get('p-1'), 'GET', '/profiles/p-1'); });
  it('stats.tokenUsage', async () => { await exec(stats.tokenUsage(), 'GET', '/stats/tokens'); });

  it('apiCallLogs.list (no filter)', async () => {
    await apiCallLogs.list();
    expect(fetchMock.mock.calls[0][0]).toMatch(/\/api\/api-call-logs$/);
  });
  it('apiCallLogs.list builds + prunes the querystring', async () => {
    await apiCallLogs.list({ source: 'workflow', plugin_slug: '', limit: 50 });
    const [url] = fetchMock.mock.calls[0];
    // empty plugin_slug dropped; source + limit kept
    expect(url).toMatch(/source=workflow/);
    expect(url).toMatch(/limit=50/);
    expect(url).not.toMatch(/plugin_slug/);
  });
  it('apiCallLogs.get', async () => { await exec(apiCallLogs.get('log-1'), 'GET', '/api-call-logs/log-1'); });
  it('apiCallLogs.purge', async () => {
    const b = await exec(apiCallLogs.purge(30), 'POST', '/api-call-logs/purge');
    expect(b).toEqual({ days: 30 });
  });

  it('userContext.get encodes the name', async () => {
    await userContext.get('my file.md');
    expect(fetchMock.mock.calls[0][0]).toMatch(/\/api\/user-context\/my%20file\.md$/);
  });
  it('userContext.put encodes the name + sends content', async () => {
    const b = await execRaw(userContext.put('a b.md', 'hi'), 'PUT', /\/api\/user-context\/a%20b\.md$/);
    expect(JSON.parse(b as string)).toEqual({ content: 'hi' });
  });
  it('userContext.delete encodes the name', async () => {
    await userContext.delete('a b.md');
    const [url, opts] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/user-context\/a%20b\.md$/);
    expect((opts as { method: string }).method).toBe('DELETE');
  });
});

describe('health.get (raw, unenveloped)', () => {
  it('GETs /api/health and returns the RAW JSON (no {success,data} unwrap)', async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ ok: true, version: '0.8.9', host_os: 'macOS', in_docker: false }),
    });

    const info = await health.get();

    // Raw passthrough is the whole point: in_docker must survive. The standard
    // api<T>() envelope unwrap would have returned `.data` (undefined) instead.
    expect(info).toEqual({ ok: true, version: '0.8.9', host_os: 'macOS', in_docker: false });
    expect(info.in_docker).toBe(false);

    const [url, opts] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/health$/);
    // No explicit method → fetch default GET (never a POST/PUT/etc.).
    expect((opts as { method?: string } | undefined)?.method).toBeUndefined();
  });

  it('surfaces in_docker=true (Docker) distinctly from native', async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ ok: true, version: '0.8.9', host_os: 'Linux', in_docker: true }),
    });

    const info = await health.get();
    expect(info.in_docker).toBe(true);
  });
});
