import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, cleanup } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  discussions: {
    participants: vi.fn(),
    invitePeer: vi.fn(),
    meta: vi.fn(),
  },
}));

import { DiscParticipantsHeader } from '../DiscParticipantsHeader';
import { freshnessOf, DEFAULT_AWAY_AFTER_MS } from '../../lib/discPresence';
import { discussions as discussionsApi } from '../../lib/api';

const toast = vi.fn();
const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}(${args.join(',')})` : key;

beforeEach(() => {
  vi.clearAllMocks();
  (discussionsApi.meta as ReturnType<typeof vi.fn>).mockResolvedValue({
    poll_policy: { max_delay_seconds: 480 },
  });
});

afterEach(() => {
  cleanup();
});

describe('DiscParticipantsHeader — 0.8.6 phase 2', () => {
  it('shows the empty-state copy when the disc has no participants', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-empty" toast={toast} t={t} />);
      await Promise.resolve();
    });
    expect(document.body.textContent).toContain('disc.participantsEmpty');
  });

  it('renders one chip per active participant', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', session_id: 'sess-A', role: 'owner', status: 'active' },
      { id: 2, agent_type: 'Codex', session_id: 'sess-B', role: 'peer', status: 'active' },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-2" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const chips = document.querySelectorAll('.disc-participant-chip');
    expect(chips.length).toBe(2);
    expect(chips[0].textContent).toContain('ClaudeCode');
    expect(chips[1].textContent).toContain('Codex');
  });

  it('renders paused participants with the paused style attribute', async () => {
    // Visual differentiation : the chip has `data-status="paused"`,
    // CSS turns it grey. The test checks the attribute rather than
    // the computed style (CSS isn't loaded in jsdom).
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', session_id: 'sA', role: 'owner', status: 'paused' },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-3" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const chip = document.querySelector('.disc-participant-chip') as HTMLElement;
    expect(chip).not.toBeNull();
    expect(chip.getAttribute('data-status')).toBe('paused');
  });

  it('opens the invite modal with the instruction when `+ Inviter` is clicked', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockResolvedValue({
      token: 'kr-join-abc',
      disc_id: 'd-4',
      expires_at: '2026-05-21T10:00:00Z',
      ttl_seconds: 600,
      instruction_text: 'Join Kronn discussion: disc_join({token: "kr-join-abc"})',
    });
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-4" toast={toast} t={t} />);
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

  it('toasts an error when the invite-peer call fails', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('boom')
    );
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-err" toast={toast} t={t} />);
      await Promise.resolve();
    });
    const inviteBtn = document.querySelector('.disc-participants-invite-btn') as HTMLButtonElement;
    await act(async () => {
      fireEvent.click(inviteBtn);
      await Promise.resolve();
    });
    // toast(_key, 'error') called with the failure key.
    const errToast = toast.mock.calls.find(c => c[1] === 'error');
    expect(errToast).toBeDefined();
    expect(errToast![0]).toContain('disc.inviteFailed');
  });

  it('refetches participants when discId changes', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    const { rerender } = render(<DiscParticipantsHeader discId="d-1" toast={toast} t={t} />);
    await act(async () => { await Promise.resolve(); });
    expect(discussionsApi.participants).toHaveBeenCalledWith('d-1');

    rerender(<DiscParticipantsHeader discId="d-2" toast={toast} t={t} />);
    await act(async () => { await Promise.resolve(); });
    expect(discussionsApi.participants).toHaveBeenCalledWith('d-2');
    expect((discussionsApi.participants as ReturnType<typeof vi.fn>).mock.calls.length).toBe(2);
  });
});

describe('activity placeholder — presence phase 1 (0.8.12 PR B)', () => {
  it('renders the i18n label for a live activity and nothing when absent', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', session_id: 'sA', role: 'owner', status: 'active', activity: 'listening' },
      { id: 2, agent_type: 'Codex', session_id: 'sB', role: 'peer', status: 'active', activity: 'reading' },
      { id: 3, agent_type: 'GeminiCli', session_id: 'sC', role: 'peer', status: 'active', activity: null },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-act" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const labels = Array.from(document.querySelectorAll('.disc-participant-activity'));
    expect(labels.length, 'only live activities render').toBe(2);
    expect(labels[0].textContent).toBe('disc.activityListening');
    expect(labels[1].textContent).toBe('disc.activityReading');
  });

  it('never renders a raw token for an unknown future activity value', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', session_id: 'sA', role: 'owner', status: 'active', activity: 'compiling' },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-unk" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(document.querySelector('.disc-participant-activity')).toBeNull();
    expect(document.body.textContent).not.toContain('compiling');
  });
});

describe('freshnessOf — presence thresholds aligned with PollBackoffPolicy (stab-3)', () => {
  // Timestamps are built relative to Date.now() so the assertions pin the
  // BOUNDARIES (2 min fresh/idle, awayAfterMs idle/away), not wall-clock.
  const ago = (ms: number) => new Date(Date.now() - ms).toISOString();

  it('treats a missing or unparseable last_seen as away', () => {
    expect(freshnessOf(null, DEFAULT_AWAY_AFTER_MS)).toBe('away');
    expect(freshnessOf(undefined, DEFAULT_AWAY_AFTER_MS)).toBe('away');
    expect(freshnessOf('not-a-date', DEFAULT_AWAY_AFTER_MS)).toBe('away');
  });

  it('is fresh under 2 min, idle beyond', () => {
    expect(freshnessOf(ago(60_000), DEFAULT_AWAY_AFTER_MS)).toBe('fresh');
    expect(freshnessOf(ago(2 * 60_000 + 1_000), DEFAULT_AWAY_AFTER_MS)).toBe('idle');
  });

  it('stays idle up to the away threshold, away beyond it', () => {
    // A cold-regime agent sleeping the full max_delay is "en veille" (idle),
    // not absent — grey only once it missed its own pacing contract.
    expect(freshnessOf(ago(DEFAULT_AWAY_AFTER_MS - 5_000), DEFAULT_AWAY_AFTER_MS)).toBe('idle');
    expect(freshnessOf(ago(DEFAULT_AWAY_AFTER_MS + 1_000), DEFAULT_AWAY_AFTER_MS)).toBe('away');
  });

  it('follows a server-provided threshold instead of the fallback constant', () => {
    const threeMin = 3 * 60_000;
    expect(freshnessOf(ago(2.5 * 60_000), threeMin)).toBe('idle');
    expect(freshnessOf(ago(4 * 60_000), threeMin)).toBe('away');
  });
});
