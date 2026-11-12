import { useEffect, useState } from 'react';
import { useT } from '../../lib/I18nContext';
import { AGENT_LABELS } from '../../lib/constants';
import { agents as agentsApi, config as configApi } from '../../lib/api';
import type { AgentFallback, AgentType } from '../../types/generated';
import { FallbackTimeline } from './FallbackTimeline';
import './AgentFallbackControl.css';

type Mode = 'default' | 'none' | 'custom';

const modeOf = (value: AgentFallback[] | null | undefined): Mode =>
  value == null ? 'default' : value.length === 0 ? 'none' : 'custom';

interface Props {
  testId: string;
  /** The agent the turn or the step starts on: its default chain is what « default » means. */
  origin: AgentType | null;
  /** `null`: no override, the agent's default chain applies. `[]`: no fallback here. */
  value: AgentFallback[] | null | undefined;
  /** Stores the override (`null` removes it); a refusal rejects with its reason. */
  onChange: (next: AgentFallback[] | null) => Promise<void>;
}

/** The override of a discussion or of a workflow Agent step: use the agent's
 *  default chain, cancel the fallback here, or give this one its own chain. */
export function FallbackOverride({ testId, origin, value, onChange }: Props) {
  const { t } = useT();
  const [mode, setMode] = useState<Mode>(modeOf(value));
  const [defaults, setDefaults] = useState<AgentFallback[]>([]);
  const [candidates, setCandidates] = useState<AgentType[]>([]);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => setMode(modeOf(value)), [value]);

  useEffect(() => {
    let active = true;
    configApi.getAgentFallback()
      .then((overview) => {
        if (!active) return;
        const own = overview.chains.find((chain) => chain.agent === origin);
        setDefaults(own?.links.map((link) => ({ agent: link.agent, model: link.model ?? null, allow_paid: link.allow_paid })) ?? []);
      })
      .catch(() => { if (active) setDefaults([]); });
    agentsApi.detect()
      .then((detected) => {
        if (active) setCandidates(detected.filter((agent) => agent.installed && agent.agent_type !== 'Custom').map((agent) => agent.agent_type));
      })
      .catch(() => { if (active) setCandidates([]); });
    return () => { active = false; };
  }, [origin]);

  const choose = async (next: Mode) => {
    setMode(next);
    setFailure(null);
    try {
      if (next === 'default') await onChange(null);
      else if (next === 'none') await onChange([]);
      // « custom » stores nothing until an entry is added: an empty custom chain
      // would be the same as « no fallback ».
      else if (modeOf(value) !== 'custom') await onChange(defaults.length > 0 ? defaults : null);
    } catch (error) {
      setMode(modeOf(value));
      setFailure(error instanceof Error ? error.message : String(error));
    }
  };

  const defaultText = defaults.length === 0
    ? t('fallback.defaultNone')
    : [origin, ...defaults.map((link) => link.agent)]
      .filter((agent): agent is AgentType => agent !== null)
      .map((agent) => AGENT_LABELS[agent] ?? agent)
      .join(' → ');

  return (
    <div className="fb-override" data-testid={testId}>
      <div className="fb-override-modes" role="radiogroup" aria-label={t('fallback.overrideLabel')}>
        {(['default', 'none', 'custom'] as const).map((option) => (
          <label key={option} className="fb-override-mode" data-active={mode === option}>
            <input
              type="radio"
              name={`${testId}-mode`}
              checked={mode === option}
              data-testid={`${testId}-mode-${option}`}
              onChange={() => void choose(option)}
            />
            <span>{t(`fallback.mode.${option}`)}</span>
          </label>
        ))}
      </div>
      {mode === 'default' && (
        <div className="fb-override-default" data-testid={`${testId}-default`}>
          {t('fallback.defaultIs', defaultText)}
        </div>
      )}
      {mode === 'none' && <div className="fb-empty">{t('fallback.cancelled')}</div>}
      {mode === 'custom' && (
        <FallbackTimeline
          testId={`${testId}-timeline`}
          origin={origin}
          chain={value ?? []}
          candidates={candidates}
          onChange={(next) => onChange(next.length === 0 ? [] : next)}
        />
      )}
      {failure && <div className="fb-error" role="alert">{t('fallback.refused', failure)}</div>}
    </div>
  );
}
