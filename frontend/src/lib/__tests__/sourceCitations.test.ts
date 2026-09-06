import { describe, expect, it } from 'vitest';
import { hasSourceCitation, splitSourceCitations } from '../sourceCitations';

const citations = (text: string) => splitSourceCitations(text)
  .flatMap(part => ('citation' in part ? [part.citation] : []));

const texts = (text: string) => splitSourceCitations(text)
  .flatMap(part => ('text' in part ? [part.text] : []));

describe('splitSourceCitations', () => {
  /// KT-609 — the real shape, straight out of the room Romuald was reading.
  it('reads a file citation down to its name and lines', () => {
    const [file] = citations('Voir [src: file: frontend/src/lib/discussionQuestions.ts:74-121] ici.');
    expect(file.kind).toBe('file');
    expect(file.reference).toBe('frontend/src/lib/discussionQuestions.ts:74-121');
    // The path is what makes the marker unreadable inside a sentence; the name
    // and the lines are what a reader actually uses.
    expect(file.label).toBe('discussionQuestions.ts:74-121');
    // And the exact reference survives, because a citation is evidence.
    expect(file.raw).toBe('file: frontend/src/lib/discussionQuestions.ts:74-121');
  });

  it('keeps the prose around a citation intact', () => {
    expect(texts('Avant [src: commit: abc1234] après.')).toEqual(['Avant ', ' après.']);
  });

  it('shortens a sha to what anyone actually reads', () => {
    const [commit] = citations('[src: commit: 0593f12a91bae2ece513cd1c6aeef7cfde86ef99]');
    expect(commit.label).toBe('0593f12');
    expect(commit.raw).toContain('0593f12a91bae2ece513cd1c6aeef7cfde86ef99');
  });

  it('shows a url by its host', () => {
    const [url] = citations('[src: url: https://learn.chatgpt.com/docs/models]');
    expect(url.label).toBe('learn.chatgpt.com');
  });

  it('does not pretend a malformed url is one', () => {
    const [url] = citations('[src: url: pas-une-url]');
    expect(url.label).toBe('pas-une-url');
  });

  /// Several in a row is the case that pushed Romuald to ask: two or three
  /// markers end to end turn a sentence into notation.
  it('reads a run of citations one by one', () => {
    const found = citations(
      '[src: file: a/b.rs:1] [src: user: 2026-09-06: question-answer:1555d630:0]',
    );
    expect(found).toHaveLength(2);
    expect(found[0].label).toBe('b.rs:1');
    expect(found[1].kind).toBe('user');
  });

  /// A chip made out of ordinary prose would be worse than a marker left
  /// unread, so the pattern stays narrow on purpose.
  it('leaves alone anything that only looks like a marker', () => {
    for (const text of [
      'un tableau src: file: a.rs sans crochets',
      '[source: file: a.rs]',
      '[src: file: a.rs',
      '[src:\nfile: a.rs]',
    ]) {
      expect(citations(text)).toHaveLength(0);
      expect(texts(text)).toEqual([text]);
    }
  });

  it('survives a marker with no kind at all', () => {
    const [bare] = citations('[src: quelque chose]');
    expect(bare.kind).toBe('');
    expect(bare.reference).toBe('quelque chose');
    expect(bare.label).toBe('quelque chose');
  });

  it('returns plain prose untouched, so a caller can skip the work', () => {
    expect(splitSourceCitations('rien à voir')).toEqual([{ text: 'rien à voir' }]);
    expect(hasSourceCitation('rien à voir')).toBe(false);
    expect(hasSourceCitation('avec [src: file: a.rs]')).toBe(true);
    // The regex is global and stateful; asking twice must answer the same.
    expect(hasSourceCitation('avec [src: file: a.rs]')).toBe(true);
  });
});
