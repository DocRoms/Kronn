import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// We mock `fetch` globally to test the internal `api()` function behavior.
beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

function mockFetchResponse(data: unknown, success = true, status = 200) {
  (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
    status,
    headers: {
      get: (name: string) => name === 'content-type' ? 'application/json' : null,
    },
    json: () => Promise.resolve({ success, data, error: success ? null : (data as string) }),
  });
}

function mockFetchError(error: string, status = 400) {
  (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
    status,
    headers: {
      get: (name: string) => name === 'content-type' ? 'application/json' : null,
    },
    json: () => Promise.resolve({ success: false, data: null, error }),
  });
}

function mockFetchHtml(status = 502) {
  (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
    status,
    headers: {
      get: (name: string) => name === 'content-type' ? 'text/html' : null,
    },
    text: () => Promise.resolve('<html>Bad Gateway</html>'),
  });
}

describe('api module', () => {
  async function getApi() {
    const mod = await import('../api');
    return mod;
  }

  // ─── setApiBase / getApiBase ─────────────────────────────────────────

  describe('setApiBase / getApiBase', () => {
    it('sets and gets the API base URL', async () => {
      const { setApiBase, getApiBase } = await getApi();
      setApiBase('http://localhost:3000');
      expect(getApiBase()).toBe('http://localhost:3000');
    });

    it('strips trailing slash', async () => {
      const { setApiBase, getApiBase } = await getApi();
      setApiBase('http://localhost:3000/');
      expect(getApiBase()).toBe('http://localhost:3000');
    });

    it('returns empty string by default', async () => {
      const { setApiBase, getApiBase } = await getApi();
      setApiBase('');
      expect(getApiBase()).toBe('');
    });
  });

  // ─── Structure tests ──────────────────────────────────────────────────

  describe('API structure', () => {
    it('exports all expected namespaces', async () => {
      const api = await getApi();
      expect(api.setup).toBeDefined();
      expect(api.config).toBeDefined();
      expect(api.projects).toBeDefined();
      expect(api.agents).toBeDefined();
      expect(api.mcps).toBeDefined();
      expect(api.discussions).toBeDefined();
      expect(api.workflows).toBeDefined();
      expect(api.skills).toBeDefined();
      expect(api.stats).toBeDefined();
      expect(api.profiles).toBeDefined();
      expect(api.directives).toBeDefined();
    });

    it('projects has briefing methods', async () => {
      const { projects } = await getApi();
      expect(typeof (projects as Record<string, unknown>)['startBriefing']).toBe('function');
      expect(typeof (projects as Record<string, unknown>)['getBriefing']).toBe('function');
      expect(typeof (projects as Record<string, unknown>)['setBriefing']).toBe('function');
    });

    it('projects has checkDrift method', async () => {
      const { projects } = await getApi();
      expect(typeof (projects as Record<string, unknown>)['checkDrift']).toBe('function');
    });

    it('projects has expected methods', async () => {
      const { projects } = await getApi();
      const expected = [
        'list', 'get', 'scan', 'create', 'bootstrap', 'delete', 'clone',
        'discoverRepos', 'installTemplate', 'auditInfo', 'auditStream',
        'fullAuditStream', 'validateAudit', 'markBootstrapped', 'cancelAudit',
        'setDefaultSkills', 'setDefaultProfile',
        'listAiFiles', 'readAiFile', 'searchAiFiles',
        'gitStatus', 'gitDiff', 'gitCreateBranch', 'gitCommit', 'gitPush',
        'exec',
      ];
      for (const method of expected) {
        expect(typeof (projects as Record<string, unknown>)[method]).toBe('function');
      }
    });

    it('workflows has expected methods', async () => {
      const { workflows } = await getApi();
      const expected = ['list', 'get', 'create', 'update', 'delete', 'trigger', 'triggerStream', 'listRuns', 'getRun', 'deleteRun', 'deleteAllRuns'];
      for (const method of expected) {
        expect(typeof (workflows as Record<string, unknown>)[method]).toBe('function');
      }
    });

    it('discussions has expected methods', async () => {
      const { discussions } = await getApi();
      const expected = ['list', 'get', 'create', 'delete', 'update', '_streamSSE', 'sendMessageStream', 'runAgent', 'orchestrate', 'deleteLastAgentMessages', 'editLastUserMessage'];
      for (const method of expected) {
        expect(typeof (discussions as Record<string, unknown>)[method]).toBe('function');
      }
    });

    it('config has expected methods', async () => {
      const { config } = await getApi();
      const expected = ['getTokens', 'saveApiKey', 'deleteApiKey', 'activateApiKey', 'syncAgentTokens', 'discoverKeys', 'getLanguage', 'saveLanguage', 'getScanPaths', 'setScanPaths', 'dbInfo', 'exportData', 'importData', 'getServerConfig', 'setServerConfig', 'regenerateAuthToken'];
      for (const method of expected) {
        expect(typeof (config as Record<string, unknown>)[method]).toBe('function');
      }
    });
  });

  // ─── Error handling ───────────────────────────────────────────────────

  describe('error handling', () => {
    it('throws on API error response with message', async () => {
      mockFetchError('Project not found');
      const { projects } = await getApi();
      await expect(projects.get('nonexistent')).rejects.toThrow('Project not found');
    });

    it('throws on non-JSON response (e.g. 502 gateway)', async () => {
      mockFetchHtml(502);
      const { projects } = await getApi();
      await expect(projects.list()).rejects.toThrow('Server error (HTTP 502)');
    });

    it('throws "Unknown API error" when error field is null', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        status: 500,
        headers: {
          get: (name: string) => name === 'content-type' ? 'application/json' : null,
        },
        json: () => Promise.resolve({ success: false, data: null, error: null }),
      });
      const { projects } = await getApi();
      await expect(projects.list()).rejects.toThrow('Unknown API error');
    });

    it('throws on network failure', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockRejectedValue(new TypeError('Failed to fetch'));
      const { projects } = await getApi();
      await expect(projects.list()).rejects.toThrow('Failed to fetch');
    });
  });

  // ─── Auth token management ────────────────────────────────────────────

  describe('auth token', () => {
    it('setAuthToken and getAuthToken work together', async () => {
      const { setAuthToken, getAuthToken } = await getApi();
      setAuthToken('test-token-123');
      expect(getAuthToken()).toBe('test-token-123');
      setAuthToken(null);
      expect(getAuthToken()).toBeNull();
    });

    it('authHeaders returns empty when no token set', async () => {
      const { authHeaders, setAuthToken } = await getApi();
      setAuthToken(null);
      expect(authHeaders()).toEqual({});
    });

    it('authHeaders includes Bearer token when set', async () => {
      const { authHeaders, setAuthToken } = await getApi();
      setAuthToken('my-token');
      expect(authHeaders()).toEqual({ Authorization: 'Bearer my-token' });
      setAuthToken(null); // cleanup
    });
  });

  // ─── Git API (simplified) ────────────────────────────────────────────

  describe('git API', () => {
    it('gitStatus returns parsed response', async () => {
      mockFetchResponse({ branch: 'main', default_branch: 'main', is_default_branch: true, files: [], ahead: 0, behind: 0 });
      const { projects } = await getApi();
      const result = await projects.gitStatus('proj-1');
      expect(result.branch).toBe('main');
      expect(result.files).toEqual([]);
    });

    it('gitDiff encodes path in URL', async () => {
      mockFetchResponse({ path: 'src/main.rs', diff: '+added line' });
      const { projects } = await getApi();
      await projects.gitDiff('proj-1', 'src/main.rs');
      expect(globalThis.fetch).toHaveBeenCalledWith(
        '/api/projects/proj-1/git-diff?path=src%2Fmain.rs',
        expect.objectContaining({ method: 'GET' }),
      );
    });

    it('gitCommit sends files and message', async () => {
      mockFetchResponse({ hash: 'abc1234', message: 'fix bug' });
      const { projects } = await getApi();
      const result = await projects.gitCommit('proj-1', { files: ['src/main.rs'], message: 'fix bug' });
      expect(result.hash).toBe('abc1234');
    });
  });
});
