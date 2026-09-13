import { discussions, publicationCredentials } from './api';
import { preparePublication } from './importantPublication';

export type ImportantPublishOutcome = 'confirmed' | 'text-only' | 'cancelled' | 'uncertain' | 'refused';

export interface ImportantSubmission {
  discussionId: string;
  content: string;
  dedupKey: string;
  clientMessageId: string;
  grant: string;
  signal: AbortSignal;
  reconcileFirst: boolean;
  onSending: () => void;
}

interface PublicationApi {
  send: typeof discussions.sendMessageStream;
  list: typeof discussions.importantMessages;
  proof: typeof publicationCredentials.proof;
}

/** Receipt confirms a message; only the persisted card confirms publication. */
export async function submitImportantMessage(
  request: ImportantSubmission,
  api: PublicationApi = { send: discussions.sendMessageStream, list: discussions.importantMessages, proof: publicationCredentials.proof },
): Promise<ImportantPublishOutcome> {
  const exists = async () => (await api.list(request.discussionId)).items.some(card =>
    card.discussion_id === request.discussionId && card.dedup_key === request.dedupKey,
  );
  if (request.reconcileFirst) {
    try {
      if (await exists()) return 'confirmed';
    } catch { return 'uncertain'; }
  }
  const prepared = await preparePublication({
    grant: request.grant,
    content: request.content,
    discussionId: request.discussionId,
    signal: request.signal,
    issueProof: api.proof,
    onRefused: () => undefined,
  });
  if (prepared.outcome === 'abandon' || request.signal.aborted) return 'cancelled';
  request.onSending();
  try {
    await new Promise<void>((resolve, reject) => {
      let accepted = false;
      const rejectBeforeReceipt = (error: string) => {
        if (!accepted) reject(new Error(error));
      };
      // Once claimed, keep observing the write even if the form closes. A
      // browser abort is not proof that the server did not persist it.
      void api.send(request.discussionId, {
        content: request.content,
        channel: 'main',
        client_message_id: request.clientMessageId,
        defer_dispatch: true,
        targets: [],
        ...prepared.fields,
      }, () => undefined, () => {
        if (!accepted) rejectBeforeReceipt('Missing persistence receipt');
      }, rejectBeforeReceipt, undefined, undefined, undefined, receipt => {
        if (receipt.message_id !== request.clientMessageId) {
          rejectBeforeReceipt('Mismatched persistence receipt');
          return;
        }
        accepted = true;
        resolve();
      }).catch(() => rejectBeforeReceipt('Message transport failed'));
    });
    return await exists() ? 'confirmed' : 'text-only';
  } catch {
    try { if (await exists()) return 'confirmed'; } catch { /* retry keeps the exact identity */ }
    return 'uncertain';
  }
}
