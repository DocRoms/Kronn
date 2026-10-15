import type {
  AgentType,
  MessageTargetKind,
  ParticipantView,
  PlanningAssignee,
  PlanningAssigneeRole,
} from '../types/generated';
import { AGENT_LABELS } from './constants';

/** The typed identity (KT-116): kind + provider + exact CLI session + connection. */
interface AssigneeIdentity {
  kind: MessageTargetKind;
  agent_type: AgentType;
  cli_session_id?: number | null;
  connection_id?: string | null;
}

export function assigneeKey(identity: AssigneeIdentity): string {
  return [
    identity.kind,
    identity.agent_type,
    identity.cli_session_id ?? '',
    identity.connection_id ?? '',
  ].join(':');
}

/** One agent a human can put on a task from the quick menu. */
export interface AssigneeOption extends AssigneeIdentity {
  key: string;
  /** Stable room ordinal of a joined CLI (`@codex-cli-2` → 2). */
  cliOrdinal: number | null;
}

/**
 * The agents of a room that can be assigned: its configured discussion agent
 * and every joined CLI session — by session, never by provider, so two Codex
 * CLIs stay two options.
 *
 * A `Custom` discussion agent IS its connection: the backend refuses it without
 * `connection_id` (and refuses a `connection_id` on any other provider), so it
 * is offered with its connection, or not at all.
 */
export function assigneeOptions(
  discussionAgent: AgentType | null | undefined,
  participants: ParticipantView[],
  discussionConnectionId?: string | null,
): AssigneeOption[] {
  const options: AssigneeOption[] = [];
  const connectionId = discussionConnectionId?.trim() || null;
  if (discussionAgent && (discussionAgent !== 'Custom' || connectionId)) {
    const identity: AssigneeIdentity = {
      kind: 'discussion_agent',
      agent_type: discussionAgent,
      cli_session_id: null,
      connection_id: discussionAgent === 'Custom' ? connectionId : null,
    };
    options.push({ ...identity, key: assigneeKey(identity), cliOrdinal: null });
  }
  for (const participant of participants) {
    // A workflow step is a Kronn-owned agent, not a terminal a human joined.
    if (participant.workflow_step || participant.status === 'left') continue;
    if (!(participant.agent_type in AGENT_LABELS)) continue;
    const identity: AssigneeIdentity = {
      kind: 'cli',
      agent_type: participant.agent_type as AgentType,
      cli_session_id: participant.id,
      connection_id: null,
    };
    options.push({
      ...identity,
      key: assigneeKey(identity),
      cliOrdinal: participant.cli_ordinal ?? null,
    });
  }
  return options;
}

export function assigneeProviderLabel(agentType: string): string {
  return AGENT_LABELS[agentType] ?? agentType;
}

/** `Codex`, `Codex CLI`, `Codex CLI 2`: the provider plus the room identity. */
export function assigneeLabel(
  identity: AssigneeIdentity & { cli_ordinal?: number | null; cliOrdinal?: number | null },
  cliWord: string,
): string {
  const provider = assigneeProviderLabel(identity.agent_type);
  if (identity.kind !== 'cli') return provider;
  const ordinal = identity.cli_ordinal ?? identity.cliOrdinal ?? null;
  return ordinal && ordinal > 1
    ? `${provider} ${cliWord} ${ordinal}`
    : `${provider} ${cliWord}`;
}

export function assigneeIdentity(assignee: PlanningAssignee): AssigneeIdentity {
  return {
    kind: assignee.kind,
    agent_type: assignee.agent_type,
    cli_session_id: assignee.cli_session_id ?? null,
    connection_id: assignee.connection_id ?? null,
  };
}

/** The assignee row matching an option, if the task has it. */
export function assigneeFor(
  assignees: PlanningAssignee[],
  option: AssigneeIdentity,
): PlanningAssignee | undefined {
  const key = assigneeKey(option);
  return assignees.find(assignee => assigneeKey(assigneeIdentity(assignee)) === key);
}

export function nextRoleOnToggle(
  current: PlanningAssigneeRole | null | undefined,
  clicked: PlanningAssigneeRole,
): PlanningAssigneeRole | null {
  return current === clicked ? null : clicked;
}
