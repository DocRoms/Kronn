import { useState } from 'react';
import { AlertTriangle, ArrowRight, ChevronLeft, ChevronRight, Plus, Scissors, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { AGENT_LABELS } from '../../lib/constants';
import type { AgentFallback, AgentType, ChainView } from '../../types/generated';
import { MAX_FALLBACK_CHAIN, chainProblem } from '../../lib/fallbackChain';
import './AgentFallbackControl.css';

/** Agents billed per call by API key: their use as a fallback needs the owner's opt-in. */
const PAID_AGENTS: readonly AgentType[] = ['LiteLlm', 'Nvidia', 'Custom'];

const label = (agent: AgentType) => AGENT_LABELS[agent] ?? agent;

interface Props {
  /** Prefix of the test ids. */
  testId: string;
  /** The agent the chain belongs to, shown first and never allowed in the chain; `null` for a discussion-wide chain. */
  origin: AgentType | null;
  chain: AgentFallback[];
  /** The agents a link may be (installed, enabled). */
  candidates: AgentType[];
  /** What the backend resolved for this chain (gaps of each link, its own refusal). */
  view?: ChainView;
  disabled?: boolean;
  /** Applies the new chain; a refusal (loop, depth, access) rejects with its reason. */
  onChange: (next: AgentFallback[]) => Promise<void>;
}

/** A fallback chain as a timeline read left to right — origin → first fallback →
 *  next…. Every entry can be changed, given a model, moved earlier or later,
 *  removed, or made the last one (« stop here »); the whole chain can be cut.
 *  A loop, a chain past its limit and the gaps of a link are flagged inline, and
 *  a chain the backend refuses is not stored: its reason is shown instead. */
export function FallbackTimeline({ testId, origin, chain, candidates, view, disabled, onChange }: Props) {
  const { t } = useT();
  const [refusal, setRefusal] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [models, setModels] = useState<Record<number, string>>({});

  const apply = async (next: AgentFallback[]) => {
    const problem = chainProblem(origin, next);
    if (problem) {
      setRefusal(t(problem.key, ...problem.args));
      return;
    }
    setBusy(true);
    try {
      await onChange(next);
      setRefusal(null);
    } catch (error) {
      setRefusal(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setModels({});
    }
  };

  const offered = (current?: AgentType) =>
    candidates.filter((agent) => agent !== origin && (agent === current || !chain.some((link) => link.agent === agent)));
  const replaceAt = (index: number, patch: Partial<AgentFallback>) =>
    chain.map((link, at) => (at === index ? { ...link, ...patch } : link));
  const move = (index: number, by: -1 | 1) => {
    const next = [...chain];
    [next[index], next[index + by]] = [next[index + by], next[index]];
    return next;
  };
  const locked = disabled || busy;
  const gapsOf = (index: number) => view?.links[index]?.gaps ?? [];

  return (
    <div className="fb-timeline" data-testid={testId}>
      <ol className="fb-track" aria-label={t('fallback.timeline')}>
        {origin && (
          <li className="fb-node fb-node--origin" data-testid={`${testId}-origin`}>
            {label(origin)}
          </li>
        )}
        {chain.map((link, index) => (
          <li key={`${link.agent}-${index}`} className="fb-node" data-testid={`${testId}-entry-${index}`}>
            {(origin || index > 0) && <ArrowRight size={12} className="fb-arrow" aria-hidden="true" />}
            <div className="fb-entry">
              <select
                value={link.agent}
                disabled={locked}
                aria-label={t('fallback.agent', String(index + 1))}
                data-testid={`${testId}-agent-${index}`}
                onChange={(event) => void apply(replaceAt(index, { agent: event.target.value as AgentType }))}
              >
                {offered(link.agent).map((agent) => (
                  <option key={agent} value={agent}>{label(agent)}</option>
                ))}
              </select>
              <input
                type="text"
                value={models[index] ?? link.model ?? ''}
                maxLength={200}
                disabled={locked}
                placeholder={t('fallback.modelPlaceholder')}
                aria-label={t('fallback.model', label(link.agent))}
                data-testid={`${testId}-model-${index}`}
                onChange={(event) => setModels({ ...models, [index]: event.target.value })}
                onBlur={() => {
                  if (models[index] === undefined) return;
                  const trimmed = models[index].trim();
                  setModels({});
                  if (trimmed === (link.model ?? '')) return;
                  void apply(replaceAt(index, { model: trimmed === '' ? null : trimmed }));
                }}
              />
              {PAID_AGENTS.includes(link.agent) && (
                <label className="fb-paid">
                  <input
                    type="checkbox"
                    checked={link.allow_paid}
                    disabled={locked}
                    data-testid={`${testId}-paid-${index}`}
                    onChange={(event) => void apply(replaceAt(index, { allow_paid: event.target.checked }))}
                  />
                  <span>{t('fallback.paid')}</span>
                </label>
              )}
              <span className="fb-actions">
                <button
                  type="button"
                  disabled={locked || index === 0}
                  title={t('fallback.moveEarlier')}
                  aria-label={t('fallback.moveEarlier')}
                  data-testid={`${testId}-earlier-${index}`}
                  onClick={() => void apply(move(index, -1))}
                >
                  <ChevronLeft size={12} />
                </button>
                <button
                  type="button"
                  disabled={locked || index === chain.length - 1}
                  title={t('fallback.moveLater')}
                  aria-label={t('fallback.moveLater')}
                  data-testid={`${testId}-later-${index}`}
                  onClick={() => void apply(move(index, 1))}
                >
                  <ChevronRight size={12} />
                </button>
                <button
                  type="button"
                  disabled={locked || index === chain.length - 1}
                  title={t('fallback.stopHere')}
                  aria-label={t('fallback.stopHere')}
                  data-testid={`${testId}-stop-${index}`}
                  onClick={() => void apply(chain.slice(0, index + 1))}
                >
                  <Scissors size={12} />
                </button>
                <button
                  type="button"
                  disabled={locked}
                  title={t('fallback.remove')}
                  aria-label={t('fallback.remove')}
                  data-testid={`${testId}-remove-${index}`}
                  onClick={() => void apply(chain.filter((_, at) => at !== index))}
                >
                  <X size={12} />
                </button>
              </span>
              {gapsOf(index).length > 0 && (
                <span className="fb-gap" role="note">
                  <AlertTriangle size={11} aria-hidden="true" />
                  {t('fallback.gaps', label(link.agent), gapsOf(index).join(', '))}
                </span>
              )}
            </div>
          </li>
        ))}
        {chain.length < MAX_FALLBACK_CHAIN && offered().length > 0 && (
          <li className="fb-node fb-node--add">
            {(origin || chain.length > 0) && <ArrowRight size={12} className="fb-arrow" aria-hidden="true" />}
            <label className="fb-add">
              <Plus size={12} aria-hidden="true" />
              <select
                value=""
                disabled={locked}
                aria-label={t('fallback.add')}
                data-testid={`${testId}-add`}
                onChange={(event) => {
                  const agent = event.target.value as AgentType;
                  if (agent) void apply([...chain, { agent, model: null, allow_paid: false }]);
                }}
              >
                <option value="">{t('fallback.add')}</option>
                {offered().map((agent) => (
                  <option key={agent} value={agent}>{label(agent)}</option>
                ))}
              </select>
            </label>
          </li>
        )}
      </ol>
      {chain.length === 0 && <div className="fb-empty">{t('fallback.empty')}</div>}
      {chain.length > 0 && (
        <button
          type="button"
          className="fb-cut"
          disabled={locked}
          data-testid={`${testId}-cut`}
          onClick={() => void apply([])}
        >
          {t('fallback.cut')}
        </button>
      )}
      {view?.error && (
        <div className="fb-error" role="alert">
          <AlertTriangle size={11} aria-hidden="true" />
          {view.error}
        </div>
      )}
      {refusal && (
        <div className="fb-error" role="alert" data-testid={`${testId}-refused`}>
          <AlertTriangle size={11} aria-hidden="true" />
          {t('fallback.refused', refusal)}
        </div>
      )}
    </div>
  );
}
