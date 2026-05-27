/**
 * 0.8.7 — Agent usage & cost card (ccusage) unit tests.
 *
 * Pins the UX fixes that followed the first live smoke:
 *   - formatPeriod: ccusage stamps only a bucket *start*, so weekly must render
 *     a `start → end` range and monthly a localised month (daily passes through)
 *   - pagination: the recent-periods table is capped at ROWS_PER_PAGE per page,
 *     Next advances to older rows, and switching period resets to page 1
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, act } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { formatPeriod, rowsPerPage, UsageSection } from '../settings/UsageSection';
import type { UsageReport, UsageRow } from '../../types/generated';

const { getUsage } = vi.hoisted(() => ({ getUsage: vi.fn() }));
vi.mock('../../lib/api', () => ({
  usage: { get: getUsage },
  // I18nProvider reads the backend UI locale once on mount.
  config: { getUiLanguage: vi.fn().mockResolvedValue('en') },
}));

// Local date stepper (the component's addDaysISO is internal) — handles the
// May→June rollover so 35 consecutive daily buckets are valid ISO dates.
function iso(start: string, offset: number): string {
  const d = new Date(`${start}T00:00:00Z`);
  d.setUTCDate(d.getUTCDate() + offset);
  return d.toISOString().slice(0, 10);
}

function row(period: string): UsageRow {
  return {
    period, agent: 'all', models_used: [], model_breakdowns: [],
    input_tokens: 1, output_tokens: 1, cache_creation_tokens: 0,
    cache_read_tokens: 0, total_tokens: 2, total_cost: 0.5,
  };
}
function report(period_kind: string, periods: string[]): UsageReport {
  return {
    period_kind,
    rows: periods.map(row),
    totals: {
      input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0,
      cache_read_tokens: 0, total_tokens: 0, total_cost: 0,
    },
    agents_detected: ['claude'],
  };
}

const openDetails = async () => {
  await waitFor(() => screen.getByTestId('usage-details-toggle'));
  fireEvent.click(screen.getByTestId('usage-details-toggle'));
};

beforeEach(() => getUsage.mockReset());
afterEach(cleanup);

describe('formatPeriod', () => {
  it('expands a weekly week-start into a start → end range', () => {
    expect(formatPeriod('weekly', '2026-05-25', 'en')).toBe('2026-05-25 → 2026-05-31');
  });
  it('localises a monthly YYYY-MM bucket', () => {
    expect(formatPeriod('monthly', '2026-02', 'en')).toBe('Feb 2026');
  });
  it('passes a daily date through unchanged', () => {
    expect(formatPeriod('daily', '2026-05-27', 'en')).toBe('2026-05-27');
  });
  it('falls back to the raw value on a malformed bucket', () => {
    expect(formatPeriod('monthly', 'garbage', 'en')).toBe('garbage');
    expect(formatPeriod('weekly', 'nope', 'en')).toBe('nope');
  });
});

describe('rowsPerPage', () => {
  it('pages a meaningful span per bucket (≈month / quarter+ / year)', () => {
    expect(rowsPerPage('daily')).toBe(30);
    expect(rowsPerPage('weekly')).toBe(15);
    expect(rowsPerPage('monthly')).toBe(12);
    expect(rowsPerPage('unknown')).toBe(30); // safe default
  });
});

describe('UsageSection pagination', () => {
  it('caps the daily table at rowsPerPage and pages to older rows', async () => {
    const perPage = rowsPerPage('daily'); // 30
    const total = perPage + 5; // 35 → 2 pages
    // ISO dates from 2026-05-01 onward (35 consecutive days, crosses into June).
    const days = Array.from({ length: total }, (_, i) => iso('2026-05-01', i));
    getUsage.mockResolvedValue(report('daily', days));

    await act(async () => {
      render(<I18nProvider><UsageSection /></I18nProvider>);
    });
    await openDetails();

    const firstCell = () => screen.getByTestId('usage-table').querySelector('tbody tr td')!.textContent;
    expect(screen.getAllByRole('row').length - 1).toBe(perPage); // minus header
    expect(screen.getByTestId('usage-page-indicator').textContent).toBe(`1 / ${Math.ceil(total / perPage)}`);
    expect(firstCell()).toBe(days[total - 1]); // newest first

    fireEvent.click(screen.getByTestId('usage-next-page'));
    expect(screen.getByTestId('usage-page-indicator').textContent).toBe(`2 / ${Math.ceil(total / perPage)}`);
    // page 2 starts at the (perPage+1)-th newest row
    expect(firstCell()).toBe(days[total - 1 - perPage]);
    expect(screen.getAllByRole('row').length - 1).toBe(total - perPage); // remainder
  });

  it('hides the pager when rows fit on a single page', async () => {
    getUsage.mockResolvedValue(report('daily', ['2026-05-27', '2026-05-26']));
    await act(async () => {
      render(<I18nProvider><UsageSection /></I18nProvider>);
    });
    await openDetails();
    expect(screen.queryByTestId('usage-pager')).toBeNull();
  });

  it('renders weekly buckets as start → end ranges', async () => {
    getUsage.mockResolvedValue(report('weekly', ['2026-05-18', '2026-05-11']));
    await act(async () => {
      render(<I18nProvider><UsageSection /></I18nProvider>);
    });
    await openDetails();
    const cells = [...screen.getByTestId('usage-table').querySelectorAll('tbody tr td:first-child')]
      .map(c => c.textContent);
    expect(cells).toContain('2026-05-18 → 2026-05-24');
  });
});
