import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, cleanup } from '@testing-library/react';
import { readFileSync } from 'node:fs';

vi.mock('../../lib/api', () => ({
  discussions: {
    participants: vi.fn(),
    meta: vi.fn(),
  },
}));

import { DiscParticipantsHeader } from '../DiscParticipantsHeader';
import { freshnessOf, DEFAULT_AWAY_AFTER_MS } from '../../lib/discPresence';
import { discussions as discussionsApi } from '../../lib/api';

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
  /// KT-581 — the title row carries the invite beside it, and the chip strip
  /// moved down to share row 2. Both rows must hold their content on one line:
  /// a wrap in either is the third header line coming back.
  it('keeps the title and the invite on one row, and the chip strip scrollable', () => {
    const headerCss = readFileSync('src/pages/DiscussionsPage.css', 'utf8');
    const participantCss = readFileSync('src/styles/components.css', 'utf8');
    const headerTop = headerCss.match(/\.disc-chat-header-top\s*\{([^}]*)\}/)?.[1] ?? '';
    const stripInSub = headerCss
      .match(/\.disc-chat-header-sub > \.disc-participants-row\s*\{([^}]*)\}/)?.[1] ?? '';
    const participantList = participantCss.match(/\.disc-participants-list\s*\{([^}]*)\}/)?.[1] ?? '';

    expect(headerTop).toContain('display: grid');
    // Two columns: title, then the invite pinned right on the same line.
    expect(headerTop).toContain('grid-template-columns: minmax(0, 1fr) auto');
    expect(stripInSub).toContain('min-width: 0');
    expect(participantList).toContain('flex-wrap: nowrap');
    expect(participantList).toContain('overflow-x: auto');
  });

  it('shows the empty-state copy when the disc has no participants', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-empty" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-2" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-models" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-resume" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-no-resume" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-3" t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const chip = document.querySelector('.disc-participant-chip') as HTMLElement;
    expect(chip).not.toBeNull();
    expect(chip.getAttribute('data-status')).toBe('paused');
  });

  it('refetches participants when discId changes', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    const { rerender } = render(<DiscParticipantsHeader discId="d-1" t={t} />);
    await act(async () => { await Promise.resolve(); });
    expect(discussionsApi.participants).toHaveBeenCalledWith('d-1');

    rerender(<DiscParticipantsHeader discId="d-2" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-act" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-wait" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-unk" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-honest" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-paused" t={t} />);
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
      render(<DiscParticipantsHeader discId="d-obligation" t={t} />);
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

  it('renders durable native resume reason/since and keeps stall/quota distinct', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        id: -1, agent_type: 'Ollama', session_id: null, role: 'native_resume', status: 'active',
        presence_state: 'resume_expected', read_live: false, write_state: 'unknown',
        wake_mode: 'native_dispatch', resume_reason: 'waiting for CI',
        resume_since: '2026-08-21T12:00:00Z',
      },
      {
        id: -2, agent_type: 'Codex', session_id: null, role: 'native_resume', status: 'active',
        presence_state: 'stalled', read_live: false, write_state: 'unknown',
        wake_mode: 'native_dispatch',
      },
      {
        id: -3, agent_type: 'ClaudeCode', session_id: null, role: 'native_resume', status: 'active',
        presence_state: 'quota_exhausted', read_live: false, write_state: 'unknown',
        wake_mode: 'native_dispatch',
      },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-resume" t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    const chips = [...document.querySelectorAll('.disc-participant-chip')];
    expect(chips.map(chip => chip.getAttribute('data-presence'))).toEqual([
      'resume_expected', 'stalled', 'quota_exhausted',
    ]);
    fireEvent.click(chips[0] as HTMLButtonElement);
    const details = document.querySelector('.disc-participant-details')?.textContent ?? '';
    expect(details).toContain('disc.presenceResumeExpected');
    expect(details).toContain('disc.targetNative');
    expect(details).toContain('waiting for CI');
    expect(details).toContain('disc.resumeSince');
  });

  it('keeps 3 offline peers inside the scrollable chip strip', async () => {
    // Regression: with 3+ offline "reconnexion requise" peers the strip widens.
    // It must scroll rather than wrap — the row shares a line with the agent
    // control and the details toggle, and a wrap there costs a header line.
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 1, agent_type: 'ClaudeCode', session_id: 'sA', role: 'owner', status: 'active', presence_state: 'offline' },
      { id: 2, agent_type: 'Codex', session_id: 'sB', role: 'peer', status: 'active', presence_state: 'offline' },
      { id: 3, agent_type: 'GeminiCli', session_id: 'sC', role: 'peer', status: 'active', presence_state: 'offline' },
    ]);
    await act(async () => {
      render(<DiscParticipantsHeader discId="d-3offline" t={t} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const list = document.querySelector('.disc-participants-list');
    expect(list?.querySelectorAll('.disc-participant-chip')).toHaveLength(3);
    // Minting a token is an action on the discussion, not on the strip: it
    // belongs to DiscInviteButton, on the row above.
    expect(document.querySelector('.disc-participants-invite-btn')).toBeNull();
    // Every chip renders the honest "offline" state.
    document.querySelectorAll('.disc-participant-chip').forEach(chip => {
      expect(chip.getAttribute('data-presence')).toBe('offline');
    });
  });

  /// A peer invited from the row above must show up without waiting out the
  /// 5s poll.
  it('refetches when the invite button bumps refreshKey', async () => {
    (discussionsApi.participants as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    const { rerender } = render(<DiscParticipantsHeader discId="d-r" t={t} refreshKey={0} />);
    await act(async () => { await Promise.resolve(); });
    const before = (discussionsApi.participants as ReturnType<typeof vi.fn>).mock.calls.length;

    await act(async () => {
      rerender(<DiscParticipantsHeader discId="d-r" t={t} refreshKey={1} />);
      await Promise.resolve();
    });
    expect((discussionsApi.participants as ReturnType<typeof vi.fn>).mock.calls.length)
      .toBeGreaterThan(before);
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
