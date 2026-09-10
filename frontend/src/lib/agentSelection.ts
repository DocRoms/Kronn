import type { AgentSettings, ModelTier } from '../types/generated';

/** Called only after an explicit target/tier choice, never during catalogue refresh. */
export function agentSettingsForSelection(
  previous: AgentSettings | null | undefined,
  tier: ModelTier,
  connectionId?: string | null,
): AgentSettings {
  return {
    ...previous,
    tier,
    connection_id: connectionId ?? null,
    model: null,
    reasoning_effort: null,
  };
}
