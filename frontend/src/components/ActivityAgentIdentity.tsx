import { Cpu } from 'lucide-react';
import { AGENT_LABELS, agentTextColor } from '../lib/constants';
import type { AgentType } from '../types/generated';
import { AgentIdentity } from './ui/AgentIdentity';
import type { AgentIdentityModel, AgentIdentityRole } from './ui/agentIdentityKinds';

/** KT-1936 — the agent of an activity bubble (pending, streaming, failed):
 *  its name, its model and its role. The caller resolves the model the way
 *  the header does (`discussionAgentModel`: exact model, the mode's model,
 *  the last reply, else the « default model » state). */
export function ActivityAgentIdentity({ agent, label, role = 'main', model }: {
  agent: AgentType;
  label?: string;
  role?: AgentIdentityRole;
  model: AgentIdentityModel;
}) {
  const name = label ?? AGENT_LABELS[agent] ?? agent;
  return (
    <AgentIdentity
      icon={<Cpu size={12} />}
      name={name}
      color={agentTextColor(agent)}
      model={model}
      role={role}
    />
  );
}
