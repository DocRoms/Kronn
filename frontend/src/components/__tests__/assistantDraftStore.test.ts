import { describe, it, expect, vi, beforeEach } from 'vitest';

const { updateMock } = vi.hoisted(() => ({ updateMock: vi.fn() }));
vi.mock('../../lib/api', () => ({ assistantConversations: { update: updateMock } }));

import { AssistantDraftStore, pendingFor, savedStepKey, settlePending } from '../assistantConversation';

beforeEach(() => {
  updateMock.mockReset().mockResolvedValue(null);
  localStorage.clear();
});

describe('AssistantDraftStore (KT-1111)', () => {
  it('attaches each conversation with the step it ended up on', async () => {
    const store = new AssistantDraftStore();
    store.track('d1', 'fetch');
    store.track('d2', 'other');
    store.renameStep('fetch', 'fetch news');
    await store.attach('wf-1');
    expect(updateMock).toHaveBeenCalledWith('d1', { target_id: 'wf-1', target_step: 'fetch news' });
    expect(updateMock).toHaveBeenCalledWith('d2', { target_id: 'wf-1', target_step: 'other' });
    expect(store.size).toBe(0);
  });

  it('keeps what failed to attach for the next attempt', async () => {
    const store = new AssistantDraftStore();
    store.track('d1');
    store.track('d2');
    updateMock.mockImplementation(async (id: string) => {
      if (id === 'd2') throw new Error('backend down');
      return null;
    });
    await expect(store.attach('srv-1', 'Svc')).rejects.toThrow(/1 assistant conversation/);
    expect(store.ids()).toEqual(['d2']);

    updateMock.mockReset().mockResolvedValue(null);
    await store.attach('srv-1', 'Svc');
    expect(updateMock).toHaveBeenCalledTimes(1);
    expect(updateMock).toHaveBeenCalledWith('d2', { target_id: 'srv-1', target_label: 'Svc' });
    expect(store.size).toBe(0);
  });

  it('attaches a step under the saved step durable id', async () => {
    const store = new AssistantDraftStore();
    store.track('d1', 'fetch');
    await store.attach('wf-1', undefined, name => (name === 'fetch' ? 'step-uuid-1' : name));
    expect(updateMock).toHaveBeenCalledWith('d1', { target_id: 'wf-1', target_step: 'step-uuid-1' });
  });

  it('keeps a step owed, never filed under its name, when the saved step id is unknown', async () => {
    const store = new AssistantDraftStore();
    store.track('d1', 'fetch');
    await expect(store.attach('wf-1', undefined, () => null)).rejects.toThrow(/no saved id/);
    expect(updateMock).not.toHaveBeenCalled();
    expect(store.ids()).toEqual(['d1']);
    expect(pendingFor('wf-1', ['fetch'])).toEqual(['d1']);
    expect(pendingFor('wf-1', ['other'])).toEqual([]);
  });

  it('records a failed attachment as owed to the target until it succeeds', async () => {
    const store = new AssistantDraftStore();
    store.track('d1');
    updateMock.mockRejectedValueOnce(new Error('down'));
    await expect(store.attach('srv-1')).rejects.toThrow();
    store.clear();
    expect(pendingFor('srv-1')).toEqual(['d1']);
    settlePending('srv-1', 'd1');
    expect(pendingFor('srv-1')).toEqual([]);
  });

  it('a workflow that could not be read back leaves its step conversations owed', async () => {
    const store = new AssistantDraftStore();
    store.track('d1', 'fetch');
    await expect(store.attach('wf-1', undefined, savedStepKey(null))).rejects.toThrow();
    expect(updateMock).not.toHaveBeenCalled();
    expect(pendingFor('wf-1', ['fetch'])).toEqual(['d1']);

    const saved = { steps: [{ name: 'fetch', id: 'step-uuid-1' }], on_failure: [] } as never;
    await store.attach('wf-1', undefined, savedStepKey(saved));
    expect(updateMock).toHaveBeenCalledWith('d1', { target_id: 'wf-1', target_step: 'step-uuid-1' });
    expect(pendingFor('wf-1', ['fetch'])).toEqual([]);
  });

  it('Codex review: a failed attachment with a resolved step UUID survives a later rename', async () => {
    const store = new AssistantDraftStore();
    store.track('d1', 'fetch');
    updateMock.mockRejectedValueOnce(new Error('down'));
    const saved = { steps: [{ name: 'fetch', id: 'step-uuid-1' }], on_failure: [] } as never;
    await expect(store.attach('wf-1', undefined, savedStepKey(saved))).rejects.toThrow();
    store.clear();
    // The saved step is later renamed: its id still finds the owed conversation.
    expect(pendingFor('wf-1', ['renamed', 'step-uuid-1'])).toEqual(['d1']);
  });
});
