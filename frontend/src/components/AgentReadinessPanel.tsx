import type { AgentReadiness, AgentType } from '../types/generated';
import { AGENT_LABELS } from '../lib/constants';
import { blockingAgents, readinessIcon, readinessMessage } from '../lib/agentReadiness';
import { X } from 'lucide-react';

type Translate = (key: string, ...args: (string | number)[]) => string;

export interface AgentReadinessPanelProps {
  agents: AgentType[];
  /** `null` while the check runs. */
  results: AgentReadiness[] | null;
  error?: string | null;
  onFix?: (result: AgentReadiness) => void;
  onRemove?: (agent: AgentType) => void;
  onLaunchAnyway?: () => void;
  onRecheck?: () => void;
  onCancel?: () => void;
  t: Translate;
}

/** Pre-launch check of the selected agents, in the new-discussion form (KT-1107). */
export function AgentReadinessPanel({
  agents, results, error, onFix, onRemove, onLaunchAnyway, onRecheck, onCancel, t,
}: AgentReadinessPanelProps) {
  const checking = results === null && !error;
  const blocked = results ? blockingAgents(results).length > 0 : false;
  const state = checking ? 'checking' : error ? 'error' : blocked ? 'blocked' : 'ok';
  const title = checking
    ? t('readiness.checking')
    : error
      ? t('readiness.error', error)
      : blocked
        ? t('readiness.blocked')
        : t('readiness.title');
  return (
    <div
      className="agent-readiness"
      data-state={state}
      data-testid="agent-readiness"
      role={blocked || error ? 'alert' : 'status'}
    >
      <div className="agent-readiness-title">{title}</div>
      <ul className="agent-readiness-list">
        {agents.map(agent => {
          const result = results?.find(candidate => candidate.agent_type === agent);
          return (
            <li key={agent} className="agent-readiness-row" data-status={result?.status ?? 'checking'}>
              <span className="agent-readiness-icon" aria-hidden="true">{readinessIcon(result)}</span>
              <span className="agent-readiness-text">
                {result ? readinessMessage(result, t) : <strong>{AGENT_LABELS[agent] ?? agent}</strong>}
              </span>
              {result?.status === 'not_ready' && (
                <span className="agent-readiness-row-actions">
                  {onFix && (
                    <button type="button" className="btn btn-sm btn-ghost" onClick={() => onFix(result)}>
                      {t('readiness.fix')}
                    </button>
                  )}
                  {onRemove && (
                    <button type="button" className="btn btn-sm btn-ghost" onClick={() => onRemove(agent)}>
                      {t('readiness.remove')}
                    </button>
                  )}
                </span>
              )}
            </li>
          );
        })}
      </ul>
      {(onLaunchAnyway || onRecheck || onCancel) && (blocked || error || checking) && (
        <div className="agent-readiness-actions">
          {onRecheck && !checking && (
            <button type="button" className="btn btn-sm btn-ghost" onClick={onRecheck}>
              {t('readiness.recheck')}
            </button>
          )}
          {onCancel && (
            <button type="button" className="btn btn-sm btn-ghost" onClick={onCancel}>
              {t('readiness.cancel')}
            </button>
          )}
          {onLaunchAnyway && (
            <button type="button" className="btn btn-sm" onClick={onLaunchAnyway}>
              {t('readiness.launchAnyway')}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** The same state, compact, at the top of the room it was checked for. */
export function AgentReadinessNotice({
  results, onDismiss, t,
}: { results: AgentReadiness[]; onDismiss: () => void; t: Translate }) {
  if (results.length === 0) return null;
  const blocked = blockingAgents(results).length > 0;
  return (
    <div className="agent-readiness-notice" data-state={blocked ? 'blocked' : 'ok'} data-testid="agent-readiness-notice" role="status">
      <span className="agent-readiness-notice-title">
        {blocked ? t('readiness.noticeLaunchedAnyway') : t('readiness.notice')}
      </span>
      {results.map(result => (
        <span key={result.agent_type} className="agent-readiness-chip" title={readinessMessage(result, t)}>
          {readinessIcon(result)} {AGENT_LABELS[result.agent_type] ?? result.agent_type}
        </span>
      ))}
      <button type="button" className="disc-icon-btn" onClick={onDismiss} aria-label={t('common.close')}>
        <X size={12} />
      </button>
    </div>
  );
}
