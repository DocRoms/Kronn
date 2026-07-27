import type {
  AgentType,
  MessageTarget,
  ParticipantView,
} from '../types/generated';
import { AGENT_MENTIONS } from './constants';

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
      },
    });
  }

  const ordinalByAgent = new Map<AgentType, number>();
  for (const participant of participants) {
    const type = agentByWireName.get(participant.agent_type);
    if (!type) continue;
    const canonical = AGENT_MENTIONS.find(mention => mention.type === type);
    if (!canonical) continue;
    const ordinal = (ordinalByAgent.get(type) ?? 0) + 1;
    ordinalByAgent.set(type, ordinal);
    options.push({
      trigger: `${canonical.trigger}-cli${ordinal > 1 ? `-${ordinal}` : ''}`,
      displayTrigger: canonical.trigger,
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
  // message. Mention examples in inline/fenced code are documentation, never a
  // dispatch request.
  return text
    .replace(/```[\s\S]*?(?:```|$)/g, match => ' '.repeat(match.length))
    .replace(/`[^`\n]*`/g, match => ' '.repeat(match.length));
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
