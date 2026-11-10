// KT-1525 — the « heavy output » warning of a workflow step: its size, which
// step it is, and how to lighten it. A CollectApiData step also lists what each
// source weighs, so the heavy one is obvious.
import { AlertTriangle } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { formatBytes } from '../../lib/live-page-weight';
import type { SourceWeight } from '../../lib/heavyOutput';

/** The chip on a step row: the size of an output above the threshold. */
export function HeavyOutputBadge({ bytes }: { bytes: number }) {
  const { t } = useT();
  const size = formatBytes(bytes);
  return (
    <span
      className="wf-step-kind-badge"
      data-kind="heavy-output"
      data-testid="wf-heavy-output-badge"
      style={{ color: 'var(--kr-warning)', borderColor: 'rgba(var(--kr-warning-rgb), 0.4)' }}
      title={t('wf.heavyOutput.badgeTitle', size)}
    >
      <AlertTriangle size={9} /> {t('wf.heavyOutput.badge')} · {size}
    </span>
  );
}

/** The detail of a heavy output: the step, its size, the fix, and the sources. */
export function HeavyOutputNotice({ stepName, bytes, sources }: {
  stepName: string;
  bytes: number;
  sources: SourceWeight[];
}) {
  const { t } = useT();
  const ranked = [...sources].sort((a, b) => b.bytes - a.bytes);
  return (
    <div className="wf-heavy-output" data-testid="wf-heavy-output" role="note">
      <div className="flex-row gap-3">
        <AlertTriangle size={12} style={{ color: 'var(--kr-warning)' }} />
        <strong>{t('wf.heavyOutput.title', stepName, formatBytes(bytes))}</strong>
      </div>
      <p className="text-xs text-faint">{t('wf.heavyOutput.fix')}</p>
      {ranked.length > 0 && (
        <ul className="text-xs" data-testid="wf-heavy-output-sources">
          {ranked.map(source => (
            <li key={source.alias}>
              <span style={{ fontFamily: 'var(--kr-font-mono)' }}>{source.alias}</span>
              {' · '}{formatBytes(source.bytes)}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** What each source of a CollectApiData step weighs, whether or not the step is heavy. */
export function SourceWeights({ sources }: { sources: SourceWeight[] }) {
  const { t } = useT();
  if (sources.length === 0) return null;
  return (
    <div className="flex-row gap-4 text-xs text-faint" data-testid="wf-source-weights">
      <span>{t('wf.heavyOutput.perSource')}</span>
      {sources.map(source => (
        <span key={source.alias}>
          <span style={{ fontFamily: 'var(--kr-font-mono)' }}>{source.alias}</span>: {formatBytes(source.bytes)}
        </span>
      ))}
    </div>
  );
}
