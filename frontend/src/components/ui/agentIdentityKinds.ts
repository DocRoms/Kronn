// KT-1936 — the pieces of an agent's identity, shared by AgentIdentity and by
// the callers that write the same identity into a title, a tooltip or an
// accessible name (a chip button, a list row).
import type { ModelTier } from '../../types/generated';

/** Who the agent is in this discussion. Never replaces the model. */
export type AgentIdentityRole = 'main' | 'oneOff' | 'cli' | 'workflowStep';

export const AGENT_IDENTITY_ROLES: readonly AgentIdentityRole[] = ['main', 'oneOff', 'cli', 'workflowStep'];

/**
 * The model next to the agent: a concrete id, or one of three explicit
 * unknown states instead of a blank or a tier name.
 * - `notDeclared`: a joined CLI session that declared none at join;
 * - `notRecorded`: a legacy message whose model was not stored;
 * - `providerDefault`: the provider's default, a model nobody can name yet
 *   (« default model »: the agent's name is written just before it).
 */
export type AgentIdentityModel =
  | { kind: 'id'; id: string }
  | { kind: 'notDeclared' }
  | { kind: 'notRecorded' }
  | { kind: 'providerDefault' };

export type AgentIdentityUnknown = Exclude<AgentIdentityModel['kind'], 'id'>;

export const AGENT_IDENTITY_UNKNOWN_STATES: readonly AgentIdentityUnknown[] = ['notDeclared', 'notRecorded', 'providerDefault'];

/** full: a detail header, the participant popover. inline: a message header,
 *  an activity bubble, a participant chip, the picker trigger. compact: a
 *  discussion list row, where the model is only in the accessible name. */
export type AgentIdentitySize = 'full' | 'inline' | 'compact';

/** Past this many characters a truncated model id ends with an ellipsis
 *  (the participant chip, the picker trigger); the full id stays in the
 *  tooltip, the accessible name and the details. */
export const AGENT_IDENTITY_MODEL_MAX_CHARS = 17;

/** A concrete id when there is one, else `fallback`. */
export function agentModel(
  id: string | null | undefined,
  fallback: AgentIdentityUnknown,
): AgentIdentityModel {
  const trimmed = id?.trim();
  return trimmed ? { kind: 'id', id: trimmed } : { kind: fallback };
}

type Translate = (key: string, ...args: (string | number)[]) => string;

/** The model as written: its id, or the words of its unknown state. */
export function agentModelText(model: AgentIdentityModel, t: Translate): string {
  switch (model.kind) {
    case 'id': return model.id;
    case 'notDeclared': return t('ui.agentIdentity.model.notDeclared');
    case 'notRecorded': return t('ui.agentIdentity.model.notRecorded');
    case 'providerDefault': return t('ui.agentIdentity.model.providerDefault');
  }
}

export const agentRoleText = (role: AgentIdentityRole, t: Translate) => t(`ui.agentIdentity.role.${role}`);

export interface AgentIdentityTextParts {
  name: string;
  model: AgentIdentityModel;
  role?: AgentIdentityRole | null;
  /** Written instead of the role's own label (a custom kind, a demo). */
  roleLabel?: string | null;
  tier?: ModelTier | null;
  tierLabel?: string | null;
}

/** « name · model · role »: the same identity, for a title, a tooltip or an
 *  accessible name (the full model id, never cut). */
export function agentIdentityText(parts: AgentIdentityTextParts, t: Translate): string {
  const role = parts.roleLabel ?? (parts.role ? agentRoleText(parts.role, t) : null);
  return [
    parts.name,
    agentModelText(parts.model, t),
    parts.tierLabel ?? null,
    role,
  ].filter(Boolean).join(' · ');
}
