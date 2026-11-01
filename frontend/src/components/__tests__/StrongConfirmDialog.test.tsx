/**
 * KT-1496 — a route that returns a secret or destroys data asks for the strong
 * confirmation inline: the server answers 403 `strong_confirmation_required`,
 * the dialog explains why and takes the code, the code buys a single-use grant
 * bound to that call, and the same request is repeated with the grant.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { I18nProvider } from '../../lib/I18nContext';
import { ApiRequestError } from '../../lib/apiRequestError';
import { setup } from '../../lib/api';
import { currentStrongPrompt } from '../../lib/strongConfirm';
import { StrongConfirmDialog } from '../StrongConfirmDialog';

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

const REQUIRED = {
  success: false,
  data: null,
  error: 'this action needs the strong confirmation',
  error_code: 'strong_confirmation_required',
};

const fetchMock = vi.fn<(url: string, init?: RequestInit) => Promise<Response>>();

const callsTo = (path: string) =>
  fetchMock.mock.calls.filter(([url]) => String(url).endsWith(path));

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
  render(
    <I18nProvider>
      <StrongConfirmDialog />
    </I18nProvider>,
  );
});

afterEach(() => {
  // A prompt left open by a failed test must not leak into the next one.
  currentStrongPrompt()?.answer(null);
  cleanup();
  vi.unstubAllGlobals();
});

describe('the inline strong confirmation', () => {
  it('asks for the code, explains why, then repeats the call with a grant', async () => {
    fetchMock
      .mockResolvedValueOnce(json(REQUIRED, 403))
      .mockResolvedValueOnce(json({ success: true, data: { grant: 'kgr_one', expires_in_secs: 120 }, error: null }))
      .mockResolvedValueOnce(json({ success: true, data: null, error: null }));

    const done = setup.reset();

    const dialog = await screen.findByTestId('strong-confirm-dialog');
    expect(dialog.textContent).toContain('POST /api/setup/reset');

    fireEvent.change(screen.getByTestId('strong-confirm-input'), { target: { value: ' 123456 ' } });
    fireEvent.click(screen.getByTestId('strong-confirm-submit'));
    await done;

    // The code bought a grant bound to this very call...
    const [mint] = callsTo('/api/trust/grants');
    expect(JSON.parse(String(mint[1]?.body))).toEqual({
      method: 'POST',
      path: '/api/setup/reset',
      code: '123456',
    });
    // ...and the repeated request carries the grant, never the code.
    const resets = callsTo('/api/setup/reset');
    expect(resets).toHaveLength(2);
    const headers = resets[1][1]?.headers as Record<string, string>;
    expect(headers['X-Kronn-Grant']).toBe('kgr_one');
    expect(headers['X-Kronn-Confirm']).toBeUndefined();
    expect(screen.queryByTestId('strong-confirm-dialog')).toBeNull();
  });

  it('keeps the dialog open with the reason when the code is refused', async () => {
    fetchMock
      .mockResolvedValueOnce(json(REQUIRED, 403))
      .mockResolvedValueOnce(json({ ...REQUIRED, error: 'the code is missing, wrong or already used' }, 403))
      .mockResolvedValueOnce(json({ success: true, data: { grant: 'kgr_two', expires_in_secs: 120 }, error: null }))
      .mockResolvedValueOnce(json({ success: true, data: null, error: null }));

    const done = setup.reset();

    fireEvent.change(await screen.findByTestId('strong-confirm-input'), { target: { value: '000000' } });
    fireEvent.click(screen.getByTestId('strong-confirm-submit'));
    const refused = await screen.findByTestId('strong-confirm-refused');
    expect(refused.textContent).toContain('wrong or already used');

    fireEvent.change(screen.getByTestId('strong-confirm-input'), { target: { value: '654321' } });
    fireEvent.click(screen.getByTestId('strong-confirm-submit'));
    await done;
    expect(callsTo('/api/setup/reset')).toHaveLength(2);
  });

  it('gives the refusal back to the caller when the owner declines', async () => {
    fetchMock.mockResolvedValueOnce(json(REQUIRED, 403));

    const done = setup.reset();
    const outcome = done.then(() => null, (error: unknown) => error);

    fireEvent.click(await screen.findByTestId('strong-confirm-cancel'));
    const error = await outcome;
    expect(error).toBeInstanceOf(ApiRequestError);
    expect((error as ApiRequestError).code).toBe('strong_confirmation_required');
    expect(callsTo('/api/trust/grants')).toHaveLength(0);
    await waitFor(() => expect(screen.queryByTestId('strong-confirm-dialog')).toBeNull());
  });

  it('does not ask for a call the server accepts as it is', async () => {
    fetchMock.mockResolvedValueOnce(json({ success: true, data: null, error: null }));
    await setup.reset();
    expect(screen.queryByTestId('strong-confirm-dialog')).toBeNull();
    expect(callsTo('/api/setup/reset')).toHaveLength(1);
    expect(callsTo('/api/trust/grants')).toHaveLength(0);
  });
});
