import type {
  ActiveAgentDispatch,
  AgentType,
  Discussion,
  DiscussionMessage,
  MessageTarget,
  ParticipantView,
} from '../types/generated';
import { AGENT_MENTIONS, mentionedAgents } from './constants';

export interface ComposerMention {
  /** Stable token inserted in the textarea (`@codex-cli`, for example). */
  trigger: string;
  /** Human-facing identity shown in autocomplete and help. */
  displayTrigger: string;
  label: string;
  type?: AgentType;
  target?: MessageTarget;
  targetAll?: boolean;
}

const agentByWireName = new Map(
  AGENT_MENTIONS.map(mention => [mention.type as string, mention.type]),
);

export function composerMentions(
  discussionAgent: AgentType,
  installedAgentTypes: AgentType[],
  participants: ParticipantView[],
  labels: {
    discussionAgent: string;
    punctualAgent: string;
    cli: string;
    all: string;
  },
): ComposerMention[] {
  const installed = new Set(installedAgentTypes);
  const options: ComposerMention[] = [{
    trigger: '@all',
    displayTrigger: '@all',
    label: labels.all,
    targetAll: true,
  }];

  for (const mention of AGENT_MENTIONS) {
    if (!installed.has(mention.type)) continue;
    const isPrincipal = mention.type === discussionAgent;
    options.push({
      trigger: mention.trigger,
      displayTrigger: mention.trigger,
      label: isPrincipal ? labels.discussionAgent : labels.punctualAgent,
      type: mention.type,
      target: {
        kind: isPrincipal ? 'discussion_agent' : 'agent',
        agent_type: mention.type,
        cli_session_id: null,
        tier: isPrincipal ? null : 'default',
      },
    });
  }

  // KT-247 — the ordinal is now the backend's STABLE per-(disc, provider) rank
  // (`participant.cli_ordinal`), not a positional count over the rendered list.
  // A positional count renumbers the moment a CLI leaves or the list is
  // filtered, so `@claude-cli-2` could silently point at a different session
  // between two renders. The backend ranks by the AUTOINCREMENT session id, so
  // the alias is permanent. Fallback to a positional count only when the field
  // is absent (older payload), never guessing a different number.
  const positionalFallback = new Map<AgentType, number>();
  for (const participant of participants) {
    const type = agentByWireName.get(participant.agent_type);
    if (!type) continue;
    const canonical = AGENT_MENTIONS.find(mention => mention.type === type);
    if (!canonical) continue;
    const fallback = (positionalFallback.get(type) ?? 0) + 1;
    positionalFallback.set(type, fallback);
    const ordinal = participant.cli_ordinal ?? fallback;
    const trigger = `${canonical.trigger}-cli${ordinal > 1 ? `-${ordinal}` : ''}`;
    options.push({
      trigger,
      // KT-211 — the menu must never show two identities under the same
      // alias: a joined CLI displays its REAL room alias, not the bare
      // provider trigger it shares with the punctual agent.
      displayTrigger: trigger,
      label: ordinal > 1 ? `${labels.cli} ${ordinal}` : labels.cli,
      type,
      target: {
        kind: 'cli',
        agent_type: type,
        cli_session_id: participant.id,
      },
    });
  }

  return options;
}

function escaped(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function proseOnly(text: string): string {
  // Keep offsets stable so textual target ordering still reflects the original
  // message. Mention examples in code, quotations and Markdown blockquotes are
  // documentation, never a dispatch request. This is deliberately handled at
  // the routing boundary: copied UI labels such as `"@ollama"` must not be able
  // to attach a provider to the discussion merely because their text contains
  // a valid alias.
  return text
    .replace(/```[\s\S]*?(?:```|$)/g, match => ' '.repeat(match.length))
    .replace(/`[^`\n]*`/g, match => ' '.repeat(match.length))
    .replace(/^[ \t]*>[^\n]*(?:\n|$)/gm, match => ' '.repeat(match.length))
    .replace(/"[\s\S]*?(?:"|$)/g, match => ' '.repeat(match.length))
    .replace(/“[\s\S]*?(?:”|$)/g, match => ' '.repeat(match.length))
    .replace(/«[\s\S]*?(?:»|$)/g, match => ' '.repeat(match.length))
    .replace(/(^|[\s([{,:;])'[\s\S]*?(?:'|$)/g, match => ' '.repeat(match.length));
}

export function targetsFromComposerText(
  text: string,
  mentions: ComposerMention[],
): { targets: MessageTarget[]; targetAll: boolean } {
  const prose = proseOnly(text);
  const matches: Array<{ index: number; mention: ComposerMention }> = [];
  for (const mention of mentions) {
    const pattern = new RegExp(
      `(^|[^\\p{L}\\p{N}_-])${escaped(mention.trigger)}(?=$|[^\\p{L}\\p{N}_-])`,
      'giu',
    );
    for (const match of prose.matchAll(pattern)) {
      matches.push({
        index: (match.index ?? 0) + match[1].length,
        mention,
      });
    }
  }
  matches.sort((left, right) => left.index - right.index);

  const targets: MessageTarget[] = [];
  let targetAll = false;
  const seen = new Set<string>();
  for (const { mention } of matches) {
    if (mention.targetAll) {
      targetAll = true;
      continue;
    }
    if (!mention.target) continue;
    const key = [
      mention.target.kind,
      mention.target.agent_type,
      mention.target.cli_session_id ?? '',
    ].join(':');
    if (seen.has(key)) continue;
    seen.add(key);
    targets.push(mention.target);
  }
  return { targets, targetAll };
}

/** Native responders attached to a discussion, in stable reply order.
 * Joined CLI sessions are intentionally excluded: `@all` remains the explicit
 * way to address every native and CLI participant at once. */
export function nativeDiscussionTargets(
  discussion: Pick<Discussion, 'agent' | 'participants'>,
): MessageTarget[] {
  const agents = [discussion.agent, ...discussion.participants];
  return agents
    .filter((agent, index) => agents.indexOf(agent) === index)
    .map(agent => ({
      kind: agent === discussion.agent ? 'discussion_agent' : 'agent',
      agent_type: agent,
      cli_session_id: null,
      tier: agent === discussion.agent ? null : 'default',
    }));
}

export interface PendingAgentReply {
  id: string;
  triggerMessageId: string;
  agent: AgentType;
  status: string;
}

type DiscussionWithActiveDispatches = Discussion & {
  active_agent_dispatches?: ActiveAgentDispatch[];
};

/** Durable reply slots, keyed by dispatch rather than agent type. Two turns
 * can therefore both wait for Ollama without collapsing into one placeholder.
 * The fallback keeps rolling-upgrade compatibility with an older backend. */
export function pendingAgentReplies(
  discussion: DiscussionWithActiveDispatches,
): PendingAgentReply[] {
  if ('active_agent_dispatches' in discussion) {
    return (discussion.active_agent_dispatches ?? []).map(dispatch => ({
      id: dispatch.id,
      triggerMessageId: dispatch.trigger_message_id,
      agent: dispatch.agent_type,
      status: dispatch.status,
    }));
  }

  let latestUserIndex = -1;
  for (let index = discussion.messages.length - 1; index >= 0; index -= 1) {
    const message = discussion.messages[index];
    if (message.role === 'User' && message.channel === 'main') {
      latestUserIndex = index;
      break;
    }
  }
  if (latestUserIndex < 0) return [];

  const latestUser = discussion.messages[latestUserIndex];
  const routingText = proseOnly(latestUser.content);
  let requested = mentionedAgents(routingText);
  if (/(^|[^\p{L}\p{N}_-])@all(?=$|[^\p{L}\p{N}_-])/iu.test(routingText)) {
    requested = [discussion.agent, ...discussion.participants];
  } else if (requested.length === 0) {
    // Participants are historical capabilities, not an implicit recipient
    // list. A general turn always belongs to the configured principal unless
    // the human explicitly names another target (or uses @all).
    requested = [latestUser.target_agent ?? discussion.agent];
  }

  const answered = new Set<AgentType>();
  for (const message of discussion.messages.slice(latestUserIndex + 1)) {
    if ((message.role === 'Agent' || message.role === 'System') && message.agent_type) {
      answered.add(message.agent_type);
    }
  }
  return requested.filter((agent, index) => (
    requested.indexOf(agent) === index && !answered.has(agent)
  )).map(agent => ({
    id: `legacy:${latestUser.id}:${agent}`,
    triggerMessageId: latestUser.id,
    agent,
    status: 'Pending',
  }));
}

/** Display projection for overlapping turns. The database remains strictly
 * append-only, but a late native reply linked to an older User message belongs
 * to that conversational turn and must render before newer questions. */
export function messagesInConversationOrder(
  messages: DiscussionMessage[],
): DiscussionMessage[] {
  const mainUserIds = new Set(
    messages
      .filter(message => message.role === 'User' && message.channel === 'main')
      .map(message => message.id),
  );
  const turnRankByUserId = new Map<string, number>();
  const messageById = new Map(messages.map(message => [message.id, message]));
  const naturalTurnRank: number[] = [];
  let currentTurnRank = -1;

  for (const message of messages) {
    if (message.role === 'User' && message.channel === 'main') {
      currentTurnRank += 1;
      turnRankByUserId.set(message.id, currentTurnRank);
    }
    naturalTurnRank.push(currentTurnRank);
  }

  const linkedTurnRank = (message: DiscussionMessage): number | undefined => {
    if ((message.role !== 'Agent' && message.role !== 'System') || !message.reply_to_message_id) {
      return undefined;
    }
    const visited = new Set<string>();
    let parentId: string | undefined = message.reply_to_message_id;
    while (parentId && !visited.has(parentId) && visited.size < 16) {
      visited.add(parentId);
      if (mainUserIds.has(parentId)) return turnRankByUserId.get(parentId);
      const parent = messageById.get(parentId);
      if (!parent || (parent.role !== 'Agent' && parent.role !== 'System')) return undefined;
      parentId = parent.reply_to_message_id ?? undefined;
    }
    return undefined;
  };

  return messages
    .map((message, index) => {
      const linkedTurn = linkedTurnRank(message);
      return {
        message,
        index,
        turnRank: linkedTurn ?? naturalTurnRank[index],
      };
    })
    .sort((left, right) => left.turnRank - right.turnRank || left.index - right.index)
    .map(entry => entry.message);
}
