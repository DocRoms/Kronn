import { describe, expect, it } from 'vitest';
import { formatFieldValue, parseUnifiedDiff, sideBySide } from '../repositoryResourceDiff';

const DIFF = [
  '--- repository',
  '+++ Kronn',
  '@@ -1,4 +1,4 @@',
  ' name: lint',
  '-command: npm run lint',
  '-timeout: 30',
  '+command: pnpm lint',
  ' end: true',
  '',
].join('\n');

describe('repository resource diff', () => {
  it('parses the backend unified diff without its file headers', () => {
    expect(parseUnifiedDiff(DIFF)).toEqual([
      { kind: 'hunk', text: '@@ -1,4 +1,4 @@' },
      { kind: 'context', text: 'name: lint' },
      { kind: 'removed', text: 'command: npm run lint' },
      { kind: 'removed', text: 'timeout: 30' },
      { kind: 'added', text: 'command: pnpm lint' },
      { kind: 'context', text: 'end: true' },
    ]);
  });

  it('keeps a removed line that itself starts with dashes', () => {
    expect(parseUnifiedDiff('---- rule\n')).toEqual([{ kind: 'removed', text: '--- rule' }]);
  });

  it('puts dropped repository lines opposite the Kronn lines that replace them', () => {
    expect(sideBySide(parseUnifiedDiff(DIFF))).toEqual([
      { kind: 'hunk', left: '@@ -1,4 +1,4 @@', right: '@@ -1,4 +1,4 @@' },
      { kind: 'context', left: 'name: lint', right: 'name: lint' },
      { kind: 'changed', left: 'command: npm run lint', right: 'command: pnpm lint' },
      { kind: 'changed', left: 'timeout: 30', right: null },
      { kind: 'context', left: 'end: true', right: 'end: true' },
    ]);
  });

  it('prints an absent field as a dash and anything else as it is', () => {
    expect(formatFieldValue(undefined)).toBe('—');
    expect(formatFieldValue('daily')).toBe('daily');
    expect(formatFieldValue(30)).toBe('30');
    expect(formatFieldValue(null)).toBe('null');
    expect(formatFieldValue(['a'])).toBe('["a"]');
  });
});
