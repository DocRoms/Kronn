/**
 * KT-997 — what an audit cost, as its agents reported it, else as Kronn
 * estimated it from its rate table. A step with neither is unknown: it is never
 * counted as 0, and a total over it is a floor, never presented as the whole.
 * One estimated step makes the total estimated.
 */

export interface CostedStep {
  ended_at?: string | null;
  cost_usd_micros?: number | null;
  estimated_cost_usd_micros?: number | null;
}

export type AuditCostSummary =
  /** No finished step yet: nothing to say. */
  | { kind: 'none' }
  /** Every finished step has a cost. */
  | { kind: 'exact'; usdMicros: number; estimated: boolean }
  /** Some finished steps have none: the sum of the others is a floor. */
  | { kind: 'floor'; usdMicros: number; unknownSteps: number; estimated: boolean }
  /** No finished step reported a cost. */
  | { kind: 'unknown'; unknownSteps: number };

/** Sum the finished steps' costs; a step still running counts for nothing yet. */
export function summarizeAuditCost(steps: CostedStep[]): AuditCostSummary {
  let usdMicros = 0;
  let known = 0;
  let unknownSteps = 0;
  let estimated = false;
  for (const step of steps) {
    if (!step.ended_at) continue;
    if (typeof step.cost_usd_micros === 'number') {
      usdMicros += step.cost_usd_micros;
      known += 1;
    } else if (typeof step.estimated_cost_usd_micros === 'number') {
      usdMicros += step.estimated_cost_usd_micros;
      known += 1;
      estimated = true;
    } else {
      unknownSteps += 1;
    }
  }
  if (known === 0 && unknownSteps === 0) return { kind: 'none' };
  if (unknownSteps === 0) return { kind: 'exact', usdMicros, estimated };
  if (known === 0) return { kind: 'unknown', unknownSteps };
  return { kind: 'floor', usdMicros, unknownSteps, estimated };
}

/** `0.42`, `0.0031`, `12.50`: dollars with enough digits for a cheap step to show. */
export function formatUsd(usdMicros: number, locale: string): string {
  const usd = usdMicros / 1_000_000;
  if (usd > 0 && usd < 0.01) {
    return usd.toLocaleString(locale, { maximumSignificantDigits: 2 });
  }
  return usd.toLocaleString(locale, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

/** The pricing module's reasons (`core::pricing::UnknownCost::reason`), keyed for i18n. */
const COST_REASON_KEYS: Record<string, string> = {
  'only a token total was reported, not the input/output split': 'auditTimeline.cost.reason.noTokenBreakdown',
  'cache reads were not reported': 'auditTimeline.cost.reason.cacheReadNotReported',
  'cache writes were not reported': 'auditTimeline.cost.reason.cacheWriteNotReported',
  'cache writes were reported but this model has no write rate': 'auditTimeline.cost.reason.cacheWriteUnpriced',
  'the serving model was not recorded': 'auditTimeline.cost.reason.modelNotReported',
  'no confirmed rate for the serving model': 'auditTimeline.cost.reason.noRateForModel',
};

/** The i18n key of a known reason, or `null` to show the server's text as is. */
export function costReasonKey(reason: string): string | null {
  return COST_REASON_KEYS[reason.trim()] ?? null;
}
