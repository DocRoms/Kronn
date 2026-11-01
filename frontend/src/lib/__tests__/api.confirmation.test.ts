import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// KT-1495 — a vault route answers 403 `confirm_required` once a confirmation is
// enrolled; the human types a code and the request is sent again with it.

function reply(status: number, body: unknown) {
  const make = (): unknown => ({
    status,
    headers: { get: (name: string) => (name === 'content-type' ? 'application/json' : null) },
    json: () => Promise.resolve(body),
    clone: make,
  });
  return make();
}

const REFUSED = { success: false, data: null, error: 'needs a code', error_code: 'confirm_required' };

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('strong confirmation of the vault routes', () => {
  it('asks for a code, then sends the request again with the X-Kronn-Confirm header', async () => {
    const { config, setApiBase, setConfirmationPrompt } = await import('../api');
    setApiBase('');
    const prompt = vi.fn(() => '123456');
    setConfirmationPrompt(prompt);
    vi.mocked(fetch)
      .mockResolvedValueOnce(reply(403, REFUSED) as Response)
      .mockResolvedValueOnce(reply(200, { success: true, data: 'new-token', error: null }) as Response);

    await expect(config.regenerateAuthToken()).resolves.toBe('new-token');

    expect(prompt).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledTimes(2);
    const first = vi.mocked(fetch).mock.calls[0][1]?.headers as Record<string, string>;
    const second = vi.mocked(fetch).mock.calls[1][1]?.headers as Record<string, string>;
    expect(first['X-Kronn-Confirm']).toBeUndefined();
    expect(second['X-Kronn-Confirm']).toBe('123456');
  });

  it('gives up with the refusal when the human declines the code', async () => {
    const { config, setApiBase, setConfirmationPrompt } = await import('../api');
    setApiBase('');
    setConfirmationPrompt(() => null);
    vi.mocked(fetch).mockResolvedValue(reply(403, REFUSED) as Response);

    await expect(config.regenerateAuthToken()).rejects.toMatchObject({
      message: 'needs a code',
      code: 'confirm_required',
    });
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('never asks for a code on another refusal', async () => {
    const { config, setApiBase, setConfirmationPrompt } = await import('../api');
    setApiBase('');
    const prompt = vi.fn(() => '123456');
    setConfirmationPrompt(prompt);
    vi.mocked(fetch).mockResolvedValue(
      reply(403, { success: false, data: null, error: 'forbidden', error_code: 'validation' }) as Response,
    );

    await expect(config.regenerateAuthToken()).rejects.toMatchObject({ code: 'validation' });
    expect(prompt).not.toHaveBeenCalled();
  });

  it('asks once: a wrong code is the final answer', async () => {
    const { config, setApiBase, setConfirmationPrompt } = await import('../api');
    setApiBase('');
    const prompt = vi.fn(() => '000000');
    setConfirmationPrompt(prompt);
    vi.mocked(fetch).mockResolvedValue(reply(403, REFUSED) as Response);

    await expect(config.regenerateAuthToken()).rejects.toMatchObject({ code: 'confirm_required' });
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledTimes(2);
  });
});
