import type { AgentType, DiscussionMessage, MessageTarget } from '../types/generated';
import { mentionedAgents } from './constants';
import { proseOnly, type PositionedTarget } from './messageTargets';

export interface DelegationProposal {
  orchestrator: MessageTarget;
  delegated: MessageTarget[];
}

const WORD_START = '(?<![\\p{L}\\p{N}_])';
const WORD_END = '(?![\\p{L}\\p{N}_])';

// Imperative delegation verbs only: generic verbs ("fais", "have", "run")
// would turn ordinary multi-agent requests into false proposals.
const DELEGATION_VERBS = [
  // fr
  'lance[rz]?', 'demande[rz]?\\s+(?:à|a|aux?)', 'demande-(?:lui|leur)', 'délègue[rz]?', 'déléguer',
  'confie[rz]?', 'sollicite[rz]?', 'consulte[rz]?', 'interroge[rz]?', 'mobilise[rz]?',
  'coordonne[rz]?', 'orchestre[rz]?', 'fais\\s+(?:\\p{L}+\\s+){1,2}par', 'fai(?:s|re)\\s+appel\\s+(?:à|a)',
  // en
  'ask', 'launch', 'delegate(?:\\s+to)?', 'consult', 'involve', 'coordinate', 'orchestrate',
  // es
  'p[ií]de(?:le|les)?\\s+a', 'pregunta(?:le|les)?\\s+a', 'lanza', 'delega(?:r)?', 'consulta(?:r)?',
  'coordina(?:r)?', 'orquesta(?:r)?', 'haz\\s+que',
];
const DELEGATION_PATTERN = new RegExp(
  `${WORD_START}(?:${DELEGATION_VERBS.join('|')})${WORD_END}|让|叫|派|安排|协调|调度`,
  'iu',
);

function isNative(target: MessageTarget): boolean {
  return target.kind === 'discussion_agent' || target.kind === 'agent';
}

/** Propose orchestration when the first named agent is told to delegate to
 *  the others ("@opencode lance @codex et @claude"). It only offers a choice:
 *  a missed or wrong detection still sends the message in parallel. */
export function detectDelegation(
  text: string,
  positioned: { targets: PositionedTarget[]; targetAll: boolean },
): DelegationProposal | null {
  const { targets, targetAll } = positioned;
  if (targetAll || targets.length < 2) return null;
  // Handoff markers launch native agents only; a joined CLI keeps its own route.
  if (!targets.every(entry => isNative(entry.target))) return null;
  const [first, second] = targets;
  const between = proseOnly(text).slice(first.end, second.start);
  if (!DELEGATION_PATTERN.test(between)) return null;
  return {
    orchestrator: first.target,
    delegated: targets.slice(1).map(entry => entry.target),
  };
}

export type InertMentionReason = 'scheduled' | 'prose';

export interface InertMention {
  agent: AgentType;
  reason: InertMentionReason;
}

const NO_INERT_MENTIONS: InertMention[] = [];

function nativeTargetAgents(targets: readonly MessageTarget[] | undefined): AgentType[] {
  return (targets ?? []).filter(isNative).map(target => target.agent_type);
}

/** Agents a native reply names in prose without launching them, keyed by
 *  message id. "scheduled" means the agent already owns a run on this user
 *  turn; "prose" means the mention carried no handoff marker. */
export function inertAgentMentions(
  messages: readonly DiscussionMessage[],
  messageTargets: Readonly<Record<string, MessageTarget[]>>,
  discussionAgent: AgentType,
): Map<string, InertMention[]> {
  const byId = new Map(messages.map(message => [message.id, message]));
  const rootOf = (message: DiscussionMessage): string | null => {
    let current: DiscussionMessage | undefined = message;
    const visited = new Set<string>();
    while (current && !visited.has(current.id) && visited.size < 32) {
      if (current.role === 'User' && current.channel === 'main') return current.id;
      visited.add(current.id);
      current = current.reply_to_message_id ? byId.get(current.reply_to_message_id) : undefined;
    }
    return null;
  };

  const scheduled = new Map<string, Set<AgentType>>();
  const roots = new Map<string, string | null>();
  for (const message of messages) {
    const root = rootOf(message);
    roots.set(message.id, root);
    if (!root) continue;
    const agents = scheduled.get(root) ?? new Set<AgentType>();
    if (message.id === root) {
      const named = nativeTargetAgents(messageTargets[root]);
      for (const agent of named.length > 0 ? named : [discussionAgent]) agents.add(agent);
    } else if (message.role === 'Agent') {
      if (message.agent_type) agents.add(message.agent_type);
      for (const agent of nativeTargetAgents(messageTargets[message.id])) agents.add(agent);
    }
    scheduled.set(root, agents);
  }

  const result = new Map<string, InertMention[]>();
  for (const message of messages) {
    // Joined CLIs route their own mentions through the bridge.
    if (message.role !== 'Agent' || !message.agent_type || message.author_cli_ordinal != null) continue;
    if (message.channel !== 'main') continue;
    const launched = new Set(nativeTargetAgents(messageTargets[message.id]));
    const root = roots.get(message.id);
    const alreadyScheduled = root ? scheduled.get(root) : undefined;
    const inert = mentionedAgents(proseOnly(message.content))
      .filter(agent => agent !== message.agent_type && !launched.has(agent))
      .map(agent => ({
        agent,
        reason: alreadyScheduled?.has(agent) ? 'scheduled' as const : 'prose' as const,
      }));
    if (inert.length > 0) result.set(message.id, inert);
  }
  return result;
}

export function inertMentionsFor(
  map: Map<string, InertMention[]>,
  messageId: string,
): InertMention[] {
  return map.get(messageId) ?? NO_INERT_MENTIONS;
}

/** Agents a user turn names in prose but did not address: the orchestrated
 *  ones when the human chose to let the single target delegate. */
export function orchestratedAgents(
  message: DiscussionMessage,
  targets: readonly MessageTarget[],
): AgentType[] {
  if (message.role !== 'User' || targets.length !== 1 || !isNative(targets[0])) return [];
  const prose = proseOnly(message.content);
  // An unavailable agent named in passing is not an orchestration.
  if (!DELEGATION_PATTERN.test(prose)) return [];
  const addressed = targets[0].agent_type;
  return mentionedAgents(prose).filter(agent => agent !== addressed);
}
