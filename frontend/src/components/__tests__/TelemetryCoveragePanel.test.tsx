// TelemetryCoveragePanel — KT-190.
//
// The panel exists to stop one specific lie: a token total that looks complete
// while most sessions were never measured. On one real session, 4.1 billion
// tokens of traffic were stored as zero because Kronn never spawned that CLI.
//
// So the tests are about honesty, not layout: an unmeasured session must show
// as UNKNOWN, a failed query must not render as 0%, and a vendor with no
// collector must not be counted as covered.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  telemetry: { coverage: vi.fn() },
  // Re-implemented rather than imported: the module is mocked wholesale, and
  // the real function is one line whose behaviour is asserted below.
  measuredRatio: (row: {
    sessions: number;
    attributed: number;
    attributed_without_counters: number;
  }) =>
    row.sessions === 0
      ? null
      : (row.attributed - row.attributed_without_counters) / row.sessions,
}));

import { telemetry } from '../../lib/api';
import { TelemetryCoveragePanel } from '../TelemetryCoveragePanel';
import type { TelemetryCoverage } from '../../types/generated';

const row = (over: Partial<TelemetryCoverage> = {}): TelemetryCoverage => ({
  agent_type: 'ClaudeCode',
  sessions: 4,
  attributed: 4,
  attributed_without_counters: 0,
  ...over,
});

const mockCoverage = telemetry.coverage as unknown as ReturnType<typeof vi.fn>;

describe('TelemetryCoveragePanel', () => {
  beforeEach(() => {
    mockCoverage.mockReset();
  });

  it('shows full coverage when every session is measured', async () => {
    mockCoverage.mockResolvedValue([row()]);
    render(<TelemetryCoveragePanel />);
    await waitFor(() => expect(screen.getByText('100 %')).toBeTruthy());
    // No unknown BADGE (the footer note always mentions the word, so the
    // assertion has to match the counted form).
    expect(screen.queryByText(/\d+ inconnue/)).toBeNull();
  });

  it('names unmeasured sessions as unknown rather than hiding them', async () => {
    // The core case: two sessions of four carry no readable counter. Their cost
    // is unknown, and a panel that rounded that to "50 %" without saying so
    // would let a reader assume the other two were free.
    mockCoverage.mockResolvedValue([row({ sessions: 4, attributed: 2 })]);
    render(<TelemetryCoveragePanel />);
    await waitFor(() => expect(screen.getByText('50 %')).toBeTruthy());
    expect(screen.getByText(/2 inconnues/)).toBeTruthy();
  });

  it('does not credit a vendor whose rows carry no counters', async () => {
    // Codex/Copilot get a row saying "nothing measured". Counting that as
    // coverage would claim attribution Kronn does not have.
    mockCoverage.mockResolvedValue([
      row({ agent_type: 'Codex', sessions: 3, attributed: 3, attributed_without_counters: 3 }),
    ]);
    render(<TelemetryCoveragePanel />);
    await waitFor(() => expect(screen.getByText('0 %')).toBeTruthy());
    expect(screen.getByText(/3 inconnues/)).toBeTruthy();
  });

  it('renders a failed query as unknown, never as zero coverage', async () => {
    // Turning a network error into 0% would invent a measurement.
    mockCoverage.mockRejectedValue(new Error('backend down'));
    render(<TelemetryCoveragePanel />);
    await waitFor(() => expect(screen.getByText(/backend down/)).toBeTruthy());
    expect(screen.getByText(/couverture inconnue/)).toBeTruthy();
    expect(screen.queryByText('0 %')).toBeNull();
  });

  it('omits agent types that have no sessions at all', async () => {
    // 0% would read as a failure where there is nothing to measure yet.
    mockCoverage.mockResolvedValue([
      row(),
      row({ agent_type: 'Kiro', sessions: 0, attributed: 0 }),
    ]);
    render(<TelemetryCoveragePanel />);
    await waitFor(() => expect(screen.getByText('ClaudeCode')).toBeTruthy());
    expect(screen.queryByText('Kiro')).toBeNull();
  });

  it('degrades to unknown when the API surface throws synchronously', async () => {
    // The panel is mounted inside a shared zone of the Dashboard. A telemetry
    // failure must never take the page down with it — the same rule the bridge
    // follows. This regression was real: a missing mock crashed the project
    // list into its ErrorBoundary.
    mockCoverage.mockImplementation(() => {
      throw new TypeError('coverage is not a function');
    });
    render(<TelemetryCoveragePanel />);
    await waitFor(() =>
      expect(screen.getByText(/coverage is not a function/)).toBeTruthy(),
    );
    expect(screen.getByText(/couverture inconnue/)).toBeTruthy();
  });

  it('states plainly that an unmeasured session is not a free one', async () => {
    mockCoverage.mockResolvedValue([row()]);
    render(<TelemetryCoveragePanel />);
    await waitFor(() =>
      expect(screen.getByText(/pas gratuite/)).toBeTruthy(),
    );
  });
});
