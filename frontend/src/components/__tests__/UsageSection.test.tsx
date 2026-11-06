/**
 * 0.8.7 — Agent usage & cost card (ccusage) unit tests.
 *
 * Pins the UX fixes that followed the first live smoke:
 *   - formatPeriod: ccusage stamps only a bucket *start*, so weekly must render
 *     a `start → end` range and monthly a localised month (daily passes through)
 *   - the selector scopes the report: each period shows its own total (#270)
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, act } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { UsageSection } from '../settings/UsageSection';
import { formatPeriod } from '../settings/usageFormat';
import type { UsageReport, UsageRow } from '../../types/generated';

const { getUsage } = vi.hoisted(() => ({ getUsage: vi.fn() }));
vi.mock('../../lib/api', () => ({
  usage: { get: getUsage },
  // I18nProvider reads the backend UI locale once on mount.
  config: { getUiLanguage: vi.fn().mockResolvedValue('en') },
}));

function row(period: string): UsageRow {
  return {
    period, agent: 'all', models_used: [], model_breakdowns: [],
    input_tokens: 1, output_tokens: 1, cache_creation_tokens: 0,
    cache_read_tokens: 0, total_tokens: 2, total_cost: 0.5,
  };
}
function report(period_kind: string, periods: string[], total_cost = 0): UsageReport {
  return {
    period_kind,
    rows: periods.map(row),
    totals: {
      input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0,
      cache_read_tokens: 0, total_tokens: 0, total_cost,
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

describe('UsageSection period scope', () => {
  it('shows a different total for each period (#270)', async () => {
    // ccusage used to return the all-time total whatever the period.
    getUsage.mockImplementation(async (period: string) => ({
      daily: report('daily', ['2026-10-27'], 3.7),
      weekly: report('weekly', ['2026-10-26'], 44.5),
      monthly: report('monthly', ['2026-10'], 874.06),
    })[period]);

    await act(async () => {
      render(<I18nProvider><UsageSection /></I18nProvider>);
    });
    await waitFor(() => expect(screen.getByTestId('usage-total-cost').textContent).toBe('$3.70'));

    fireEvent.click(screen.getByTestId('usage-period-weekly'));
    await waitFor(() => expect(screen.getByTestId('usage-total-cost').textContent).toBe('$44.50'));

    fireEvent.click(screen.getByTestId('usage-period-monthly'));
    await waitFor(() => expect(screen.getByTestId('usage-total-cost').textContent).toBe('$874.06'));

    expect(getUsage.mock.calls.map(call => call[0])).toEqual(['daily', 'weekly', 'monthly']);
  });

  it('lists the rows of the report newest first, with no pager', async () => {
    getUsage.mockResolvedValue(report('daily', ['2026-05-26', '2026-05-27']));
    await act(async () => {
      render(<I18nProvider><UsageSection /></I18nProvider>);
    });
    await openDetails();

    const cells = [...screen.getByTestId('usage-table').querySelectorAll('tbody tr td:first-child')]
      .map(c => c.textContent);
    expect(cells).toEqual(['2026-05-27', '2026-05-26']);
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
