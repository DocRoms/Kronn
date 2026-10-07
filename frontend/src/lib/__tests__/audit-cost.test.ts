import { describe, it, expect } from 'vitest';
import { costReasonKey, formatUsd, summarizeAuditCost } from '../audit-cost';

const done = (cost?: number | null) => ({ ended_at: '2026-10-04T10:00:00Z', cost_usd_micros: cost });

describe('summarizeAuditCost (KT-997)', () => {
  it('is exact only when every finished step reported a cost, a real 0 included', () => {
    expect(summarizeAuditCost([done(400_000), done(0), done(20_000)])).toEqual({ kind: 'exact', usdMicros: 420_000, estimated: false });
  });

  it('is a floor naming the unknown steps when some reported nothing', () => {
    expect(summarizeAuditCost([done(420_000), done(null), done(undefined)]))
      .toEqual({ kind: 'floor', usdMicros: 420_000, unknownSteps: 2, estimated: false });
  });

  it('is unknown, never 0, when no finished step reported', () => {
    expect(summarizeAuditCost([done(null), done(undefined)])).toEqual({ kind: 'unknown', unknownSteps: 2 });
  });

  it('counts an estimated step and then calls the whole total estimated', () => {
    const estimated = { ended_at: '2026-10-04T10:00:00Z', cost_usd_micros: null, estimated_cost_usd_micros: 30_000 };
    expect(summarizeAuditCost([done(400_000), estimated])).toEqual({ kind: 'exact', usdMicros: 430_000, estimated: true });
    expect(summarizeAuditCost([estimated, done(null)])).toEqual({ kind: 'floor', usdMicros: 30_000, unknownSteps: 1, estimated: true });
    // A reported cost wins over an estimate.
    expect(summarizeAuditCost([{ ...estimated, cost_usd_micros: 1 }])).toEqual({ kind: 'exact', usdMicros: 1, estimated: false });
  });

  it('ignores a step still running and says nothing before the first step ends', () => {
    expect(summarizeAuditCost([])).toEqual({ kind: 'none' });
    expect(summarizeAuditCost([{ ended_at: null, cost_usd_micros: null }])).toEqual({ kind: 'none' });
    expect(summarizeAuditCost([done(5), { ended_at: null }])).toEqual({ kind: 'exact', usdMicros: 5, estimated: false });
  });
});

describe('formatUsd', () => {
  it('keeps two decimals, and enough digits for a cheap step to show', () => {
    expect(formatUsd(420_000, 'en')).toBe('0.42');
    expect(formatUsd(12_500_000, 'en')).toBe('12.50');
    expect(formatUsd(0, 'en')).toBe('0.00');
    expect(formatUsd(3_100, 'en')).toBe('0.0031');
    expect(formatUsd(420_000, 'fr')).toBe('0,42');
  });
});

describe('costReasonKey', () => {
  it('keys every reason the pricing module gives, and nothing else', () => {
    expect(costReasonKey('no confirmed rate for the serving model')).toBe('auditTimeline.cost.reason.noRateForModel');
    expect(costReasonKey('the serving model was not recorded')).toBe('auditTimeline.cost.reason.modelNotReported');
    expect(costReasonKey('something new from the server')).toBeNull();
  });
});
