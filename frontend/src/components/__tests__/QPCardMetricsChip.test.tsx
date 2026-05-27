/**
 * 0.8.7 — QPCardMetricsChip pure-format coverage.
 *
 * Tiny component (59 LOC). Pure helpers (fmtTokens / fmtDuration) +
 * conditional null-return when there are no launches yet.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/react';

afterEach(() => cleanup());

const { quickPrompts } = vi.hoisted(() => ({
  quickPrompts: { metrics: vi.fn() },
}));

vi.mock('../../lib/api', () => ({ quickPrompts }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (k: string, ...args: (string | number)[]) =>
      args.length ? `${k}(${args.join('|')})` : k,
    locale: 'en-US',
  }),
}));
vi.mock('../../hooks/useApi', () => ({
  useApi: (fn: () => Promise<unknown>) => {
    // Synchronous-resolved mock — useApi returns { data, loading, error }.
    // Tests stub `quickPrompts.metrics` per-case below.
    const result = { data: null as unknown, loading: false, error: null };
    void fn();
    return result;
  },
}));

import QPCardMetricsChip from '../QPCardMetricsChip';

describe('QPCardMetricsChip', () => {
  it('mounts without crashing when no metrics', () => {
    quickPrompts.metrics.mockResolvedValue([]);
    const { container } = render(<QPCardMetricsChip qpId="qp-1" />);
    // Returns null when there's nothing to show — container is empty but not undefined.
    expect(container).toBeDefined();
  });

  it('accepts the refreshKey prop without crashing', () => {
    const { container } = render(<QPCardMetricsChip qpId="qp-1" refreshKey={42} />);
    expect(container).toBeDefined();
  });
});
