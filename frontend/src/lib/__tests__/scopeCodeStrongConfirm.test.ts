// KT-1491 — granting `code` on another project asks for the strong confirmation
// (the server answers 403 `strong_confirmation_required`, KT-1496): the owner
// types the code in the dialog, it buys a single-use grant bound to that one
// call, and the very same request is sent again with it.
import { describe, it, expect, vi, afterEach } from 'vitest';
import { waitFor } from '@testing-library/react';
import { discussions } from '../api';
import { currentStrongPrompt } from '../strongConfirm';

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

const grantBody = {
  id: 'g1', discussion_id: 'd1', project_id: 'p2', project_name: 'infra', access: 'code', source: 'link',
  request_id: null, granted_by: 'romu', granted_at: '2026-11-04T10:00:00Z',
  revoked_by: null, revoked_at: null, revoked_reason: null,
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('granting code asks for the strong confirmation', () => {
  it('asks for the code, buys a grant bound to the call and repeats the request with it', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json({
        success: false, error: 'this action needs the strong confirmation', error_code: 'strong_confirmation_required',
      }, 403))
      .mockResolvedValueOnce(json({ success: true, data: { grant: 'kronn-grant-1', expires_in_secs: 120 } }))
      .mockResolvedValueOnce(json({ success: true, data: grantBody }));
    vi.stubGlobal('fetch', fetchMock);

    const pending = discussions.grantScope('d1', {
      project_id: 'p2', all: false, general: false, access: 'code', confirm: null, expected_fingerprint: null,
    });

    await waitFor(() => expect(currentStrongPrompt()).not.toBeNull());
    const prompt = currentStrongPrompt()!;
    expect(prompt.method).toBe('POST');
    expect(prompt.path).toBe('/api/discussions/d1/scope/grants');
    prompt.answer('123456');

    const granted = await pending;
    expect(granted.access).toBe('code');
    expect(fetchMock).toHaveBeenCalledTimes(3);
    // The code buys a grant for this very call...
    const mint = fetchMock.mock.calls[1];
    expect(String(mint[0])).toContain('/api/trust/grants');
    expect(JSON.parse(mint[1].body as string)).toEqual({
      method: 'POST', path: '/api/discussions/d1/scope/grants', code: '123456',
    });
    // ... and the request is repeated with the grant, never with the code.
    const retry = fetchMock.mock.calls[2];
    expect(String(retry[0])).toContain('/api/discussions/d1/scope/grants');
    expect((retry[1].headers as Record<string, string>)['X-Kronn-Grant']).toBe('kronn-grant-1');
    expect((retry[1].headers as Record<string, string>)['X-Kronn-Confirm']).toBeUndefined();
  });

  it('asks for nothing when the server does not (planning, or nothing enrolled)', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(json({ success: true, data: { ...grantBody, access: 'plan' } }));
    vi.stubGlobal('fetch', fetchMock);
    await discussions.grantScope('d1', {
      project_id: 'p2', all: false, general: false, access: 'plan', confirm: null, expected_fingerprint: null,
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(currentStrongPrompt()).toBeNull();
  });
});
