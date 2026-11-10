/** KT-1524 — the connection card's "token refused" notice. */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import type { McpAuthFailure } from '../../../types/generated';
import { TokenRefusedNotice } from '../TokenRefusedNotice';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key} ${args.join(' ')}` : key;

const failure = (over: Partial<McpAuthFailure> = {}): McpAuthFailure => ({
  kind: 'refused',
  http_status: 401,
  message: 'Bad credentials',
  failed_at: '2026-11-10T08:00:00Z',
  ...over,
});

describe('TokenRefusedNotice', () => {
  it('shows the badge, the provider message and the update action', () => {
    const onUpdate = vi.fn();
    render(<TokenRefusedNotice failure={failure()} t={t} onUpdate={onUpdate} />);

    const notice = screen.getByTestId('mcp-token-refused');
    expect(notice.textContent).toContain('mcp.authFailure.title.refused (HTTP 401)');
    expect(notice.textContent).toContain('mcp.authFailure.provider Bad credentials');
    fireEvent.click(screen.getByTestId('mcp-token-refused-update'));
    expect(onUpdate).toHaveBeenCalledTimes(1);
  });

  it('names a failed refresh without an HTTP status', () => {
    render(<TokenRefusedNotice failure={failure({ kind: 'refresh_failed', http_status: undefined })} t={t} onUpdate={() => {}} />);
    const notice = screen.getByTestId('mcp-token-refused');
    expect(notice.textContent).toContain('mcp.authFailure.title.refresh_failed');
    expect(notice.textContent).not.toContain('HTTP');
  });
});
