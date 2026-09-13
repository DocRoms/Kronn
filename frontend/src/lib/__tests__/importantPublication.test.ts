// KT-619 — what a send carries, and what it must not carry.
//
// A reviewer's point, and a fair one: a green end-to-end run proves the
// nominal path. It proves nothing about a Stop landing inside the proof
// round-trip, because nobody presses Stop in a nominal run.

import { describe, it, expect, vi } from 'vitest';
import { preparePublication } from '../importantPublication';

const FENCE = ['Point.', '', '```kronn-important', '{"version":1}', '```'].join('\n');
const GRANT = 'kr-human-abcdef';
const DISC = 'd-1';

function live(): AbortSignal {
  return new AbortController().signal;
}

describe('preparePublication', () => {
  it('asks for nothing when the message carries no card', async () => {
    const issueProof = vi.fn();
    const result = await preparePublication({
      grant: GRANT,
      content: 'Bonjour, rien de structuré ici.',
      discussionId: DISC,
      signal: live(),
      issueProof,
      onRefused: vi.fn(),
    });

    expect(result).toEqual({ outcome: 'send', fields: {} });
    // An ordinary message must not cost a round-trip, and must not spend a
    // proof it has no use for.
    expect(issueProof).not.toHaveBeenCalled();
  });

  it('asks for nothing when nobody presented a credential', async () => {
    const issueProof = vi.fn();
    const result = await preparePublication({
      grant: '   ',
      content: FENCE,
      discussionId: DISC,
      signal: live(),
      issueProof,
      onRefused: vi.fn(),
    });

    // The fence still travels: the message posts, the card does not, and the
    // bubble underneath says so. That is the fail-closed state, not an error.
    expect(result).toEqual({ outcome: 'send', fields: {} });
    expect(issueProof).not.toHaveBeenCalled();
  });

  it('binds the proof to this body and this room, and carries both', async () => {
    const issueProof = vi.fn().mockResolvedValue('proof-1');
    const result = await preparePublication({
      grant: `  ${GRANT}  `,
      content: FENCE,
      discussionId: DISC,
      signal: live(),
      issueProof,
      onRefused: vi.fn(),
    });

    // The EXACT body, untouched: a proof issued for a trimmed or reformatted
    // version would be spent on something the server never saw.
    expect(issueProof).toHaveBeenCalledWith(GRANT, DISC, FENCE);
    expect(result).toEqual({
      outcome: 'send',
      fields: { publication_grant: GRANT, publication_proof: 'proof-1' },
    });
  });

  it('loses the card and keeps the message when the proof is refused', async () => {
    const onRefused = vi.fn();
    const result = await preparePublication({
      grant: GRANT,
      content: FENCE,
      discussionId: DISC,
      signal: live(),
      issueProof: vi.fn().mockRejectedValue(new Error('forbidden')),
      onRefused,
    });

    // Throwing away text somebody wrote because a publication right failed is
    // the wrong trade.
    expect(result).toEqual({ outcome: 'send', fields: {} });
    expect(onRefused).toHaveBeenCalledTimes(1);
  });

  it('abandons the send when Stop lands while the proof is in flight', async () => {
    const controller = new AbortController();
    let release: (value: string) => void = () => {};
    const issueProof = vi.fn(
      () => new Promise<string>((resolve) => { release = resolve; }),
    );

    const pending = preparePublication({
      grant: GRANT,
      content: FENCE,
      discussionId: DISC,
      signal: controller.signal,
      issueProof,
      onRefused: vi.fn(),
    });

    // The user stops DURING the round-trip. The proof then arrives, and the
    // message must not go out behind them.
    controller.abort();
    release('proof-too-late');

    await expect(pending).resolves.toEqual({ outcome: 'abandon' });
  });

  it('abandons after a refused proof too, when Stop landed during it', async () => {
    const controller = new AbortController();
    let reject: (reason: unknown) => void = () => {};
    const issueProof = vi.fn(
      () => new Promise<string>((_resolve, rejectIt) => { reject = rejectIt; }),
    );

    const pending = preparePublication({
      grant: GRANT,
      content: FENCE,
      discussionId: DISC,
      signal: controller.signal,
      issueProof,
      onRefused: vi.fn(),
    });

    controller.abort();
    reject(new Error('forbidden'));

    // The failure path has the same race as the success path, and an earlier
    // shape of this code only checked one of them.
    await expect(pending).resolves.toEqual({ outcome: 'abandon' });
  });

  it('does not spend a proof on a send that was already stopped', async () => {
    const controller = new AbortController();
    controller.abort();
    const issueProof = vi.fn();

    const result = await preparePublication({
      grant: GRANT,
      content: FENCE,
      discussionId: DISC,
      signal: controller.signal,
      issueProof,
      onRefused: vi.fn(),
    });

    expect(result).toEqual({ outcome: 'abandon' });
    expect(issueProof).not.toHaveBeenCalled();
  });

  it('issues one proof per send, even when two sends overlap', async () => {
    const issueProof = vi.fn()
      .mockResolvedValueOnce('proof-a')
      .mockResolvedValueOnce('proof-b');

    const [first, second] = await Promise.all([
      preparePublication({
        grant: GRANT,
        content: FENCE,
        discussionId: DISC,
        signal: live(),
        issueProof,
        onRefused: vi.fn(),
      }),
      preparePublication({
        grant: GRANT,
        content: `${FENCE}\n`,
        discussionId: DISC,
        signal: live(),
        issueProof,
        onRefused: vi.fn(),
      }),
    ]);

    // Each send carries its OWN proof. Sharing one would mean the second send
    // spends a proof issued for a body it is not sending.
    expect(issueProof).toHaveBeenCalledTimes(2);
    expect(first).toMatchObject({ fields: { publication_proof: 'proof-a' } });
    expect(second).toMatchObject({ fields: { publication_proof: 'proof-b' } });
  });
});
