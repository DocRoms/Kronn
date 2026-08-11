import { describe, expect, it } from 'vitest';
import { findAgentMentionQuery } from '../mention-autocomplete';

describe('findAgentMentionQuery', () => {
  it('finds an alias at the beginning or after whitespace', () => {
    expect(findAgentMentionQuery('@co', 3)).toEqual({ query: 'co', start: 0, end: 3 });
    expect(findAgentMentionQuery('Avis de @ClAu', 13)).toEqual({ query: 'clau', start: 8, end: 13 });
  });

  it('uses the caret and ignores email-like text or completed mentions', () => {
    expect(findAgentMentionQuery('avant @codex après', 10)).toEqual({ query: 'cod', start: 6, end: 10 });
    expect(findAgentMentionQuery('hello@example.com', 17)).toBeNull();
    expect(findAgentMentionQuery('@codex ', 7)).toBeNull();
  });

  it('supports unicode text surrounding the alias', () => {
    expect(findAgentMentionQuery('Équipe 👋 @ge', 13)).toEqual({ query: 'ge', start: 10, end: 13 });
  });
});
