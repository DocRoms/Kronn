import type { StepResult } from '../../types/generated';
import { stepTokensUnknown } from './stepTokenStatus';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** Per-step token badge: measured count, explicit unknown, or nothing for a zero. */
export function StepTokensBadge({ sr, t, className }: { sr: StepResult; t: Translate; className: string }) {
  if (stepTokensUnknown(sr)) {
    return (
      <span className={className} data-unknown="true" title={t('wf.stepTokensUnknownHint')}>
        {t('wf.stepTokensUnknown')}
      </span>
    );
  }
  if (typeof sr.tokens_used !== 'number' || sr.tokens_used <= 0) return null;
  return (
    <span className={className} title={t('wf.stepTokensHint')} style={{ color: 'var(--kr-accent-ink)' }}>
      {sr.tokens_used.toLocaleString()} {t('wf.stepTokensSuffix')}
    </span>
  );
}
