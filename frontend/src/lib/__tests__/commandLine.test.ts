import { describe, expect, it } from 'vitest';
import { commandLine, quoteWord } from '../commandLine';

describe('commandLine', () => {
  it('leaves plain words bare, placeholders included', () => {
    expect(commandLine('fastly', ['purge', '--key', '{{surrogate_key}}'])).toBe(
      'fastly purge --key {{surrogate_key}}',
    );
  });

  it('keeps argument boundaries: a word holding spaces is quoted', () => {
    expect(commandLine('fastly', ['purge --key', '{{id}}'])).toBe("fastly 'purge --key' {{id}}");
    expect(commandLine('fastly', ['purge', '--key', '{{id}}'])).not.toBe(
      commandLine('fastly', ['purge --key', '{{id}}']),
    );
  });

  it('shows an empty argument and escapes a quote', () => {
    expect(quoteWord('')).toBe("''");
    expect(quoteWord("it's")).toBe("'it'\\''s'");
    expect(commandLine('echo', ['', 'a;b', '$HOME'])).toBe("echo '' 'a;b' '$HOME'");
  });
});
