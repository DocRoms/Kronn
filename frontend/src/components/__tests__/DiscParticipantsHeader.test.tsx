import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, cleanup } from '@testing-library/react';
import { readFileSync } from 'node:fs';

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

async function openParticipant(index = 0) {
  await act(async () => {
    fireEvent.click(document.querySelectorAll('.disc-participant-chip')[index] as HTMLButtonElement);
  });
}

const participantStatus = () =>
  document.querySelector('.disc-participant-details dd')?.textContent;

describe('DiscParticipantsHeader — 0.8.6 phase 2', () => {
  it('reserves separate header rows for the title and compact participant strip', () => {
    const headerCss = readFileSync('src/pages/DiscussionsPage.css', 'utf8');
    const participantCss = readFileSync('src/styles/components.css', 'utf8');
    const headerTop = headerCss.match(/\.disc-chat-header-top\s*\{([^}]*)\}/)?.[1] ?? '';
    const presence = headerCss.match(/\.disc-chat-header-presence\s*\{([^}]*)\}/)?.[1] ?? '';
    const participantList = participantCss.match(/\.disc-participants-list\s*\{([^}]*)\}/)?.[1] ?? '';

    expect(headerTop).toContain('display: grid');
    expect(headerTop).toContain('grid-template-columns: minmax(0, 1fr)');
    expect(presence).toContain('width: 100%');
    expect(participantList).toContain('flex-wrap: nowrap');
    expect(participantList).toContain('overflow-x: auto');
  });

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
    expect(chips[0].querySelector('.disc-participant-name')?.textContent).toBe('@claude');
    expect(chips[1].querySelector('.disc-participant-name')?.textContent).toBe('@codex');
    expect(document.querySelector('[data-testid="disc-participant-details"]')).toBeNull();
    await openParticipant();
    expect(document.querySelector('[data-testid="disc-participant-details"]')).not.toBeNull();
    expect(chips[0].getAttribute('aria-expanded')).toBe('true');
    fireEvent.click(document.querySelector('.disc-participant-details header button') as HTMLButtonElement);
    expect(document.querySelector('[data-testid="disc-participant-details"]')).toBeNull();
  });

  it('shows only an explicitly declared JOIN model and labels it as join metadata', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        id: 1, agent_type: 'ClaudeCode', model: 'claude-sonnet-4-5',
        session_id: 'sess-A', role: 'peer', status: 'active',
      },
      {
        id: 2, agent_type: 'Codex', model: null,
        session_id: 'sess-B', role: 'peer', status: 'active',
      },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-models" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(document.querySelectorAll('.disc-participant-model')).toHaveLength(0);
    await openParticipant();
    const models = document.querySelectorAll('.disc-participant-model');
    expect(models).toHaveLength(1);
    expect(models[0].textContent).toContain('claude-sonnet-4-5');
    expect(document.body.textContent).not.toContain('undefined');
  });

  it('copies the exact native resume command only when the CLI exposes an id', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        id: 1, agent_type: 'ClaudeCode',
        conversation_id: '11111111-1111-4111-8111-111111111111',
        session_id: 'bridge-A', role: 'peer', status: 'active',
      },
      {
        id: 2, agent_type: 'Codex',
        conversation_id: '22222222-2222-4222-8222-222222222222',
        session_id: 'bridge-B', role: 'peer', status: 'active',
      },
      {
        id: 3, agent_type: 'GeminiCli',
        conversation_id: null,
        session_id: 'bridge-C', role: 'peer', status: 'active',
      },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-resume" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    await openParticipant();
    await act(async () => {
      fireEvent.click(document.querySelector('button.disc-participant-resume') as HTMLButtonElement);
      fireEvent.click(document.querySelectorAll('.disc-participant-chip')[1] as HTMLButtonElement);
    });
    await act(async () => {
      fireEvent.click(document.querySelector('button.disc-participant-resume') as HTMLButtonElement);
      await Promise.resolve();
    });
    expect(writeText).toHaveBeenNthCalledWith(
      1,
      'claude --resume 11111111-1111-4111-8111-111111111111',
    );
    expect(writeText).toHaveBeenNthCalledWith(
      2,
      'codex resume 22222222-2222-4222-8222-222222222222',
    );
  });

  it('does not invent a resume button for unsupported agents or a missing id', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', conversation_id: null, session_id: 'a', role: 'peer', status: 'active' },
      { id: 2, agent_type: 'GeminiCli', conversation_id: '33333333-3333-4333-8333-333333333333', session_id: 'b', role: 'peer', status: 'active' },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-no-resume" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    for (const chip of document.querySelectorAll('.disc-participant-chip')) {
      await act(async () => { fireEvent.click(chip); });
      expect(document.querySelector('.disc-participant-resume')).toBeNull();
    }
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
      instruction_text: 'Join Kronn discussion: disc_join({token: "kr-join-abc"})\nlis le plan avec plan_get',
      instruction_text_minimal: 'Join Kronn discussion: disc_join({token: "kr-join-abc"})',
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

  /// KT-52 — the enriched handoff must be what the human copies by default;
  /// dropping to the bare call is a deliberate, reversible choice.
  it('offers the enriched handoff by default and can fall back to the bare call', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    (discussionsApi.invitePeer as ReturnType<typeof vi.fn>).mockResolvedValue({
      token: 'kr-join-xyz',
      disc_id: 'd-5',
      expires_at: '2026-05-21T10:00:00Z',
      ttl_seconds: 600,
      instruction_text: 'disc_join({token: "kr-join-xyz"})\nlis le plan avec plan_get\nreste en écoute',
      instruction_text_minimal: 'disc_join({token: "kr-join-xyz"})',
    });
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-5" toast={toast} t={t} />);
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

describe('honest participant presence — 0.9.2 G', () => {
  it('keeps a conservative legacy fallback when the backend has no presence_state', async () => {
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
    for (const [index, expected] of [
      'disc.presenceListening',
      'disc.activityReading',
      'disc.presenceOffline',
    ].entries()) {
      await openParticipant(index);
      expect(participantStatus(), 'every participant exposes an honest state').toBe(expected);
    }
  });

  it('a waiting legacy participant renders dormant, not falsely listening', async () => {
    const staleHeartbeat = new Date(Date.now() - 20 * 60_000).toISOString();
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', session_id: 'sA', role: 'owner', status: 'active', activity: 'waiting', last_seen: staleHeartbeat },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-wait" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const chip = document.querySelector('.disc-participant-chip');
    expect(chip?.getAttribute('data-presence')).toBe('dormant');
    expect(chip?.getAttribute('data-freshness')).toBe('idle');
    await openParticipant();
    expect(participantStatus()).toBe('disc.presenceDormant');
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
    await openParticipant();
    expect(participantStatus()).toBe('disc.presenceOffline');
    expect(document.body.textContent).not.toContain('compiling');
  });

  it('renders server-derived listening, dormant and offline states plus write failure', async () => {
    const nextPollAt = new Date(Date.now() + 30_000).toISOString();
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        id: 1, agent_type: 'ClaudeCode', session_id: 'sA', role: 'owner', status: 'active',
        presence_state: 'listening', read_live: true, write_state: 'ok',
        wake_mode: 'external_poll', next_poll_at: null,
      },
      {
        id: 2, agent_type: 'Codex', session_id: 'sB', role: 'peer', status: 'active',
        presence_state: 'dormant', read_live: false, write_state: 'unknown',
        wake_mode: 'external_poll', next_poll_at: nextPollAt,
      },
      {
        id: 3, agent_type: 'GeminiCli', session_id: 'sC', role: 'peer', status: 'active',
        presence_state: 'offline', read_live: false, write_state: 'failed',
        wake_mode: 'external_poll', next_poll_at: null,
      },
      {
        id: 4, agent_type: 'Ollama', session_id: 'sD', role: 'peer', status: 'active',
        presence_state: 'running', read_live: false, write_state: 'unknown',
        wake_mode: 'native_dispatch', next_poll_at: null,
      },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-honest" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const chips = Array.from(document.querySelectorAll('.disc-participant-chip'));
    expect(chips.map(chip => chip.getAttribute('data-presence')))
      .toEqual(['listening', 'dormant', 'offline', 'running']);
    expect(chips[0].getAttribute('data-read-live')).toBe('true');
    await openParticipant(1);
    expect(participantStatus()).toContain('disc.presenceDormantSeconds');
    await openParticipant(2);
    expect(participantStatus()).toBe('disc.presenceOffline');
    expect(document.querySelector('.disc-participant-write-failed')).not.toBeNull();
    expect(chips[3].querySelector('.disc-participant-running')).not.toBeNull();
  });

  it('projects paused participants to the explicit paused/offline state', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        id: 1, agent_type: 'Codex', session_id: 'sA', role: 'peer', status: 'paused',
        presence_state: 'listening', read_live: true, write_state: 'ok',
      },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-paused" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const chip = document.querySelector('.disc-participant-chip');
    expect(chip?.getAttribute('data-presence')).toBe('offline');
    await openParticipant();
    expect(participantStatus()).toBe('disc.presencePaused');
  });

  it('labels a deferred native obligation as waiting for a runtime', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        id: -1, agent_type: 'Vibe', session_id: null, role: 'peer', status: 'active',
        presence_state: 'dormant', read_live: false, write_state: 'unknown',
        wake_mode: 'native_dispatch',
        next_poll_at: new Date(Date.now() + 30_000).toISOString(),
      },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-obligation" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const chip = document.querySelector('.disc-participant-chip');
    expect(chip?.getAttribute('data-wake-mode')).toBe('native_dispatch');
    await openParticipant();
    expect(participantStatus()).toBe('disc.presenceAwaitingRuntime');
    expect(document.querySelector('.disc-participant-details')?.textContent)
      .not.toContain('disc.presenceDormantSeconds');
  });

  it('pins the invite button OUTSIDE the scrollable chip strip (3 offline peers)', async () => {
    // Regression: with 3+ offline "reconnexion requise" peers the chip strip
    // widens; the invite button must stay a pinned sibling of the scrollable
    // list, never a wrapped/scrolled-away last item that gets clipped.
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', session_id: 'sA', role: 'owner', status: 'active', presence_state: 'offline' },
      { id: 2, agent_type: 'Codex', session_id: 'sB', role: 'peer', status: 'active', presence_state: 'offline' },
      { id: 3, agent_type: 'GeminiCli', session_id: 'sC', role: 'peer', status: 'active', presence_state: 'offline' },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-3offline" toast={toast} t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const list = document.querySelector('.disc-participants-list');
    const inviteBtn = document.querySelector('.disc-participants-invite-btn');
    // All three chips live INSIDE the scrollable strip.
    expect(list?.querySelectorAll('.disc-participant-chip')).toHaveLength(3);
    // The invite button is a sibling of the strip, NOT nested inside it.
    expect(inviteBtn).not.toBeNull();
    expect(list?.contains(inviteBtn as Node)).toBe(false);
    expect(inviteBtn?.parentElement?.classList.contains('disc-participants-row')).toBe(true);
    // Every chip renders the honest "offline" state.
    document.querySelectorAll('.disc-participant-chip').forEach(chip => {
      expect(chip.getAttribute('data-presence')).toBe('offline');
    });
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
