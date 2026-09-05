import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, cleanup } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  discussions: {
    invitePeer: vi.fn(),
  },
}));

import { DiscInviteButton } from '../DiscInviteButton';
import { discussions as discussionsApi } from '../../lib/api';

const toast = vi.fn();
const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}(${args.join(',')})` : key;

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
});

describe('DiscInviteButton', () => {
  it('opens the invite modal with the instruction when `+ Inviter` is clicked', async () => {
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockResolvedValue({
      token: 'kr-join-abc',
      disc_id: 'd-4',
      expires_at: '2026-05-21T10:00:00Z',
      ttl_seconds: 600,
      instruction_text: 'Join Kronn discussion: disc_join({token: "kr-join-abc"})\nlis le plan avec plan_get',
      instruction_text_minimal: 'Join Kronn discussion: disc_join({token: "kr-join-abc"})',
    });
    await act(async () => {
      render(<DiscInviteButton discId="d-4" toast={toast} t={t} />);
      await Promise.resolve();
    });

    const inviteBtn = document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement;
    expect(inviteBtn).not.toBeNull();
    await act(async () => {
      fireEvent.click(inviteBtn);
      await Promise.resolve();
      await Promise.resolve();
    });

    // Modal renders with the instruction text inside a <pre>.
    const pre = document.querySelector('.disc-invite-instruction');
    expect(pre).not.toBeNull();
    expect(pre!.textContent).toContain('kr-join-abc');
    expect(pre!.textContent).toContain('disc_join');
    expect(discussionsApi.invitePeer).toHaveBeenCalledWith('d-4');
  });

  /// KT-52 — the enriched handoff must be what the human copies by default;
  /// dropping to the bare call is a deliberate, reversible choice.
  it('offers the enriched handoff by default and can fall back to the bare call', async () => {
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockResolvedValue({
      token: 'kr-join-xyz',
      disc_id: 'd-5',
      expires_at: '2026-05-21T10:00:00Z',
      ttl_seconds: 600,
      instruction_text: 'disc_join({token: "kr-join-xyz"})\nlis le plan avec plan_get\nreste en écoute',
      instruction_text_minimal: 'disc_join({token: "kr-join-xyz"})',
    });
    await act(async () => {
      render(<DiscInviteButton discId="d-5" toast={toast} t={t} />);
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      await Promise.resolve();
      await Promise.resolve();
    });

    const shown = () => document.querySelector('[data-testid="disc-invite-instruction"]')!.textContent!;
    const toggle = document.querySelector('[data-testid="disc-invite-handoff-toggle"]') as HTMLInputElement;
    expect(toggle.checked).toBe(true);
    expect(shown()).toContain('plan_get');

    await act(async () => { fireEvent.click(toggle); });
    expect(shown()).not.toContain('plan_get');
    expect(shown()).toContain('kr-join-xyz');

    // And back: the choice isn't a one-way door.
    await act(async () => { fireEvent.click(toggle); });
    expect(shown()).toContain('plan_get');
  });

  it('toasts an error when the invite-peer call fails', async () => {
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('boom')
    );
    await act(async () => {
      render(<DiscInviteButton discId="d-err" toast={toast} t={t} />);
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      await Promise.resolve();
    });
    // toast(_key, 'error') called with the failure key.
    const errToast = toast.mock.calls.find(c => c[1] === 'error');
    expect(errToast).toBeDefined();
    expect(errToast![0]).toContain('disc.inviteFailed');
  });

  /// The chip strip lives in another component now, so the only way it learns
  /// about a peer invited here is this callback. Without it the new peer waits
  /// out the poll interval.
  it('tells the participants row to refetch after a successful invite', async () => {
    const onInvited = vi.fn();
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockResolvedValue({
      token: 'kr-join-ok',
      disc_id: 'd-6',
      expires_at: '2026-05-21T10:00:00Z',
      ttl_seconds: 600,
      instruction_text: 'disc_join({token: "kr-join-ok"})',
      instruction_text_minimal: 'disc_join({token: "kr-join-ok"})',
    });
    await act(async () => {
      render(<DiscInviteButton discId="d-6" toast={toast} t={t} onInvited={onInvited} />);
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onInvited).toHaveBeenCalledTimes(1);
  });

  it('does not call back when the invite fails', async () => {
    const onInvited = vi.fn();
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('nope'));
    await act(async () => {
      render(<DiscInviteButton discId="d-7" toast={toast} t={t} onInvited={onInvited} />);
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.click(document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement);
      await Promise.resolve();
    });
    expect(onInvited).not.toHaveBeenCalled();
  });
});
