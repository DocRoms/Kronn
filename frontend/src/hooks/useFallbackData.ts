import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { agents as agentsApi, config as configApi, modelCatalogApi } from '../lib/api';
import { buildFallbackOptions } from '../lib/fallbackOptions';
import type {
  AgentDetection,
  CatalogModelEntry,
  FallbackOverview,
  ModelTiersConfig,
} from '../types/generated';

/** KT-1693 — what the fallback rows of the settings, a step and a discussion
 *  share: the fallbacks set on every agent's mode rows (with who is cooling
 *  down), and everything a fallback can be. `knownAgents` spares the detection
 *  when the caller already has the agents. */
export function useFallbackData(knownAgents?: AgentDetection[]) {
  const [detected, setDetected] = useState<AgentDetection[]>([]);
  const [tiers, setTiers] = useState<ModelTiersConfig | null>(null);
  const [catalog, setCatalog] = useState<CatalogModelEntry[]>([]);
  const [overview, setOverview] = useState<FallbackOverview | null>(null);
  // Bumped by `reload`: the effect below reads everything again.
  const [generation, setGeneration] = useState(0);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  useEffect(() => {
    let current = true;
    void Promise.allSettled([
      configApi.getAgentFallback(),
      knownAgents ? Promise.resolve(knownAgents) : agentsApi.detect(),
      configApi.getModelTiers(),
      modelCatalogApi.list(),
    ]).then(([fallbacks, agents, modelTiers, snapshot]) => {
      if (!current) return;
      setOverview(fallbacks.status === 'fulfilled' ? fallbacks.value ?? null : null);
      if (agents.status === 'fulfilled') setDetected(agents.value);
      if (modelTiers.status === 'fulfilled') setTiers(modelTiers.value);
      if (snapshot.status === 'fulfilled') setCatalog(snapshot.value.targets.flatMap((target) => target.models));
    });
    return () => { current = false; };
  }, [knownAgents, generation]);

  /** Reads only the fallbacks again, after one was set. */
  const refetch = useCallback(async () => {
    try {
      const next = await configApi.getAgentFallback();
      if (alive.current) setOverview(next ?? null);
    } catch {
      if (alive.current) setOverview(null);
    }
  }, []);

  /** Reads everything again, after the model tiers changed. */
  const reload = useCallback(async () => {
    setGeneration((value) => value + 1);
  }, []);

  const agents = knownAgents ?? detected;
  const options = useMemo(() => buildFallbackOptions(agents, tiers, catalog), [agents, tiers, catalog]);
  return { overview, options, tiers, catalog, refetch, reload };
}
