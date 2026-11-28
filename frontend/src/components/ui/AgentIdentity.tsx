import type { ReactNode } from 'react';
import { useT } from '../../lib/I18nContext';
import { MODEL_TIER_ICONS } from '../../lib/constants';
import type { ModelTier } from '../../types/generated';
import { StatusBadge } from './StatusBadge';
import {
  agentModelText, agentRoleText,
  type AgentIdentityModel, type AgentIdentityRole, type AgentIdentitySize,
} from './agentIdentityKinds';
import './AgentIdentity.css';
import './visuallyHidden.css';

export interface AgentIdentityProps {
  /** The agent as written: its alias, mention or label. */
  name: string;
  /** The name's colour: `agentTextColor()`, which holds 4.5:1 in every theme. */
  color?: string;
  /** The concrete model, or an explicit unknown state. Always written. */
  model: AgentIdentityModel;
  /** Who the agent is here, as its own label. Never replaces the model. */
  role?: AgentIdentityRole | null;
  /** Written in the role chip instead of the role's own label. */
  roleLabel?: string | null;
  /** The mode it runs, as its icon from `MODEL_TIER_ICONS`. */
  tier?: ModelTier | null;
  /** The mode's name: the icon's accessible name (hidden when absent). */
  tierLabel?: string;
  /** An icon before the name, hidden from assistive technology. */
  icon?: ReactNode;
  size?: AgentIdentitySize;
  /** Cut the model id with an ellipsis past 17 characters. The caller keeps
   *  the full id in its tooltip and accessible name (`agentIdentityText`). */
  truncateModel?: boolean;
  className?: string;
}

/**
 * KT-1936 — an agent named the same way everywhere: its name in its colour,
 * its model (a concrete id or an explicit unknown state, in the secondary
 * text role), its role as a separate chip, optionally its mode's icon.
 * Never under the 12 px floor. Inline markup only: it sits inside a button
 * (a chip, the picker trigger) as well as in a line of text.
 */
export function AgentIdentity({
  name, color, model, role, roleLabel, tier, tierLabel, icon,
  size = 'inline', truncateModel = false, className,
}: AgentIdentityProps) {
  const { t } = useT();
  const modelText = agentModelText(model, t);
  const roleText = roleLabel ?? (role ? agentRoleText(role, t) : null);
  const compact = size === 'compact';
  const classes = className ? `ui-agent-identity ${className}` : 'ui-agent-identity';

  const modelNode = (
    <span
      className="ui-agent-identity-model"
      data-state={model.kind}
      data-truncate={truncateModel || undefined}
      data-testid="agent-identity-model"
    >
      {modelText}
    </span>
  );
  const tierNode = tier && (
    tierLabel
      ? <span className="ui-agent-identity-tier" role="img" aria-label={tierLabel} data-tier={tier}>{MODEL_TIER_ICONS[tier]}</span>
      : <span className="ui-agent-identity-tier" aria-hidden="true" data-tier={tier}>{MODEL_TIER_ICONS[tier]}</span>
  );

  return (
    <span className={classes} data-size={size} data-role={role ?? undefined}>
      {icon && <span className="ui-agent-identity-icon" aria-hidden="true">{icon}</span>}
      <span className="ui-agent-identity-name" style={color ? { color } : undefined}>{name}</span>
      {compact ? (
        // The list row is read to find a discussion: the model and the role
        // stay in its accessible name, the header shows them once it opens.
        <span className="ui-visually-hidden">
          {` · ${modelText}`}{roleText ? ` · ${roleText}` : ''}
        </span>
      ) : (
        <>
          <span className="ui-agent-identity-sep">{' · '}</span>
          {modelNode}
          {tierNode}
          {roleText && (
            <>
              <span className="ui-visually-hidden">{' · '}</span>
              <StatusBadge tone="neutral" className="ui-agent-identity-role">{roleText}</StatusBadge>
            </>
          )}
        </>
      )}
    </span>
  );
}
