import { Shuffle } from 'lucide-react';
import type { ReactNode } from 'react';
import { useT } from '../../lib/I18nContext';
import type { AgentFallback, AgentType, ChainView, Cooldown, FallbackMode } from '../../types/generated';
import { inheritedAfter, type FallbackOption } from '../../lib/fallbackChain';
import { FallbackRow } from './FallbackRow';
import './AgentFallbackControl.css';

/** One mode an agent has, with the model it runs on today. */
export interface AgentModeRow {
  mode: FallbackMode;
  /** The model of this mode (`null`: the agent's own default). */
  model: string | null;
}

interface Props {
  agentType: AgentType;
  /** The modes this agent has: eco / standard / advanced, and decision, image or video when it has such a model. */
  modes: AgentModeRow[];
  /** KT-1706 — the model picker of each mode (« Mode · [model ▾] → fallback »). A mode with no picker shows
   *  its resolved model as a read-only pill. */
  pickers?: Partial<Record<FallbackMode, ReactNode>>;
  /** KT-1807 — another setting of a mode, beside its model picker (Claude Code's and Codex's reasoning effort). */
  extras?: Partial<Record<FallbackMode, ReactNode>>;
  /** Links and helps about the models (the provider's model list, the cost help), beside the block's title. */
  tools?: ReactNode;
  /** KT-1706 — what the catalogue says about the pickers (loading, nothing known, stale): one muted line
   *  under the block's title. */
  hint?: ReactNode;
  /** KT-1706 — more rows of the same block, after the agent's own (an HTTP connection's image and video rows). */
  extraRows?: ReactNode;
  /** Every agent/model that could be a fallback; each row keeps the kind of its mode. */
  options: FallbackOption[];
  /** Every mode row that has a fallback, as the backend resolved it (inherited links, gaps, refusals). */
  chains: ChainView[];
  /** Set while this agent's provider is refusing: new turns go to the fallback. */
  cooldown?: Cooldown;
  /** Sets (or, with `null`, removes) the fallback of one mode row; a refusal (loop, depth, kind, access) rejects with its reason. */
  onChange: (mode: FallbackMode, next: AgentFallback | null) => Promise<void>;
  /** A click on an inherited pill: open the card of the agent that holds that fallback. */
  onOpenAgent?: (agent: AgentType) => void;
}

const clockTime = (iso: string) =>
  new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

const REASONS: Record<string, string> = {
  quota_exhausted: 'quota exhausted',
  rate_limit: 'rate limited',
  outage: 'provider unavailable',
  not_authenticated: 'not authenticated',
};

/** Settings > Agents: the one « Models and fallbacks » block of a card, one row
 *  per mode — the label on the left, one column on the right: the model picker,
 *  « If unavailable: », the fallbacks numbered in the order Kronn tries them
 *  (the row's own link, then the ones inherited through other cards' rows).
 *  The model picker sets the mode's model; the fallback is what takes over when
 *  the provider cannot serve it. A discussion or a workflow step can override a
 *  mode; with no override this one applies. */
export function AgentFallbackControl({ agentType, modes, pickers, extras, tools, hint, extraRows, options, chains, cooldown, onChange, onOpenAgent }: Props) {
  const { t } = useT();
  return (
    <div className="set-agent-fallback" data-testid={`agent-fallback-${agentType}`}>
      <div className="set-agent-fallback-head">
        <div className="set-agent-fallback-row" title={t('agents.fallbackHint')}>
          <Shuffle size={12} aria-hidden="true" />
          <span>{t('agents.modelsAndFallbacks')}</span>
        </div>
        {tools && <div className="set-agent-fallback-tools">{tools}</div>}
      </div>
      {hint}
      {modes.map(({ mode, model }) => {
        const view = chains.find((chain) => chain.agent === agentType && chain.mode === mode);
        const links: AgentFallback[] = (view?.links ?? []).map((link) => ({
          agent: link.agent,
          model: link.model ?? null,
          mode: link.mode,
        }));
        return (
          <FallbackRow
            key={mode}
            testId={`agent-fallback-${agentType}-${mode}`}
            mode={mode}
            origin={agentType}
            current={{ agent: agentType, model }}
            model={pickers?.[mode]}
            extra={extras?.[mode]}
            links={links}
            view={view}
            inherited={view?.inherited ?? inheritedAfter(chains, agentType, mode, links)}
            options={options}
            maxLinks={1}
            onOpenAgent={onOpenAgent}
            onChange={async (next) => onChange(mode, next[0] ?? null)}
          />
        );
      })}
      {extraRows}
      {cooldown && (
        <div className="set-agent-fallback-cooldown" data-testid={`agent-fallback-cooldown-${agentType}`}>
          {t('agents.fallbackCooldown', clockTime(cooldown.until), REASONS[cooldown.reason] ?? cooldown.reason)}
        </div>
      )}
    </div>
  );
}
