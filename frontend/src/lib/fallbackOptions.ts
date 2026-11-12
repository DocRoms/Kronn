// KT-1693 — what a fallback can be: every installed agent, every model it has for
// a mode. Pure functions over the model tiers and the catalogue, so the card, the
// step editor and the discussion settings offer the very same list.

import { ALL_MODES, type FallbackOption } from './fallbackChain';
import type {
  AgentDetection,
  AgentType,
  CatalogModelEntry,
  FallbackMode,
  ModelTier,
  ModelTiersConfig,
} from '../types/generated';

const TIER_KEY: Partial<Record<AgentType, keyof ModelTiersConfig>> = {
  ClaudeCode: 'claude_code',
  Codex: 'codex',
  OpenCode: 'open_code',
  GeminiCli: 'gemini_cli',
  Kiro: 'kiro',
  Vibe: 'vibe',
  CopilotCli: 'copilot_cli',
  Ollama: 'ollama',
  LiteLlm: 'lite_llm',
  Nvidia: 'nvidia',
};

const TEXT_TIER: Record<'eco' | 'standard' | 'advanced', ModelTier> = {
  eco: 'economy',
  standard: 'default',
  advanced: 'reasoning',
};

type Participant = Pick<AgentDetection, 'agent_type' | 'installed' | 'runtime_available'>;

/** The agents a fallback may be: every installed agent, OpenCode included — an
 *  agent needs no concurrency slot or special card to take part. */
export const canTakePart = (agent: Participant): boolean =>
  agent.agent_type !== 'Custom' && (agent.installed || agent.runtime_available);

/** The model `agent` runs for `mode`: the one configured for it, else the one the
 *  catalogue assigned to that tier, else `null` (the agent's own default).
 *  `undefined`: the agent has no such mode at all. */
export function modelOfMode(
  agent: AgentType,
  mode: FallbackMode,
  tiers: ModelTiersConfig | null | undefined,
  catalog: CatalogModelEntry[] = [],
): string | null | undefined {
  const key = TIER_KEY[agent];
  const config = key ? tiers?.[key] : undefined;
  if (mode === 'decision') return config?.decision?.trim() ? config.decision : undefined;
  if (mode === 'image' || mode === 'video') return undefined;
  const tier = TEXT_TIER[mode];
  const configured = config?.[tier]?.trim();
  if (configured) return configured;
  return catalog.find((entry) => entry.agent_type === agent && entry.tier_assignment === tier)?.model_id ?? null;
}

/** The modes an agent has — the rows of its card. */
export function modesOf(
  agent: AgentType,
  tiers: ModelTiersConfig | null | undefined,
  catalog: CatalogModelEntry[] = [],
): { mode: FallbackMode; model: string | null }[] {
  return ALL_MODES.flatMap((mode) => {
    const model = modelOfMode(agent, mode, tiers, catalog);
    return model === undefined ? [] : [{ mode, model }];
  });
}

/** Every agent/model that could be a fallback, any mode. */
export function buildFallbackOptions(
  agents: Participant[],
  tiers: ModelTiersConfig | null | undefined,
  catalog: CatalogModelEntry[] = [],
): FallbackOption[] {
  return agents.filter(canTakePart).flatMap((agent) =>
    modesOf(agent.agent_type, tiers, catalog).map(({ mode, model }) => ({ agent: agent.agent_type, model, mode })));
}
