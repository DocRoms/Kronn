import { Shuffle } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import type { AgentFallback, AgentType, ChainView, Cooldown } from '../../types/generated';
import { FallbackTimeline } from './FallbackTimeline';
import './AgentFallbackControl.css';

interface Props {
  agentType: AgentType;
  /** The chain stored for this agent, in order; empty = no fallback. */
  current: AgentFallback[];
  /** The agents a fallback may be (installed, not itself, no named connection). */
  candidates: AgentType[];
  /** The chain the backend resolved for this agent, with its gaps. */
  chain?: ChainView;
  /** Set while this agent's provider is refusing: new turns go to the fallback. */
  cooldown?: Cooldown;
  /** Applies the new chain; a refusal (loop, depth, access) rejects with its reason. */
  onChange: (next: AgentFallback[]) => Promise<void>;
}

const clockTime = (iso: string) =>
  new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

const REASONS: Record<string, string> = {
  quota_exhausted: 'quota exhausted',
  rate_limit: 'rate limited',
  outage: 'provider unavailable',
  not_authenticated: 'not authenticated',
};

/** Settings > Agents: the default fallback chain of an agent, as a timeline —
 *  « Claude Code → Codex → Ollama ». A discussion or a workflow step can override
 *  it; with no override this one applies. */
export function AgentFallbackControl({ agentType, current, candidates, chain, cooldown, onChange }: Props) {
  const { t } = useT();
  return (
    <div className="set-agent-fallback" data-testid={`agent-fallback-${agentType}`}>
      <div className="set-agent-fallback-row" title={t('agents.fallbackHint')}>
        <Shuffle size={12} aria-hidden="true" />
        <span>{t('agents.fallbackTo')}</span>
      </div>
      <FallbackTimeline
        testId={`agent-fallback-timeline-${agentType}`}
        origin={agentType}
        chain={current}
        candidates={candidates}
        view={chain}
        onChange={onChange}
      />
      {cooldown && (
        <div className="set-agent-fallback-cooldown" data-testid={`agent-fallback-cooldown-${agentType}`}>
          {t('agents.fallbackCooldown', clockTime(cooldown.until), REASONS[cooldown.reason] ?? cooldown.reason)}
        </div>
      )}
    </div>
  );
}
