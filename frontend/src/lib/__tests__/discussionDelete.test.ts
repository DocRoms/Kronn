// KT-2084 — a refused delete leaves the discussion as it was and gives the
// server's reason word for word; a bulk delete reports each refusal.
import { describe, it, expect, vi } from 'vitest';
import { ApiRequestError } from '../apiRequestError';
import {
  DeletingDiscussions,
  deleteDiscussions,
  deleteRefusalReason,
  describeDeleteRefusals,
  DiscussionsDeleteRefused,
} from '../discussionDelete';

const REFUSAL =
  'this discussion is the room or sub-discussion of unfinished task execution(s) '
  + '0c6f4f6e-3b9a-4c43-9d4e-2f0a6b1c7d11 (KT-12, Running), '
  + '7d1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b (KT-13, Pending): cancel them first';

describe('deleteRefusalReason', () => {
  it('keeps a server refusal whole, however long, and whatever words it holds', () => {
    expect(REFUSAL.length).toBeGreaterThan(200);
    expect(deleteRefusalReason(new ApiRequestError(REFUSAL, 'conflict'))).toBe(REFUSAL);
    const dirty = '2 uncommitted file(s) in /repo/.kronn/worktrees/at home: commit or discard them first';
    expect(deleteRefusalReason(new ApiRequestError(dirty, 'conflict'))).toBe(dirty);
  });

  it('words anything else as other errors are', () => {
    expect(deleteRefusalReason(new Error('Failed to fetch'))).toBe(
      'Impossible de contacter le serveur. Vérifiez votre connexion.',
    );
  });
});

describe('deleteDiscussions', () => {
  it('forgets a discussion only once the server deleted it', async () => {
    let settle: () => void = () => {};
    const remove = vi.fn(() => new Promise<void>(resolve => { settle = resolve; }));
    const onDeleted = vi.fn();

    const pending = deleteDiscussions(['disc-a'], remove, onDeleted);
    await Promise.resolve();
    expect(remove).toHaveBeenCalledWith('disc-a');
    expect(onDeleted).not.toHaveBeenCalled();

    settle();
    expect(await pending).toEqual([]);
    expect(onDeleted).toHaveBeenCalledWith('disc-a');
  });

  it('leaves a refused discussion alone and returns its reason, without hiding the others', async () => {
    const remove = vi.fn((id: string) => (id === 'disc-b'
      ? Promise.reject(new ApiRequestError(REFUSAL, 'conflict'))
      : Promise.resolve()));
    const onDeleted = vi.fn();

    const refused = await deleteDiscussions(['disc-a', 'disc-b', 'disc-c'], remove, onDeleted);

    expect(refused).toEqual([{ id: 'disc-b', reason: REFUSAL }]);
    expect(onDeleted.mock.calls.map(([id]) => id)).toEqual(['disc-a', 'disc-c']);
  });

  it('counts a discussion already gone (a double click) as deleted, not refused', async () => {
    const remove = vi.fn((id: string) => (id === 'disc-gone'
      ? Promise.reject(new ApiRequestError('Discussion not found', 'not_found'))
      : Promise.resolve(null)));
    const onDeleted = vi.fn();

    const refused = await deleteDiscussions(['disc-gone', 'disc-a'], remove, onDeleted);

    expect(refused).toEqual([]);
    expect(onDeleted.mock.calls.map(([id]) => id)).toEqual(['disc-gone', 'disc-a']);
  });

  it('hands on where the server kept a worktree it could not remove', async () => {
    const kept = 'worktree kept at /repo/.kronn/worktrees/p--a: it is also the worktree of discussion(s) disc-b';
    const remove = vi.fn((id: string) => Promise.resolve(id === 'disc-a' ? kept : null));
    const onDeleted = vi.fn();

    await deleteDiscussions(['disc-a', 'disc-c'], remove, onDeleted);

    expect(onDeleted.mock.calls).toEqual([['disc-a', kept], ['disc-c']]);
  });

  it('ends every delete and calls back for every deleted one even when a callback throws', async () => {
    const deleting = new DeletingDiscussions();
    const remove = vi.fn((id: string) => (id === 'disc-c'
      ? Promise.reject(new ApiRequestError(REFUSAL, 'conflict'))
      : Promise.resolve(null)));
    const onDeleted = vi.fn((id: string) => {
      if (id === 'disc-a') throw new Error('the list update failed');
    });
    const streamError = vi.fn();
    const logged = vi.spyOn(console, 'error').mockImplementation(() => {});

    const pending = deleteDiscussions(['disc-a', 'disc-b', 'disc-c'], remove, onDeleted, deleting);
    deleting.onStreamError('disc-c', streamError);
    const refused = await pending;

    expect(refused).toEqual([{ id: 'disc-c', reason: REFUSAL }]);
    expect(onDeleted.mock.calls.map(([id]) => id)).toEqual(['disc-a', 'disc-b']);
    expect(deleting.isBeingDeleted('disc-b')).toBe(true);
    expect(deleting.isBeingDeleted('disc-c')).toBe(false);
    expect(streamError).toHaveBeenCalledTimes(1);
    expect(logged).toHaveBeenCalled();
    logged.mockRestore();
  });

  it('marks the discussions as being deleted until the server answers, then unmarks the refused', async () => {
    const deleting = new DeletingDiscussions();
    const answers: Record<string, { resolve: () => void; reject: (error: unknown) => void }> = {};
    const remove = vi.fn((id: string) => new Promise<null>((resolve, reject) => {
      answers[id] = { resolve: () => resolve(null), reject };
    }));

    const pending = deleteDiscussions(['disc-a', 'disc-b'], remove, vi.fn(), deleting);
    expect(deleting.isBeingDeleted('disc-a')).toBe(true);
    expect(deleting.isBeingDeleted('disc-b')).toBe(true);

    answers['disc-a'].resolve();
    answers['disc-b'].reject(new ApiRequestError(REFUSAL, 'conflict'));
    await pending;
    expect(deleting.isBeingDeleted('disc-a')).toBe(true);
    expect(deleting.isBeingDeleted('disc-b')).toBe(false);
  });
});

describe('DeletingDiscussions', () => {
  /** Deletes of one discussion, each answered by hand in the order they started. */
  const deletesAnsweredByHand = () => {
    const answers: Array<{ resolve: () => void; reject: (error: unknown) => void }> = [];
    const remove = vi.fn(() => new Promise<null>((resolve, reject) => {
      answers.push({ resolve: () => resolve(null), reject });
    }));
    return { answers, remove };
  };
  const refusal = () => new ApiRequestError(REFUSAL, 'conflict');

  it('keeps a discussion marked while another delete of it is pending', async () => {
    const deleting = new DeletingDiscussions();
    const { answers, remove } = deletesAnsweredByHand();
    const first = deleteDiscussions(['disc-a'], remove, vi.fn(), deleting);
    const second = deleteDiscussions(['disc-a'], remove, vi.fn(), deleting);
    const streamError = vi.fn();
    deleting.onStreamError('disc-a', streamError);

    answers[1].reject(refusal());
    await second;
    expect(deleting.isBeingDeleted('disc-a')).toBe(true);
    expect(streamError).not.toHaveBeenCalled();

    answers[0].resolve();
    await first;
    expect(deleting.isBeingDeleted('disc-a')).toBe(true);
    expect(streamError).not.toHaveBeenCalled();
  });

  it('holds a stream error while the delete is pending and hands it back when it is refused', async () => {
    const deleting = new DeletingDiscussions();
    const { answers, remove } = deletesAnsweredByHand();
    const pending = deleteDiscussions(['disc-a'], remove, vi.fn(), deleting);
    const streamError = vi.fn();
    deleting.onStreamError('disc-a', streamError);
    expect(streamError).not.toHaveBeenCalled();

    answers[0].reject(refusal());
    await pending;
    expect(streamError).toHaveBeenCalledTimes(1);
    expect(deleting.isBeingDeleted('disc-a')).toBe(false);

    deleting.onStreamError('disc-a', streamError);
    expect(streamError).toHaveBeenCalledTimes(2);
  });

  it('hands nothing back while another delete of it is still pending, then once all are refused', async () => {
    const deleting = new DeletingDiscussions();
    const { answers, remove } = deletesAnsweredByHand();
    const first = deleteDiscussions(['disc-a'], remove, vi.fn(), deleting);
    const second = deleteDiscussions(['disc-a'], remove, vi.fn(), deleting);
    const streamError = vi.fn();
    deleting.onStreamError('disc-a', streamError);

    answers[0].reject(refusal());
    await first;
    expect(streamError).not.toHaveBeenCalled();
    answers[1].reject(refusal());
    await second;
    expect(streamError).toHaveBeenCalledTimes(1);
  });

  it('drops a held stream error, and any later one, once the discussion is deleted', async () => {
    const deleting = new DeletingDiscussions();
    const { answers, remove } = deletesAnsweredByHand();
    const pending = deleteDiscussions(['disc-a'], remove, vi.fn(), deleting);
    const streamError = vi.fn();
    deleting.onStreamError('disc-a', streamError);

    answers[0].resolve();
    await pending;
    deleting.onStreamError('disc-a', streamError);
    expect(streamError).not.toHaveBeenCalled();
  });

  it('hands back every held stream error even when one of them throws', async () => {
    const deleting = new DeletingDiscussions();
    const { answers, remove } = deletesAnsweredByHand();
    const pending = deleteDiscussions(['disc-a', 'disc-b'], remove, vi.fn(), deleting);
    const failing = vi.fn(() => { throw new Error('stream handler failed'); });
    const sameDiscussion = vi.fn();
    const otherDiscussion = vi.fn();
    deleting.onStreamError('disc-a', failing);
    deleting.onStreamError('disc-a', sameDiscussion);
    deleting.onStreamError('disc-b', otherDiscussion);
    const logged = vi.spyOn(console, 'error').mockImplementation(() => {});

    answers[0].reject(refusal());
    answers[1].reject(refusal());
    const refused = await pending;

    expect(refused.map(({ id }) => id)).toEqual(['disc-a', 'disc-b']);
    expect(failing).toHaveBeenCalledTimes(1);
    expect(sameDiscussion).toHaveBeenCalledTimes(1);
    expect(otherDiscussion).toHaveBeenCalledTimes(1);
    expect(deleting.isBeingDeleted('disc-a')).toBe(false);
    expect(deleting.isBeingDeleted('disc-b')).toBe(false);
    expect(logged).toHaveBeenCalled();
    logged.mockRestore();
  });

  it('ends every delete it began when the remover throws before returning a promise', async () => {
    const deleting = new DeletingDiscussions();
    const remove = vi.fn((id: string) => {
      if (id === 'disc-b') throw new ApiRequestError(REFUSAL, 'conflict');
      return Promise.resolve(null);
    });
    const onDeleted = vi.fn();

    const refused = await deleteDiscussions(['disc-a', 'disc-b', 'disc-c'], remove, onDeleted, deleting);

    expect(refused).toEqual([{ id: 'disc-b', reason: REFUSAL }]);
    expect(onDeleted.mock.calls.map(([id]) => id)).toEqual(['disc-a', 'disc-c']);
    expect(deleting.isBeingDeleted('disc-b')).toBe(false);
  });

  it('handles a stream error at once when no delete is pending', () => {
    const deleting = new DeletingDiscussions();
    const streamError = vi.fn();
    deleting.onStreamError('disc-a', streamError);
    expect(streamError).toHaveBeenCalledTimes(1);
  });
});

describe('describeDeleteRefusals', () => {
  it('names each refused discussion by its title, else its id, with its reason', () => {
    const refused = [
      { id: 'disc-a', reason: 'first reason' },
      { id: 'disc-x', reason: 'second reason' },
    ];
    const titleOf = (id: string) => (id === 'disc-a' ? 'Room A' : undefined);
    expect(describeDeleteRefusals(refused, titleOf)).toBe(
      'Room A: first reason\ndisc-x: second reason',
    );
    expect(new DiscussionsDeleteRefused(refused).refused).toBe(refused);
  });
});
