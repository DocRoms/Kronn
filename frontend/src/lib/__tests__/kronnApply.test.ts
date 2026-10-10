import { describe, expect, it } from 'vitest';
import { parseApplyBlocks, parseKronnApply } from '../kronnApply';
// Verbatim Claude Code reply from KT-1110 (marker in its own fence): the config holds no secret values.
import realMessage from './fixtures/kronn-apply-4b62875d.txt?raw';

const FENCE = '```';

describe('parseKronnApply — accepted variants', () => {
  it('bare marker followed by a json fence (documented format)', () => {
    const text = `Voici :\n\nKRONN:APPLY\n${FENCE}json\n{ "endpoint": "/x" }\n${FENCE}\nVoilà.`;
    const { blocks, prose, unreadable } = parseKronnApply(text);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toEqual({ signature: '{ "endpoint": "/x" }', parsed: { endpoint: '/x' }, applied: false });
    expect(prose).toBe('Voici :\n\n\nVoilà.');
    expect(unreadable).toBeNull();
  });

  it('marker in its own fence, then a json fence', () => {
    const text = `Intro\n${FENCE}json\nKRONN:APPLY\n${FENCE}\n${FENCE}json\n{ "name": "A" }\n${FENCE}\nOutro`;
    const { blocks, prose, unreadable } = parseKronnApply(text);
    expect(blocks.map(b => b.parsed)).toEqual([{ name: 'A' }]);
    expect(prose).toBe('Intro\n\nOutro');
    expect(unreadable).toBeNull();
  });

  it('marker as the first line inside the json fence', () => {
    const text = `Intro\n${FENCE}json\nKRONN:APPLY\n{ "name": "B" }\n${FENCE}\nOutro`;
    const { blocks, prose } = parseKronnApply(text);
    expect(blocks.map(b => b.parsed)).toEqual([{ name: 'B' }]);
    expect(blocks[0].signature).toBe('{ "name": "B" }');
    expect(prose).toBe('Intro\n\nOutro');
  });

  it('any letter case for the marker and the fence language', () => {
    expect(parseApplyBlocks(`kronn:apply\n${FENCE}JSON\n{ "a": 1 }\n${FENCE}`)).toHaveLength(1);
    expect(parseApplyBlocks(`${FENCE}\nKronn:Apply\n${FENCE}\n${FENCE}json\n{ "a": 2 }\n${FENCE}`)[0].parsed).toEqual({ a: 2 });
    expect(parseApplyBlocks(`${FENCE}json\nkRoNn:aPpLy\n{ "a": 3 }\n${FENCE}`)[0].parsed).toEqual({ a: 3 });
  });

  it('the real Claude Code message 4b62875d yields one proposal and clean prose', () => {
    const { blocks, prose, unreadable } = parseKronnApply(realMessage);
    expect(unreadable).toBeNull();
    expect(blocks).toHaveLength(1);
    const parsed = blocks[0].parsed;
    expect(parsed.name).toBe('Insider Eureka Search');
    expect(parsed.base_url).toBe('https://ineureka.api.useinsider.com');
    expect(parsed.endpoints).toHaveLength(2);
    expect(parsed.fields).toEqual([
      { label: 'X-AUTH-TOKEN', value: '' },
      { label: 'Partner ID (p)', value: '' },
      { label: 'Locale (l)', value: '' },
      { label: 'Currency (c)', value: '' },
    ]);
    expect(prose).not.toMatch(/KRONN:APPLY/);
    expect(prose).not.toContain('"base_url"');
    expect(prose).toContain('**Base URL**');
    expect(prose).toContain('**Il manque encore, pour finaliser :**');
  });
});

describe('parseKronnApply — rejections and warnings', () => {
  it('a json fence without a marker is never a proposal', () => {
    const text = `Exemple de réponse :\n${FENCE}json\n{ "items": [] }\n${FENCE}`;
    expect(parseKronnApply(text)).toEqual({ blocks: [], prose: text, unreadable: null });
  });

  it('plain chat has no blocks and no warning', () => {
    expect(parseKronnApply('Just chatting, no suggestion here.')).toEqual({
      blocks: [],
      prose: 'Just chatting, no suggestion here.',
      unreadable: null,
    });
  });

  it('a marker ending a prose sentence before a json fence is not a proposal', () => {
    const text = `Le protocole emploie le mot KRONN:APPLY\n${FENCE}json\n{"endpoint":"/example"}\n${FENCE}`;
    expect(parseKronnApply(text)).toEqual({ blocks: [], prose: text, unreadable: null });
  });

  it('a marker mentioned inside a sentence does not raise the warning', () => {
    const text = 'Je ne propose pas encore de bloc KRONN:APPLY, il me manque la base URL.';
    expect(parseKronnApply(text).unreadable).toBeNull();
  });

  it('a marker with invalid JSON reports the raw body as unreadable', () => {
    const text = `KRONN:APPLY\n${FENCE}json\n{ "endpoint": "/x", }\n${FENCE}`;
    const { blocks, prose, unreadable } = parseKronnApply(text);
    expect(blocks).toEqual([]);
    expect(prose).toBe('');
    expect(unreadable).toBe('{ "endpoint": "/x", }');
  });

  it('a marker with no fence at all reports the text after it', () => {
    const text = 'Voici :\nKRONN:APPLY\n{ "endpoint": "/x" }';
    const { blocks, unreadable } = parseKronnApply(text);
    expect(blocks).toEqual([]);
    expect(unreadable).toBe('KRONN:APPLY\n{ "endpoint": "/x" }');
  });

  it('a JSON array or scalar is not a proposal', () => {
    expect(parseKronnApply(`KRONN:APPLY\n${FENCE}json\n[1, 2]\n${FENCE}`).blocks).toEqual([]);
    expect(parseKronnApply(`KRONN:APPLY\n${FENCE}json\nnull\n${FENCE}`).unreadable).toBe('null');
  });

  it('an unterminated block mid-stream does not throw', () => {
    const text = `KRONN:APPLY\n${FENCE}json\n{ "endpoint": "/x", "query": {`;
    expect(() => parseKronnApply(text)).not.toThrow();
    expect(parseKronnApply(text).blocks).toEqual([]);
  });
});

describe('parseKronnApply — multiple blocks', () => {
  it('keeps every block, whatever its variant, and strips them all from the prose', () => {
    const text = [
      'option A:',
      'KRONN:APPLY',
      `${FENCE}json`, '{ "endpoint": "/a" }', FENCE,
      'option B:',
      `${FENCE}json`, 'KRONN:APPLY', FENCE,
      `${FENCE}json`, '{ "endpoint": "/b" }', FENCE,
      'option C:',
      `${FENCE}json`, 'KRONN:APPLY', '{ "endpoint": "/c" }', FENCE,
      'fin',
    ].join('\n');
    const { blocks, prose, unreadable } = parseKronnApply(text);
    expect(blocks.map(b => b.parsed)).toEqual([{ endpoint: '/a' }, { endpoint: '/b' }, { endpoint: '/c' }]);
    expect(prose).toBe('option A:\n\noption B:\n\noption C:\n\nfin');
    expect(unreadable).toBeNull();
  });

  it('a bare marker after an unrelated code block is still found', () => {
    const text = `Commande :\n${FENCE}bash\nls\n${FENCE}\nKRONN:APPLY\n${FENCE}json\n{ "a": 1 }\n${FENCE}`;
    const { blocks, prose } = parseKronnApply(text);
    expect(blocks.map(b => b.parsed)).toEqual([{ a: 1 }]);
    expect(prose).toBe(`Commande :\n${FENCE}bash\nls\n${FENCE}`);
  });

  it('one valid block among broken ones shows the card and keeps the broken JSON for the notice', () => {
    const text = `KRONN:APPLY\n${FENCE}json\n{ broken\n${FENCE}\nKRONN:APPLY\n${FENCE}json\n{ "ok": true }\n${FENCE}`;
    const { blocks, unreadable } = parseKronnApply(text);
    expect(blocks.map(b => b.parsed)).toEqual([{ ok: true }]);
    expect(unreadable).toBe('{ broken');
  });

  it('a malformed proposal next to a valid one is not lost (review case)', () => {
    const block = (body: string) => `KRONN:APPLY\n${FENCE}json\n${body}\n${FENCE}`;
    const malformed = '{"endpoint":"/second",}';
    const parsed = parseKronnApply(`${block('{"endpoint":"/first"}')}\n${block(malformed)}`);
    expect(parsed.blocks.map(b => b.parsed)).toEqual([{ endpoint: '/first' }]);
    expect(`${parsed.prose}\n${parsed.unreadable ?? ''}`).toContain(malformed);
  });
});

describe('parseKronnApply — CRLF line endings', () => {
  const crlf = (text: string) => text.replaceAll('\n', '\r\n');

  it.each([
    ['bare marker', `KRONN:APPLY\n${FENCE}json\n{"endpoint":"/crlf"}\n${FENCE}`],
    ['marker in its own fence', `${FENCE}json\nKRONN:APPLY\n${FENCE}\n${FENCE}json\n{"endpoint":"/crlf"}\n${FENCE}`],
    ['marker inside the json fence', `${FENCE}json\nKRONN:APPLY\n{"endpoint":"/crlf"}\n${FENCE}`],
  ])('accepts the %s variant', (_name, text) => {
    expect(parseKronnApply(crlf(text)).blocks.map(b => b.parsed)).toEqual([{ endpoint: '/crlf' }]);
  });
});
