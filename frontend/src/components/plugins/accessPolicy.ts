import type { ApiAccessRule } from '../../types/generated';

export type Kind = ApiAccessRule['kind'];

/** Whether two endpoint paths name the same template (trailing slash and
 *  leading slash ignored), as the backend matches them. */
export function sameEndpoint(a: { method: string; path: string }, b: { method: string; path: string }): boolean {
  const norm = (p: string) => p.split('?')[0].trim().replace(/^\/+|\/+$/g, '');
  return a.method.trim().toUpperCase() === b.method.trim().toUpperCase() && norm(a.path) === norm(b.path);
}

/** A rule from a kind, keeping the agent list when it stays `agents`. */
export function ruleForKind(kind: Kind, previous?: ApiAccessRule): ApiAccessRule {
  if (kind === 'agents') return { kind, agents: previous?.kind === 'agents' ? previous.agents : [] };
  return { kind } as ApiAccessRule;
}
