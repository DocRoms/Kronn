export const MESSAGE_SEND_SETTLED_EVENT = 'kronn:message-send-settled';

export type MessageSendSettlement = 'accepted' | 'refused';

export interface MessageSendSettledDetail {
  discussionId: string;
  message: string;
  settlement: MessageSendSettlement;
}

export function publishMessageSendSettled(
  discussionId: string,
  message: string,
  settlement: MessageSendSettlement,
): void {
  window.dispatchEvent(new CustomEvent<MessageSendSettledDetail>(
    MESSAGE_SEND_SETTLED_EVENT,
    { detail: { discussionId, message, settlement } },
  ));
}
