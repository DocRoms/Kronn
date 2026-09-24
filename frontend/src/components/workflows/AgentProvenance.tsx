import type { StepResult, WorkflowAgentAttempt } from '../../types/generated';
import { AGENT_LABELS } from '../../lib/constants';

type Translate = (key: string, ...args: (string | number)[]) => string;

const ROLE_KEYS: Record<WorkflowAgentAttempt['role'], string> = {
  Initial: 'wf.attemptRoleInitial',
  Repair: 'wf.attemptRoleRepair',
  Escalation: 'wf.attemptRoleEscalation',
  Review: 'wf.attemptRoleReview',
  Author: 'wf.attemptRoleAuthor',
};

/** What the provider reported, else what Kronn resolved; never the current config. */
function attemptModelLabel(attempt: WorkflowAgentAttempt, t: Translate): string {
  if (attempt.observed_models.length > 0) return attempt.observed_models.join(' / ');
  if (attempt.model_applied === false) return t('wf.attemptModelCliDefault');
  return attempt.resolved_model ?? t('wf.modelUnknown');
}

/** Badge for the step header: the backend-stamped model, or an explicit unknown. */
export function StepModelBadge({ sr, t }: { sr: StepResult; t: Translate }) {
  if (sr.step_kind !== 'Agent') return null;
  if (sr.step_model) {
    return <span className="wf-tier-badge" title={t('wf.modelTierHint')}>{sr.step_model}</span>;
  }
  return (
    <span className="wf-tier-badge" data-unknown="true" title={t('wf.modelUnknownHint')}>
      {t('wf.modelUnknown')}
    </span>
  );
}

/** Every launch of an Agent step, in order, with the one whose output was kept. */
export function AgentProvenanceDetails({ sr, t }: { sr: StepResult; t: Translate }) {
  const provenance = sr.agent_provenance;
  if (!provenance || provenance.attempts.length === 0) return null;
  const selected = provenance.selected_attempt;
  return (
    <div className="wf-native-tools" data-testid="wf-agent-provenance">
      <span className="wf-native-tools-label">{t('wf.attempts')}</span>
      {provenance.attempts.map(attempt => {
        const retained = attempt.id === selected;
        return (
          <span
            key={attempt.id}
            className="wf-native-tool-chip"
            data-ok={attempt.succeeded ? 'true' : 'false'}
            data-retained={retained ? 'true' : 'false'}
            title={retained ? t('wf.attemptRetained') : undefined}
          >
            {retained ? '★ ' : attempt.succeeded ? '✓ ' : '! '}
            {t(ROLE_KEYS[attempt.role])} · {AGENT_LABELS[attempt.agent] ?? attempt.agent} · {attemptModelLabel(attempt, t)}
            {attempt.format_fallback ? ` · ${t('wf.attemptFormatFallback')}` : ''}
            {attempt.duration_ms > 0 ? ` · ${(attempt.duration_ms / 1000).toFixed(1)}s` : ''}
          </span>
        );
      })}
      {selected == null && <span className="text-2xs text-ghost">{t('wf.attemptNoneRetained')}</span>}
    </div>
  );
}
