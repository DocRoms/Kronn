import { useEffect, useMemo, useState } from 'react';
import { externalApi, type ExternalApiConnectionView } from '../lib/api';
import { externalAgentTargets } from '../lib/externalAgentIdentity';
import type { AssistantAgentChoice } from '../lib/assistantAgentChoice';
import type { AgentType } from '../types/generated';
import { AgentSwitchPicker, type AgentSwitchTarget } from './AgentSwitchPicker';

/**
 * KT-1885 — the agent chooser of the API assistants (workflow API step, custom
 * API form): the shared agent picker with the installed agents, the external
 * connections Kronn can route to, and the tiers. It starts on the agent the
 * assistant opened with, at that agent's own default tier.
 */
export function AssistantAgentPicker({ installedAgents, current, onSelect, disabled = false, title, t }: {
  installedAgents: readonly AgentType[];
  current: AssistantAgentChoice;
  onSelect: (choice: AssistantAgentChoice) => void;
  disabled?: boolean;
  title: string;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [connections, setConnections] = useState<ExternalApiConnectionView[]>([]);
  useEffect(() => {
    let live = true;
    // Through a promise: a failing or absent list leaves the installed agents.
    Promise.resolve()
      .then(() => externalApi.list())
      .then(list => { if (live && Array.isArray(list)) setConnections(list); })
      .catch(() => { /* no connection offered: the installed agents remain */ });
    return () => { live = false; };
  }, []);
  const targets = useMemo<AgentSwitchTarget[]>(() => [
    ...installedAgents.map(agent => ({ agent })),
    ...externalAgentTargets(connections).map(({ agent, connectionId, label, modelTiers }) => ({ agent, connectionId, label, modelTiers })),
  ], [installedAgents, connections]);

  return (
    <AgentSwitchPicker
      currentAgent={current.agent}
      currentConnectionId={current.connectionId}
      currentTargetLabel={current.label}
      currentTier={current.tier}
      availableAgents={[...installedAgents]}
      availableTargets={targets}
      tierLabels={{
        economy: t('disc.tier.economy'),
        default: t('disc.tier.default'),
        reasoning: t('disc.tier.reasoning'),
      }}
      defaultModelLabel={t('disc.defaultAgentModel')}
      onTargetSelectionChange={async (target, tier) => onSelect({ ...target, tier })}
      onDefaultTierSelection={async target => onSelect({ ...target, tier: null })}
      disabled={disabled}
      title={title}
      ariaLabel={title}
    />
  );
}
