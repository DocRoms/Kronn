/**
 * KT-2084 — the server refuses to delete a discussion while a task worker runs in
 * it or it holds uncommitted work, and says why and what to do. A refused delete
 * must leave the discussion as it was: its live stream is cut only once the
 * server has deleted it, and the reason reaches the human word for word.
 */
import { ApiRequestError } from './apiRequestError';
import { userError } from './userError';

/** The server's own words for a refusal; anything else goes through `userError`. */
export function deleteRefusalReason(error: unknown): string {
  return error instanceof ApiRequestError && error.message.trim()
    ? error.message.trim()
    : userError(error);
}

export interface DiscussionDeleteRefusal {
  id: string;
  reason: string;
}

/** Already gone (a double click, another tab): nothing is left to delete. */
function alreadyDeleted(error: unknown): boolean {
  return error instanceof ApiRequestError && error.code === 'not_found';
}

/** Pending deletes are counted because two may overlap; a stream error waits for their
 *  outcome, since the server cancels the agents of a discussion before deleting it. */
export class DeletingDiscussions {
  private readonly pending = new Map<string, number>();
  private readonly deleted = new Set<string>();
  private readonly held = new Map<string, Array<() => void>>();

  isBeingDeleted(discId: string): boolean {
    return this.deleted.has(discId) || (this.pending.get(discId) ?? 0) > 0;
  }

  begin(discId: string): void {
    this.pending.set(discId, (this.pending.get(discId) ?? 0) + 1);
  }

  /** One delete of `discId` answered: `deleted` by the server, or refused. */
  end(discId: string, deleted: boolean): void {
    const left = (this.pending.get(discId) ?? 1) - 1;
    if (left > 0) this.pending.set(discId, left);
    else this.pending.delete(discId);
    if (deleted) {
      this.deleted.add(discId);
      this.held.delete(discId);
      return;
    }
    if (left > 0 || this.deleted.has(discId)) return;
    const handlers = this.held.get(discId) ?? [];
    this.held.delete(discId);
    // One failing handler must not keep the others, or the remaining deletes, from ending.
    handlers.forEach(handle => {
      try {
        handle();
      } catch (error) {
        console.error('[discussion delete] a held stream error handler failed:', error);
      }
    });
  }

  /** Handle a stream error of `discId` now, or once its pending delete is refused; never once deleted. */
  onStreamError(discId: string, handle: () => void): void {
    if (this.deleted.has(discId)) return;
    if ((this.pending.get(discId) ?? 0) > 0) {
      this.held.set(discId, [...(this.held.get(discId) ?? []), handle]);
      return;
    }
    handle();
  }
}

/**
 * Delete each of `discIds` on its own: one refusal never hides that the others are
 * gone. `deleting` marks the ids while the server may be cancelling their agents.
 */
export async function deleteDiscussions(
  discIds: string[],
  remove: (discId: string) => Promise<unknown>,
  onDeleted: (discId: string, keptWorktree?: string) => void,
  deleting?: DeletingDiscussions,
): Promise<DiscussionDeleteRefusal[]> {
  discIds.forEach(id => deleting?.begin(id));
  // Started at once, in order; one that throws before returning a promise is refused, so
  // every `begin` still ends.
  const results = await Promise.allSettled(discIds.map(discId => {
    try {
      return remove(discId);
    } catch (error) {
      return Promise.reject(error);
    }
  }));
  const refused: DiscussionDeleteRefusal[] = [];
  const deleted: Array<[string, string | undefined]> = [];
  results.forEach((result, index) => {
    const id = discIds[index];
    if (result.status === 'fulfilled') {
      deleting?.end(id, true);
      deleted.push([id, typeof result.value === 'string' && result.value ? result.value : undefined]);
    }
    else if (alreadyDeleted(result.reason)) {
      deleting?.end(id, true);
      deleted.push([id, undefined]);
    }
    else {
      deleting?.end(id, false);
      refused.push({ id, reason: deleteRefusalReason(result.reason) });
    }
  });
  // Every delete is ended first: a failing callback never strands the others.
  deleted.forEach(([id, keptWorktree]) => {
    try {
      if (keptWorktree) onDeleted(id, keptWorktree);
      else onDeleted(id);
    } catch (error) {
      console.error('[discussion delete] a deleted-discussion callback failed:', error);
    }
  });
  return refused;
}

/** Thrown by a bulk delete some of whose discussions the server refused. */
export class DiscussionsDeleteRefused extends Error {
  constructor(readonly refused: DiscussionDeleteRefusal[]) {
    super(`${refused.length} discussion(s) not deleted`);
    this.name = 'DiscussionsDeleteRefused';
  }
}

/** One line per refused discussion, named by its title when it is known. */
export function describeDeleteRefusals(
  refused: DiscussionDeleteRefusal[],
  titleOf: (discId: string) => string | undefined,
): string {
  return refused.map(({ id, reason }) => `${titleOf(id) ?? id}: ${reason}`).join('\n');
}
