import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// We need to test the internal `api()` function behavior.
// Since it's not exported, we test via the public API methods.
// We mock `fetch` globally.

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
  // Dynamic import to ensure fresh module after fetch mock
  async function getApi() {
    // Clear module cache for fresh import
    const mod = await import('../api');
    return mod;
  }

  describe('GET requests', () => {
    it('projects.list calls correct endpoint', async () => {
      mockFetchResponse([{ id: '1', name: 'test-project' }]);
      const { projects } = await getApi();

      const result = await projects.list();

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects', {
        method: 'GET',
        headers: {},
        body: undefined,
      });
      expect(result).toEqual([{ id: '1', name: 'test-project' }]);
    });

    it('config.getLanguage returns language string', async () => {
      mockFetchResponse('fr');
      const { config } = await getApi();

      const result = await config.getLanguage();
      expect(result).toBe('fr');
    });
  });

  describe('POST requests', () => {
    it('config.saveLanguage sends body as JSON', async () => {
      mockFetchResponse(null);
      const { config } = await getApi();

      await config.saveLanguage('en');

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/config/language', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '"en"',
      });
    });

    it('config.saveApiKey sends correct payload', async () => {
      mockFetchResponse({ id: '123', name: 'My Key', provider: 'openai', masked_value: 'sk-...456', active: true });
      const { config } = await getApi();

      const result = await config.saveApiKey({
        id: null,
        name: 'My Key',
        provider: 'openai',
        value: 'sk-test-123',
      });

      expect(result).toHaveProperty('id', '123');
      expect(result).toHaveProperty('active', true);
    });
  });

  describe('DELETE requests', () => {
    it('projects.delete calls correct endpoint', async () => {
      mockFetchResponse(null);
      const { projects } = await getApi();

      await projects.delete('proj-42');

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/proj-42', {
        method: 'DELETE',
        headers: {},
        body: undefined,
      });
    });
  });

  describe('error handling', () => {
    it('throws on API error response', async () => {
      mockFetchError('Project not found');
      const { projects } = await getApi();

      await expect(projects.get('nonexistent')).rejects.toThrow('Project not found');
    });

    it('throws on non-JSON response (e.g. 502)', async () => {
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
  });

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
    });

    it('projects has expected methods', async () => {
      const { projects } = await getApi();
      expect(typeof projects.list).toBe('function');
      expect(typeof projects.get).toBe('function');
      expect(typeof projects.scan).toBe('function');
      expect(typeof projects.create).toBe('function');
      expect(typeof projects.bootstrap).toBe('function');
      expect(typeof projects.delete).toBe('function');
      expect(typeof projects.installTemplate).toBe('function');
      expect(typeof projects.auditStream).toBe('function');
      expect(typeof projects.auditInfo).toBe('function');
      expect(typeof projects.validateAudit).toBe('function');
      expect(typeof projects.cancelAudit).toBe('function');
      expect(typeof projects.listAiFiles).toBe('function');
      expect(typeof projects.readAiFile).toBe('function');
      expect(typeof projects.searchAiFiles).toBe('function');
    });

    it('workflows has expected methods', async () => {
      const { workflows } = await getApi();
      expect(typeof workflows.list).toBe('function');
      expect(typeof workflows.get).toBe('function');
      expect(typeof workflows.create).toBe('function');
      expect(typeof workflows.update).toBe('function');
      expect(typeof workflows.delete).toBe('function');
      expect(typeof workflows.trigger).toBe('function');
      expect(typeof workflows.listRuns).toBe('function');
    });

    it('skills has expected methods', async () => {
      const { skills } = await getApi();
      expect(typeof skills.list).toBe('function');
      expect(typeof skills.create).toBe('function');
      expect(typeof skills.update).toBe('function');
      expect(typeof skills.delete).toBe('function');
    });
  });

  describe('audit API calls', () => {
    it('projects.auditInfo calls correct endpoint', async () => {
      mockFetchResponse({ files: [], todos: [] });
      const { projects } = await getApi();

      const result = await projects.auditInfo('proj-1');

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/proj-1/audit-info', {
        method: 'GET',
        headers: {},
        body: undefined,
      });
      expect(result).toEqual({ files: [], todos: [] });
    });

    it('projects.validateAudit calls correct endpoint', async () => {
      mockFetchResponse('Validated');
      const { projects } = await getApi();

      await projects.validateAudit('proj-1');

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/proj-1/validate-audit', {
        method: 'POST',
        headers: {},
        body: undefined,
      });
    });
  });

  describe('skills API calls', () => {
    it('skills.list calls correct endpoint', async () => {
      mockFetchResponse([{ id: 'token-saver', name: 'Token Saver' }]);
      const { skills } = await getApi();

      const result = await skills.list();

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/skills', {
        method: 'GET',
        headers: {},
        body: undefined,
      });
      expect(result).toEqual([{ id: 'token-saver', name: 'Token Saver' }]);
    });

    it('skills.create sends correct payload', async () => {
      mockFetchResponse({ id: 'custom-my-skill', name: 'My Skill' });
      const { skills } = await getApi();

      const result = await skills.create({
        name: 'My Skill',
        icon: 'Star',
        category: 'Language',
        content: 'Do the thing',
      });

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/skills', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: 'My Skill',
          icon: 'Star',
          category: 'Language',
          content: 'Do the thing',
        }),
      });
      expect(result).toHaveProperty('id', 'custom-my-skill');
    });

    it('skills.delete calls correct endpoint', async () => {
      mockFetchResponse(true);
      const { skills } = await getApi();

      await skills.delete('custom-my-skill');

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/skills/custom-my-skill', {
        method: 'DELETE',
        headers: {},
        body: undefined,
      });
    });
  });

  describe('AI files API calls', () => {
    it('projects.listAiFiles calls correct endpoint', async () => {
      mockFetchResponse([{ path: 'ai/index.md', name: 'index.md', is_dir: false }]);
      const { projects } = await getApi();
      await projects.listAiFiles('proj-1');
      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/proj-1/ai-files', expect.objectContaining({ method: 'GET' }));
    });

    it('projects.readAiFile calls correct endpoint with encoded path', async () => {
      mockFetchResponse({ path: 'ai/index.md', content: '# Index' });
      const { projects } = await getApi();
      await projects.readAiFile('proj-1', 'ai/index.md');
      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/proj-1/ai-file?path=ai%2Findex.md', expect.objectContaining({ method: 'GET' }));
    });

    it('projects.searchAiFiles calls correct endpoint', async () => {
      mockFetchResponse([]);
      const { projects } = await getApi();
      await projects.searchAiFiles('proj-1', 'test query');
      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/proj-1/ai-search?q=test%20query', expect.objectContaining({ method: 'GET' }));
    });

    it('projects.cancelAudit calls correct endpoint', async () => {
      mockFetchResponse('NoTemplate');
      const { projects } = await getApi();
      await projects.cancelAudit('proj-1');
      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/proj-1/cancel-audit', expect.objectContaining({ method: 'POST' }));
    });
  });

  describe('bootstrap API calls', () => {
    it('projects.bootstrap calls correct endpoint with correct payload', async () => {
      mockFetchResponse({ project_id: 'proj-new', discussion_id: 'disc-1' });
      const { projects } = await getApi();

      const result = await projects.bootstrap({
        name: 'My New App',
        description: 'A cool app',
        agent: 'ClaudeCode',
      });

      expect(globalThis.fetch).toHaveBeenCalledWith('/api/projects/bootstrap', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: 'My New App',
          description: 'A cool app',
          agent: 'ClaudeCode',
        }),
      });
      expect(result).toEqual({ project_id: 'proj-new', discussion_id: 'disc-1' });
    });
  });
});
