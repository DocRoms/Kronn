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
      expect(api.stats).toBeDefined();
    });

    it('projects has expected methods', async () => {
      const { projects } = await getApi();
      expect(typeof projects.list).toBe('function');
      expect(typeof projects.get).toBe('function');
      expect(typeof projects.scan).toBe('function');
      expect(typeof projects.create).toBe('function');
      expect(typeof projects.delete).toBe('function');
      expect(typeof projects.installTemplate).toBe('function');
      expect(typeof projects.auditStream).toBe('function');
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
  });
});
