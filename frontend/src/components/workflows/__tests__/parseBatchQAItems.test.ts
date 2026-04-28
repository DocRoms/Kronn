// Tests for the QA batch input parser. Rules under test:
//   - 1-var QA → newline / comma / semicolon split, trimmed, deduped
//   - N-var QA → JSON array required
//   - 0-var QA → falls back to 1-var rules (string mode)
// Errors surface as i18n keys so the test stays UI-agnostic.

import { describe, it, expect } from 'vitest';
import { parseBatchQAItems } from '../parseBatchQAItems';

describe('parseBatchQAItems — 1-variable QA (newline mode)', () => {
  it('splits one host per line', () => {
    const out = parseBatchQAItems(
      { varCount: 1 },
      'fr.euronews.com\neuronews.com\nde.euronews.com',
    );
    expect(out).toEqual({ items: ['fr.euronews.com', 'euronews.com', 'de.euronews.com'] });
  });

  it('splits on commas and semicolons too (paste from spreadsheet, ad-hoc lists)', () => {
    const out = parseBatchQAItems({ varCount: 1 }, 'a, b ; c\nd');
    expect(out).toEqual({ items: ['a', 'b', 'c', 'd'] });
  });

  it('trims whitespace around each value', () => {
    const out = parseBatchQAItems({ varCount: 1 }, '  fr.euronews.com  \n  de.euronews.com\n');
    expect(out).toEqual({ items: ['fr.euronews.com', 'de.euronews.com'] });
  });

  it('dedupes while preserving first-seen order (paste with duplicates is common)', () => {
    const out = parseBatchQAItems({ varCount: 1 }, 'a\nb\na\nc\nb');
    expect(out).toEqual({ items: ['a', 'b', 'c'] });
  });

  it('rejects empty input with errorEmpty key', () => {
    expect(parseBatchQAItems({ varCount: 1 }, '')).toEqual({ errorKey: 'qa.batch.errorEmpty' });
    expect(parseBatchQAItems({ varCount: 1 }, '   \n  ')).toEqual({ errorKey: 'qa.batch.errorEmpty' });
  });
});

describe('parseBatchQAItems — multi-variable QA (JSON mode)', () => {
  it('accepts a JSON array of objects', () => {
    const json = '[{"host":"fr.euronews.com","limit":"5"},{"host":"de.euronews.com","limit":"10"}]';
    const out = parseBatchQAItems({ varCount: 2 }, json);
    expect(out).toEqual({
      items: [
        { host: 'fr.euronews.com', limit: '5' },
        { host: 'de.euronews.com', limit: '10' },
      ],
    });
  });

  it('rejects non-array JSON with errorJsonArray', () => {
    const out = parseBatchQAItems({ varCount: 2 }, '{"host":"fr.euronews.com"}');
    expect(out).toEqual({ errorKey: 'qa.batch.errorJsonArray' });
  });

  it('rejects malformed JSON with errorJsonParse + the parse error message', () => {
    const out = parseBatchQAItems({ varCount: 2 }, '[{ unterminated');
    expect(out).toMatchObject({ errorKey: 'qa.batch.errorJsonParse' });
    expect((out as { errorKey: string; errorArg: string }).errorArg).toMatch(/SyntaxError|JSON|Unexpected/i);
  });

  it('rejects empty JSON array with errorEmpty', () => {
    const out = parseBatchQAItems({ varCount: 2 }, '[]');
    expect(out).toEqual({ errorKey: 'qa.batch.errorEmpty' });
  });

  it('does NOT split on newlines in multi-var mode (we expect strict JSON)', () => {
    // Pasting newline-separated values when 2 vars is a user error — we
    // surface the JSON parse error instead of silently trying to be
    // helpful (which would produce wrong objects).
    const out = parseBatchQAItems({ varCount: 2 }, 'fr.euronews.com\nde.euronews.com');
    expect(out).toMatchObject({ errorKey: 'qa.batch.errorJsonParse' });
  });
});

describe('parseBatchQAItems — zero-variable QA falls back to 1-var rules', () => {
  it('treats 0 vars like 1 var (string mode), each line is a no-template call', () => {
    // A QA with no variables = a static call. Re-running it batch-style
    // is unusual but legal (e.g. pinging the same endpoint N times for a
    // load test). Each line is a placeholder item; the executor falls
    // back to `{{batch.item}}` per child if the body needs it.
    const out = parseBatchQAItems({ varCount: 0 }, 'ping1\nping2');
    expect(out).toEqual({ items: ['ping1', 'ping2'] });
  });
});
