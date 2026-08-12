/** KT-254 — the header cost display.
 *
 *  The whole point of this component is what it must NOT do: produce a total, or
 *  render an unmeasured CLI as free. Both would be believed. */
import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DiscussionTokenCost } from '../DiscussionTokenCost';
import { compactTokens } from '../../lib/tokenFormat';
import { telemetry } from '../../lib/api';
import type { DiscussionTokenCost as Cost } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  telemetry: { discussionCost: vi.fn() },
}));

const t = (key: string, ...args: string[]) =>
  args.length ? `${key}(${args.join(',')})` : key;

function cost(overrides: Partial<Cost> = {}): Cost {
  return {
    disc_id: 'd',
    in_app_tokens: 12_000,
    in_app_messages: 4,
    cli_traffic_tokens: 4_100_000_000,
    cli_billable_tokens: 66_000_000,
    cli_sessions: 2,
    cli_sessions_measured: 2,
    cli_sessions_unmeasured: 0,
    ...overrides,
  };
}

function mockCost(value: Cost | null) {
  vi.mocked(telemetry.discussionCost).mockResolvedValue(value as Cost);
}

describe('DiscussionTokenCost', () => {
  beforeEach(() => vi.clearAllMocks());

  it('renders the two figures apart and never their sum', async () => {
    // THE rule. 12 000 + 4 100 000 000 must appear nowhere, in any form.
    mockCost(cost());
    render(<DiscussionTokenCost discussionId="d" t={t} />);

    const badge = await screen.findByTestId('disc-token-cost');
    expect(badge.querySelector('[data-part="in-app"]')).not.toBeNull();
    expect(badge.querySelector('[data-part="cli"]')).not.toBeNull();
    expect(badge.textContent).toContain('12k');
    expect(badge.textContent).toContain('4100.0M');
    // No combined figure, and no "total" wording that would imply one.
    expect(badge.textContent).not.toMatch(/4100012|4\.1B|total/i);
  });

  it('shows an unmeasured CLI as unknown, not as zero', async () => {
    // A "0" here is the exact reading a user must never be given: on one real
    // session 4.1 billion tokens were stored as zero.
    mockCost(cost({
      cli_traffic_tokens: null,
      cli_billable_tokens: null,
      cli_sessions: 1,
      cli_sessions_measured: 0,
      cli_sessions_unmeasured: 1,
    }));
    render(<DiscussionTokenCost discussionId="d" t={t} />);

    const badge = await screen.findByTestId('disc-token-cost');
    expect(badge.querySelector('[data-part="cli-unknown"]')).not.toBeNull();
    expect(badge.textContent).toContain('disc.tokenCostUnknown');
    expect(badge.textContent).not.toMatch(/\b0\b/);
  });

  it('keeps an unmeasured CLI visible rather than hiding it', async () => {
    // Hiding it would leave the header looking complete while a joined CLI's spend
    // is entirely unaccounted for.
    mockCost(cost({
      in_app_tokens: 0,
      in_app_messages: 0,
      cli_traffic_tokens: null,
      cli_billable_tokens: null,
      cli_sessions: 1,
      cli_sessions_measured: 0,
      cli_sessions_unmeasured: 1,
    }));
    render(<DiscussionTokenCost discussionId="d" t={t} />);
    expect(await screen.findByTestId('disc-token-cost')).toBeTruthy();
  });

  it('states coverage beside a partial figure', async () => {
    // The number only means something once you know how many sessions it covers,
    // so the ratio is next to it and not in a tooltip.
    mockCost(cost({ cli_sessions: 3, cli_sessions_measured: 1, cli_sessions_unmeasured: 2 }));
    render(<DiscussionTokenCost discussionId="d" t={t} />);
    const badge = await screen.findByTestId('disc-token-cost');
    expect(badge.textContent).toContain('disc.tokenCostPartial(1,3)');
  });

  it('omits the coverage note when every session is measured', async () => {
    // Otherwise the note would appear always and stop carrying information.
    mockCost(cost());
    render(<DiscussionTokenCost discussionId="d" t={t} />);
    const badge = await screen.findByTestId('disc-token-cost');
    expect(badge.textContent).not.toContain('disc.tokenCostPartial');
  });

  it('renders nothing when no agent replied and no CLI ever joined', async () => {
    // An empty badge would read as "measured, and it was free".
    mockCost(cost({
      in_app_tokens: 0,
      in_app_messages: 0,
      cli_traffic_tokens: null,
      cli_billable_tokens: null,
      cli_sessions: 0,
      cli_sessions_measured: 0,
      cli_sessions_unmeasured: 0,
    }));
    render(<DiscussionTokenCost discussionId="d" t={t} />);
    await waitFor(() => expect(telemetry.discussionCost).toHaveBeenCalled());
    expect(screen.queryByTestId('disc-token-cost')).toBeNull();
  });

  it('shows nothing rather than zero when the fetch fails', async () => {
    // Not knowing is not free.
    vi.mocked(telemetry.discussionCost).mockRejectedValue(new Error('offline'));
    render(<DiscussionTokenCost discussionId="d" t={t} />);
    await waitFor(() => expect(telemetry.discussionCost).toHaveBeenCalled());
    expect(screen.queryByTestId('disc-token-cost')).toBeNull();
  });

  it('survives a cost helper that throws synchronously', async () => {
    // Found by breaking four existing header test files: an endpoint missing from
    // a caller's API surface threw inside the effect and blanked the WHOLE header.
    // A cost badge must never be able to take down the title, the agent switcher
    // and the presence list.
    vi.mocked(telemetry.discussionCost).mockImplementation(() => {
      throw new Error('no such endpoint');
    });
    expect(() => render(<DiscussionTokenCost discussionId="d" t={t} />)).not.toThrow();
    await waitFor(() => expect(telemetry.discussionCost).toHaveBeenCalled());
    expect(screen.queryByTestId('disc-token-cost')).toBeNull();
  });

  it('drops the in-app pill when no agent replied but a CLI did', async () => {
    // "agents 0" would claim the agents were measured and cost nothing.
    mockCost(cost({ in_app_tokens: 0, in_app_messages: 0 }));
    render(<DiscussionTokenCost discussionId="d" t={t} />);
    const badge = await screen.findByTestId('disc-token-cost');
    expect(badge.querySelector('[data-part="in-app"]')).toBeNull();
    expect(badge.querySelector('[data-part="cli"]')).not.toBeNull();
  });
});

describe('compactTokens', () => {
  it('keeps a small non-zero cost visible instead of rounding it to zero', () => {
    // A "0 tok" on real spend is the failure this whole ticket is about.
    expect(compactTokens(1)).toBe('<1k');
    expect(compactTokens(999)).toBe('<1k');
    expect(compactTokens(0)).toBe('0');
  });

  it('shortens without inventing precision', () => {
    expect(compactTokens(12_000)).toBe('12k');
    expect(compactTokens(1_500_000)).toBe('1.5M');
    expect(compactTokens(4_100_000_000)).toBe('4100.0M');
  });
});
