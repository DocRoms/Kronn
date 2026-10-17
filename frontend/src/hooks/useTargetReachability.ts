import { useEffect, useMemo, useState } from 'react';
import { agents as agentsApi, type ReachabilityTarget, type TargetReachability } from '../lib/api';

/** Matches the backend's probe cache: asking again sooner returns the same answer. */
const TTL_MS = 30_000;
const cache = new Map<string, { at: number; value: TargetReachability }>();

export function reachabilityKey(target: ReachabilityTarget): string {
  return `${target.agent_type}:${target.connection_id ?? ''}`;
}

/** Test seam: forget every cached answer. */
export function clearReachabilityCache() {
  cache.clear();
}

/**
 * Transport state of the HTTP agents a draft names (KT-1154). Asked when the
 * set of targets changes, never per keystroke, and at most once per target
 * every 30 s.
 */
export function useTargetReachability(targets: ReachabilityTarget[]): Map<string, TargetReachability> {
  const key = [...new Set(targets.map(reachabilityKey))].sort().join('|');
  const [answers, setAnswers] = useState<Map<string, TargetReachability>>(new Map());

  useEffect(() => {
    if (!key) return;
    let active = true;
    const now = Date.now();
    const missing = key.split('|')
      .filter(entry => {
        const hit = cache.get(entry);
        return !hit || now - hit.at >= TTL_MS;
      })
      .map(entry => {
        const [agent_type, connection_id] = entry.split(':');
        return { agent_type, connection_id: connection_id || null } as ReachabilityTarget;
      });
    if (missing.length === 0) return;
    agentsApi.reachability(missing).then(results => {
      const at = Date.now();
      for (const result of results) cache.set(reachabilityKey(result), { at, value: result });
      if (!active) return;
      setAnswers(current => {
        const next = new Map(current);
        for (const result of results) next.set(reachabilityKey(result), result);
        return next;
      });
    }).catch(() => {
      // No answer is not a verdict: the chip simply stays without a warning.
    });
    return () => { active = false; };
  }, [key]);

  return useMemo(() => {
    const known = new Map<string, TargetReachability>();
    for (const entry of key ? key.split('|') : []) {
      const value = answers.get(entry) ?? cache.get(entry)?.value;
      if (value) known.set(entry, value);
    }
    return known;
  }, [key, answers]);
}
