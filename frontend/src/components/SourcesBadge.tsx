import { AlertTriangle, ShieldCheck } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import type { DriftCheckResponse } from '../types/generated';

/**
 * "Sources clean" badge, recomputed from the lines the docs cite. Kept apart
 * from the dated "Validated" milestone: it turns to "to re-verify" when a cited
 * line changes, without a new audit.
 */
export function SourcesBadge({ drift }: { drift: DriftCheckResponse | undefined }) {
  const { t } = useT();
  if (!drift || drift.sources_state === 'unknown') return null;
  if (drift.sources_state === 'clean') {
    return (
      <span className="dash-badge-green" data-testid="project-sources-state" data-state="clean" title={t('audit.sourcesCleanHint')}>
        <ShieldCheck size={9} /> {t('audit.sourcesClean')}
      </span>
    );
  }
  // KT-1168 — paths the prose names that are gone are counted apart from the
  // cited lines that changed: the label says which is which.
  const paths = drift.missing_paths ?? 0;
  const label = paths === 0
    ? t('audit.sourcesToReverify', String(drift.broken_citations))
    : drift.broken_citations === 0
      ? t('audit.sourcesToReverifyPathsOnly', String(paths))
      : t('audit.sourcesToReverifyPaths', String(drift.broken_citations), String(paths));
  return (
    <span className="dash-badge-drift" data-testid="project-sources-state" data-state="needs_reverification" title={t('audit.sourcesToReverifyHint')}>
      <AlertTriangle size={9} /> {label}
    </span>
  );
}
