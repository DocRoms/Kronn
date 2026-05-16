/**
 * 0.8.5 — Compact metrics chip rendered inside each QP card header.
 *
 * Shows the CURRENT version's launch metrics (avg tokens / avg
 * duration / launch count) in one line so the user gets a fitness
 * signal without opening the history drawer. Hidden when the QP has
 * no launches yet (avoids "0 tk · 0s" noise on freshly-created QPs).
 *
 * Reuses the `quickPromptsApi.metrics` endpoint — one call per card.
 * `useApi` caches per QP id so re-renders don't re-fetch.
 */
import { useApi } from '../hooks/useApi';
import { quickPrompts as quickPromptsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';

interface Props {
  qpId: string;
  /** Optional refreshKey: bump from the parent to force a refetch
   *  (e.g. after a deploy of an improved version). */
  refreshKey?: number;
}

function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

function fmtDuration(ms: number | null | undefined): string {
  if (ms == null) return '—';
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rs = Math.round(s - m * 60);
  return `${m}m${String(rs).padStart(2, '0')}`;
}

export default function QPCardMetricsChip({ qpId, refreshKey }: Props) {
  const { t } = useT();
  const { data: metrics } = useApi(() => quickPromptsApi.metrics(qpId), [qpId, refreshKey]);
  // Pick the newest version's row (the backend returns DESC by
  // version_index). When there are no launches → hide the chip.
  const current = metrics && metrics.length > 0 ? metrics[0] : null;
  if (!current || current.launches === 0) return null;
  return (
    <span
      className="qp-card-metrics-chip"
      data-testid={`qp-card-metrics-${qpId}`}
      title={t('qp.metrics.chipTooltip', current.version_index, current.launches)}
    >
      🚀 {current.launches}
      <span className="qp-card-metrics-sep"> · </span>
      💬 {fmtTokens(current.avg_tokens)}
      <span className="qp-card-metrics-sep"> · </span>
      ⏱ {fmtDuration(current.avg_duration_ms)}
    </span>
  );
}
