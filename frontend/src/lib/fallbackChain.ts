import { AGENT_LABELS } from './constants';
import type { AgentFallback, AgentType } from '../types/generated';

/** The longest chain the backend accepts (`MAX_CHAIN_DEPTH`). */
export const MAX_FALLBACK_CHAIN = 4;

const label = (agent: AgentType) => AGENT_LABELS[agent] ?? agent;

/** What is wrong with `chain` whatever the configuration: the same rules the
 *  backend refuses on, so the owner sees them before anything is sent. */
export function chainProblem(
  origin: AgentType | null,
  chain: AgentFallback[],
): { key: string; args: string[] } | null {
  if (chain.length > MAX_FALLBACK_CHAIN) return { key: 'fallback.tooDeep', args: [String(MAX_FALLBACK_CHAIN)] };
  const seen: AgentType[] = origin ? [origin] : [];
  for (const link of chain) {
    if (link.agent === origin) return { key: 'fallback.self', args: [label(link.agent)] };
    if (seen.includes(link.agent)) return { key: 'fallback.loop', args: [label(link.agent)] };
    seen.push(link.agent);
  }
  return null;
}
