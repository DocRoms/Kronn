/** KT-1692 — the connection card's token check line and its Check action. */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import type { McpConfigDisplay } from '../../../types/generated';
import { TokenCheckStatus, tokenCheckSummary } from '../TokenCheckStatus';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key} ${args.join(' ')}` : key;

type Shape = Pick<McpConfigDisplay, 'token_probe' | 'token_check'>;

const withProbe = (token_check?: Shape['token_check']): Shape => ({ token_probe: true, token_check });
const withoutProbe = (token_check?: Shape['token_check']): Shape => ({ token_probe: false, token_check });

const checked = (outcome: 'ok' | 'refused' | 'inconclusive', detail = '') => ({
  checked_at: '2026-11-11T08:00:00Z',
  outcome,
  detail,
  last_good_at: '2026-11-10T08:00:00Z',
});

describe('tokenCheckSummary', () => {
  it('reads the last probe of a connection that has one', () => {
    expect(tokenCheckSummary(withProbe(checked('ok')), t).state).toBe('ok');
    expect(tokenCheckSummary(withProbe(checked('refused')), t).state).toBe('refused');
    expect(tokenCheckSummary(withProbe(checked('inconclusive')), t).state).toBe('inconclusive');
  });

  it('says a probed connection never checked has never been checked', () => {
    expect(tokenCheckSummary(withProbe(undefined), t)).toEqual({ state: 'never', text: 'mcp.tokenCheck.never' });
  });

  it('shows the date of the last good call of a connection without a probe', () => {
    const summary = tokenCheckSummary(
      withoutProbe({ detail: '', last_good_at: '2026-11-10T08:00:00Z' }),
      t,
    );
    expect(summary.state).toBe('not_checked');
    expect(summary.text).toContain('mcp.tokenCheck.notChecked');
    expect(summary.text).toContain(new Date('2026-11-10T08:00:00Z').toLocaleString());
  });

  it('says a connection without a probe and without a good call never checked', () => {
    expect(tokenCheckSummary(withoutProbe(undefined), t).state).toBe('never');
    expect(tokenCheckSummary(withoutProbe({ detail: '' }), t).state).toBe('never');
  });
});

describe('TokenCheckStatus', () => {
  const cfg = (shape: Shape) => shape as McpConfigDisplay;

  it('offers Check on a probed connection and runs it', () => {
    const onCheck = vi.fn();
    render(<TokenCheckStatus cfg={cfg(withProbe(checked('refused', 'Token refused (HTTP 401): Bad credentials')))} t={t} checking={false} onCheck={onCheck} />);

    expect(screen.getByTestId('mcp-token-check').getAttribute('data-state')).toBe('refused');
    expect(screen.getByTestId('mcp-token-check').textContent).toContain('Bad credentials');
    fireEvent.click(screen.getByTestId('mcp-token-check-button'));
    expect(onCheck).toHaveBeenCalledTimes(1);
  });

  it('disables Check while a check runs', () => {
    render(<TokenCheckStatus cfg={cfg(withProbe(undefined))} t={t} checking onCheck={() => {}} />);
    expect((screen.getByTestId('mcp-token-check-button') as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows no Check button for a connection without a probe, only its last good call', () => {
    render(<TokenCheckStatus cfg={cfg(withoutProbe({ detail: '', last_good_at: '2026-11-10T08:00:00Z' }))} t={t} checking={false} onCheck={() => {}} />);

    expect(screen.queryByTestId('mcp-token-check-button')).toBeNull();
    expect(screen.getByTestId('mcp-token-check-noprobe')).toBeTruthy();
    expect(screen.getByTestId('mcp-token-check-summary').textContent).toContain('mcp.tokenCheck.notChecked');
  });
});
