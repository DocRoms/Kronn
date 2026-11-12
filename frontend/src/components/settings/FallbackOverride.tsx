import { useState } from 'react';
import { useT } from '../../lib/I18nContext';
import { useFallbackData } from '../../hooks/useFallbackData';
import { inheritedAfter } from '../../lib/fallbackChain';
import type { AgentFallback, AgentType, FallbackMode } from '../../types/generated';
import { FallbackPill, FallbackRow } from './FallbackRow';
import './AgentFallbackControl.css';

type State = 'default' | 'cut' | 'custom';

const stateOf = (value: AgentFallback[] | null | undefined): State =>
  value == null ? 'default' : value.length === 0 ? 'cut' : 'custom';

interface Props {
  testId: string;
  /** The agent the turn or the step starts on: its row is what « default » means. */
  origin: AgentType | null;
  /** The mode the turn or the step runs in. */
  mode: FallbackMode;
  /** `null`: no override, the row of the agent applies. `[]`: no fallback here. */
  value: AgentFallback[] | null | undefined;
  /** Stores the override (`null` removes it); a refusal rejects with its reason. */
  onChange: (next: AgentFallback[] | null) => Promise<void>;
}

/** The override of one mode, for a discussion or a workflow Agent step — the
 *  very row of the agent cards: replace the fallback, go back to the default of
 *  the agent, or cut the fallback here. */
export function FallbackOverride({ testId, origin, mode, value, onChange }: Props) {
  const { t } = useT();
  const { overview, options } = useFallbackData();
  const [failure, setFailure] = useState<string | null>(null);
  const state = stateOf(value);
  const chains = overview?.chains;
  const own = chains?.find((chain) => chain.agent === origin && chain.mode === mode);

  const run = async (next: AgentFallback[] | null) => {
    setFailure(null);
    try {
      await onChange(next);
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  };

  const defaults = own ? [...own.links.map((link) => ({ source: origin as AgentType, link })), ...own.inherited] : [];

  return (
    <div className="fb-override" data-testid={testId} data-state={state}>
      <FallbackRow
        testId={`${testId}-row`}
        mode={mode}
        origin={origin}
        links={value ?? []}
        inherited={inheritedAfter(chains, origin, mode, value ?? [])}
        options={options}
        empty={state === 'default' ? (
          <span className="fb-override-default" data-testid={`${testId}-default-chain`}>
            {t('fallback.defaultLabel')}
            {defaults.length === 0 && <span className="fbr-off">{t('fallback.off')}</span>}
            {defaults.map((entry, index) => (
              <FallbackPill
                key={`${entry.link.agent}-${index}`}
                testId={`${testId}-default-${index}`}
                tone="inherited"
                agent={entry.link.agent}
                model={entry.link.model}
                transport={entry.link.transport}
                title={t('fallback.defaultLabel')}
              />
            ))}
          </span>
        ) : (
          <span className="fbr-off" data-testid={`${testId}-cancelled`}>{t('fallback.cancelled')}</span>
        )}
        actions={(
          <>
            {state !== 'default' && (
              <button
                type="button"
                className="fb-override-action"
                data-testid={`${testId}-mode-default`}
                onClick={() => void run(null)}
              >
                {t('fallback.backToDefault')}
              </button>
            )}
            {state !== 'cut' && (
              <button
                type="button"
                className="fb-override-action"
                data-testid={`${testId}-mode-none`}
                onClick={() => void run([])}
              >
                {t('fallback.cut')}
              </button>
            )}
          </>
        )}
        onChange={(next) => onChange(next)}
      />
      {failure && <div className="fbr-error" role="alert">{t('fallback.refused', failure)}</div>}
    </div>
  );
}
