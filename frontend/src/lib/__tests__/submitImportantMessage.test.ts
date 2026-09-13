import { describe, expect, it, vi } from 'vitest';
import type { ImportantMessage, ImportantMessageList } from '../../types/generated';
import type { discussions } from '../api';
import { submitImportantMessage, type ImportantSubmission } from '../submitImportantMessage';

const card: ImportantMessage = {
  id: 'card', discussion_id: 'room', message_id: 'message', sort_order: 1, category: 'information',
  schema_version: 1, dedup_key: 'human:fact', title: 'Update', highlight: 'Update',
  context: null, impact: 'Not specified', action_required: { required: false, action: null, owner: null, due: null },
  references: { task_ref: null, dod_id: null, execution_id: null, agent: null, commit: null, artifact: null },
  author_kind: 'human', author_label: 'Human', source_kind: null, source_id: null, created_at: '2026-09-13T00:00:00Z',
};
const list = (items: ImportantMessage[]): ImportantMessageList => ({ items, total: items.length, total_all: items.length });
const request = (controller = new AbortController()): ImportantSubmission => ({
  discussionId: 'room', content: '```kronn-important\n{}\n```', dedupKey: 'human:fact',
  clientMessageId: 'message', grant: 'fixture-grant', signal: controller.signal,
  reconcileFirst: false, onSending: vi.fn(),
});
function api(items = [card]) {
  return {
    proof: vi.fn().mockResolvedValue('fixture-proof'),
    list: vi.fn().mockResolvedValue(list(items)),
    send: vi.fn<typeof discussions.sendMessageStream>(async (_id, _body, _chunk, _done, _error, _signal, _start, _log, accepted) => {
      accepted?.({ message_id: 'message', sort_order: 1, duplicate: false });
    }),
  };
}

describe('submitImportantMessage', () => {
  it('confirms the persisted card, defers agent dispatch and never aborts a claimed write', async () => {
    const boundary = api();
    expect(await submitImportantMessage(request(), boundary)).toBe('confirmed');
    expect(boundary.send.mock.calls[0][1]).toMatchObject({ client_message_id: 'message', defer_dispatch: true, publication_proof: 'fixture-proof' });
    expect(boundary.send.mock.calls[0][5]).toBeUndefined();
  });
  it('does not call an accepted ordinary message a published card', async () => {
    expect(await submitImportantMessage(request(), api([]))).toBe('text-only');
  });
  it('preserves the existing text-only contract when proof is refused', async () => {
    const boundary = api([]);
    boundary.proof.mockRejectedValue(new Error('refused'));
    expect(await submitImportantMessage(request(), boundary)).toBe('text-only');
    expect(boundary.send.mock.calls[0][1].publication_grant).toBeUndefined();
  });
  it('does not write when navigation cancels proof preparation', async () => {
    const controller = new AbortController();
    const boundary = api();
    boundary.proof.mockImplementation(async () => { controller.abort(); return 'proof'; });
    expect(await submitImportantMessage(request(controller), boundary)).toBe('cancelled');
    expect(boundary.send).not.toHaveBeenCalled();
  });
  it('recovers a lost receipt from the card before retrying any write', async () => {
    const boundary = api();
    expect(await submitImportantMessage({ ...request(), reconcileFirst: true }, boundary)).toBe('confirmed');
    expect(boundary.proof).not.toHaveBeenCalled();
    expect(boundary.send).not.toHaveBeenCalled();
  });
  it('keeps a network failure uncertain and retries with the same UUID and exact content', async () => {
    const boundary = api([]);
    boundary.send.mockRejectedValueOnce(new Error('network'));
    const submission = request();
    expect(await submitImportantMessage(submission, boundary)).toBe('uncertain');
    expect(await submitImportantMessage({ ...submission, reconcileFirst: true }, boundary)).toBe('text-only');
    expect(boundary.send.mock.calls[0][1].client_message_id).toBe(boundary.send.mock.calls[1][1].client_message_id);
    expect(boundary.send.mock.calls[0][1].content).toBe(boundary.send.mock.calls[1][1].content);
  });

  it('does not treat another room or another fact as confirmation', async () => {
    const boundary = api([{ ...card, discussion_id: 'other' }, { ...card, dedup_key: 'other-fact' }]);
    expect(await submitImportantMessage(request(), boundary)).toBe('text-only');
  });

  it('does not resend when reconciliation is unavailable', async () => {
    const boundary = api();
    boundary.list.mockRejectedValue(new Error('offline'));
    expect(await submitImportantMessage({ ...request(), reconcileFirst: true }, boundary)).toBe('uncertain');
    expect(boundary.send).not.toHaveBeenCalled();
  });

  it('does not confirm a mismatched receipt', async () => {
    const boundary = api([]);
    boundary.send.mockImplementation(async (_id, _body, _chunk, _done, _error, _signal, _start, _log, accepted) => {
      accepted?.({ message_id: 'another-message', sort_order: 1, duplicate: false });
    });
    expect(await submitImportantMessage(request(), boundary)).toBe('uncertain');
  });

  it('continues to verify persistence when navigation happens after the write is claimed', async () => {
    const boundary = api(), controller = new AbortController();
    const submission = request(controller);
    submission.onSending = () => controller.abort();
    expect(await submitImportantMessage(submission, boundary)).toBe('confirmed');
    expect(boundary.send).toHaveBeenCalledOnce();
    expect(boundary.send.mock.calls[0][5]).toBeUndefined();
  });
});
