/**
 * KT-997 — what an audit cost, as its agents reported it. A step whose agent
 * reported no cost is unknown: it is never counted as 0, and a total over it is
 * a floor, never presented as the whole.
 */

export interface CostedStep {
  ended_at?: string | null;
  cost_usd_micros?: number | null;
}

export type AuditCostSummary =
  /** No finished step yet: nothing to say. */
  | { kind: 'none' }
  /** Every finished step reported its cost. */
  | { kind: 'exact'; usdMicros: number }
  /** Some finished steps reported nothing: the sum of the others is a floor. */
  | { kind: 'floor'; usdMicros: number; unknownSteps: number }
  /** No finished step reported a cost. */
  | { kind: 'unknown'; unknownSteps: number };

/** Sum the finished steps' reported costs; a step still running counts for nothing yet. */
export function summarizeAuditCost(steps: CostedStep[]): AuditCostSummary {
  let usdMicros = 0;
  let known = 0;
  let unknownSteps = 0;
  for (const step of steps) {
    if (!step.ended_at) continue;
    if (typeof step.cost_usd_micros === 'number') {
      usdMicros += step.cost_usd_micros;
      known += 1;
    } else {
      unknownSteps += 1;
    }
  }
  if (known === 0 && unknownSteps === 0) return { kind: 'none' };
  if (unknownSteps === 0) return { kind: 'exact', usdMicros };
  if (known === 0) return { kind: 'unknown', unknownSteps };
  return { kind: 'floor', usdMicros, unknownSteps };
}

/** `0.42`, `0.0031`, `12.50`: dollars with enough digits for a cheap step to show. */
export function formatUsd(usdMicros: number, locale: string): string {
  const usd = usdMicros / 1_000_000;
  if (usd > 0 && usd < 0.01) {
    return usd.toLocaleString(locale, { maximumSignificantDigits: 2 });
  }
  return usd.toLocaleString(locale, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}
