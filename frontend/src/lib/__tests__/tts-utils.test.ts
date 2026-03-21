import { describe, it, expect } from 'vitest';
import { stripMarkdown, splitSentences } from '../tts-utils';

describe('stripMarkdown', () => {
  it('removes fenced code blocks', () => {
    const md = 'Before.\n```ts\nconst x = 1;\n```\nAfter.';
    const result = stripMarkdown(md);
    expect(result).toContain('Before.');
    expect(result).toContain('After.');
    expect(result).not.toContain('const x');
  });

  it('keeps inline code content but strips backticks', () => {
    expect(stripMarkdown('Use `npm install` to install.')).toBe('Use npm install to install.');
  });

  it('converts headings to sentences with period', () => {
    expect(stripMarkdown('## Hello world')).toBe('Hello world.');
  });

  it('strips bold and italic', () => {
    expect(stripMarkdown('This is **bold** and *italic*.')).toBe('This is bold and italic.');
  });

  it('extracts link text, drops URL', () => {
    expect(stripMarkdown('[Click here](https://example.com) now.')).toBe('Click here now.');
  });

  it('replaces standalone URLs with "lien"', () => {
    const result = stripMarkdown('Visit https://example.com/foo?bar=1 for more.');
    expect(result).toContain('lien');
    expect(result).not.toContain('https://');
  });

  it('keeps filename from file paths', () => {
    const result = stripMarkdown('Edit /src/pages/Dashboard.tsx to fix it.');
    expect(result).toContain('Dashboard.tsx');
    expect(result).not.toContain('/src/pages/');
  });

  it('converts snake_case to spaces', () => {
    const result = stripMarkdown('The function compute_step_checksums is important.');
    expect(result).toContain('compute step checksums');
    expect(result).not.toContain('_');
  });

  it('converts camelCase to spaced words', () => {
    const result = stripMarkdown('Call getDiscussionById to fetch it.');
    expect(result).toContain('get');
    expect(result).toContain('Discussion');
    expect(result).not.toMatch(/getDiscussion/);
  });

  it('converts simple camelCase too', () => {
    const result = stripMarkdown('Use onChange handler.');
    // onChange → "on Change"
    expect(result).toContain('on Change');
  });

  it('removes JSON-like fragments', () => {
    expect(stripMarkdown('Returns { "status": "ok", "count": 3 } as response.')).toBe('Returns as response.');
  });

  it('removes KRONN markers', () => {
    expect(stripMarkdown('Done. KRONN:BRIEFING_COMPLETE')).toBe('Done.');
  });

  it('converts bullet list items to comma-separated phrases', () => {
    const md = '- First item\n- Second item\n- Third item';
    const result = stripMarkdown(md);
    expect(result).toContain('First item,');
    expect(result).toContain('Second item,');
  });

  it('converts numbered list items to period-ended phrases', () => {
    const md = '1. First step\n2. Second step';
    const result = stripMarkdown(md);
    expect(result).toContain('First step.');
    expect(result).toContain('Second step.');
  });

  it('produces natural speech from typical markdown response', () => {
    const md = '## Configuration\n\n- Installer le package\n- Configurer les clés\n\n1. Lancer le serveur\n2. Tester la connexion';
    const result = stripMarkdown(md);
    // Heading becomes sentence
    expect(result).toContain('Configuration.');
    // Bullets have commas, numbered have periods
    expect(result).toContain('Installer le package,');
    expect(result).toContain('Lancer le serveur.');
    // No double punctuation
    expect(result).not.toMatch(/[.,]{2}/);
  });

  it('collapses multiple newlines cleanly', () => {
    const md = 'First paragraph.\n\nSecond paragraph.';
    const result = stripMarkdown(md);
    expect(result).toContain('First paragraph.');
    expect(result).toContain('Second paragraph.');
    expect(result).not.toContain('..');
  });

  it('removes table pipes', () => {
    const result = stripMarkdown('| Col1 | Col2 |');
    expect(result).not.toContain('|');
  });

  it('removes horizontal rules', () => {
    const result = stripMarkdown('Above.\n---\nBelow.');
    expect(result).toContain('Above.');
    expect(result).toContain('Below.');
    expect(result).not.toContain('---');
  });

  it('preserves sentence meaning with tech terms', () => {
    const md = 'La fonction `compute_step_checksums` vérifie les fichiers dans `/src/core/checksums.rs`.';
    const result = stripMarkdown(md);
    // Meaning preserved: function name readable, filename kept
    expect(result).toContain('compute step checksums');
    expect(result).toContain('checksums.rs');
    expect(result).toContain('vérifie');
  });
});

describe('splitSentences', () => {
  it('splits on periods', () => {
    const sentences = splitSentences('Voici la première phrase complète. Voici la deuxième phrase complète. Et enfin la troisième.');
    expect(sentences.length).toBeGreaterThanOrEqual(3);
    expect(sentences[0]).toContain('première');
    expect(sentences[1]).toContain('deuxième');
  });

  it('splits on question marks and exclamation points', () => {
    const sentences = splitSentences('Est-ce que ça fonctionne correctement? Oui ça marche très bien! Parfait merci.');
    expect(sentences.length).toBeGreaterThanOrEqual(3);
  });

  it('splits on colons and semicolons', () => {
    const sentences = splitSentences('Voici le plan détaillé: faire ceci en premier; puis faire cela ensuite.');
    expect(sentences.length).toBeGreaterThanOrEqual(2);
  });

  it('merges very short fragments with previous sentence', () => {
    const sentences = splitSentences('This is a longer sentence. OK.');
    expect(sentences).toHaveLength(1);
    expect(sentences[0]).toContain('OK');
  });

  it('handles single sentence without punctuation', () => {
    const sentences = splitSentences('Just one sentence');
    expect(sentences).toHaveLength(1);
    expect(sentences[0]).toBe('Just one sentence');
  });

  it('filters out very short fragments (< 3 chars)', () => {
    const sentences = splitSentences('Hello world. . . Goodbye.');
    for (const s of sentences) {
      expect(s.length).toBeGreaterThan(2);
    }
  });

  it('returns non-empty array for non-empty input', () => {
    expect(splitSentences('Anything')).toHaveLength(1);
  });
});
