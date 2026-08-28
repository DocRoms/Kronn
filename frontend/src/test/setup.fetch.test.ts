import { afterEach, describe, expect, it, vi } from 'vitest';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('Vitest network guard', () => {
  it('refuses an unmocked localhost request with an actionable error', () => {
    expect(() => fetch('http://localhost:3000/api/config/server')).toThrow(
      '[test-network-guard] Unexpected unmocked fetch to http://localhost:3000/api/config/server',
    );
  });

  it('lets fetch-specific tests install an explicit local stub', async () => {
    const response = new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
    const fetchMock = vi.fn().mockResolvedValue(response);
    vi.stubGlobal('fetch', fetchMock);

    await expect(fetch('/api/example')).resolves.toBe(response);
    expect(fetchMock).toHaveBeenCalledWith('/api/example');
  });
});
