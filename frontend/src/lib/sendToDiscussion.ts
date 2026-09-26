import { discussions as discussionsApi } from './api';
import { userError } from './userError';
import type { MessageChannel, MessageTarget } from '../types/generated';

/** Sends to one existing discussion through the durable outbox route: the
 *  message and its dispatch are persisted, then the server runs the agent, so
 *  closing the mosaic never cuts a reply. The tile shows it via the monitor. */
export function sendToDiscussion(
  discussionId: string,
  content: string,
  targets: MessageTarget[] | undefined,
  targetAll: boolean,
  replyToMessageId: string | undefined,
  channel: MessageChannel,
  /** Stable across retries so the server can deduplicate a lost receipt. */
  clientMessageId: string = crypto.randomUUID(),
): Promise<void> {
  return new Promise((resolve, reject) => {
    let accepted = false;
    const refuse = (error: string) => { if (!accepted) reject(new Error(userError(error))); };
    void discussionsApi.sendMessageStream(
      discussionId,
      {
        content,
        channel,
        targets: targets ?? [],
        target_all: targetAll,
        target_agents: (targets ?? []).map(target => target.agent_type),
        target_agent: targets?.[0]?.agent_type,
        client_message_id: clientMessageId,
        defer_dispatch: true,
        reply_to_message_id: replyToMessageId,
      },
      () => undefined,
      () => refuse('Message stream closed before persistence receipt'),
      refuse,
      undefined,
      undefined,
      undefined,
      () => { accepted = true; resolve(); },
    ).catch(error => refuse(error instanceof Error ? error.message : String(error)));
  });
}
