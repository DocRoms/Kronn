import { describe, expect, it } from 'vitest';
import mermaid from 'mermaid';
import { sanitizeMermaidSource } from '../mermaidSanitize';

describe('sanitizeMermaidSource with the real Mermaid parser', () => {
  it('turns natural clock periods into a valid timeline', async () => {
    const source = [
      'timeline',
      '    title Incident du 2026-09-01 (heure de Paris)',
      '    19:00 : Bruit de fond normal (7 à 76 erreurs Apollo / 5 min)',
      '    19:30 - 19:45 : Vague 1 — 538 puis 764 erreurs Apollo/5min (pic)',
      '    20:50 - 20:55 : Extinction quasi totale des erreurs Apollo',
    ].join('\n');

    await expect(mermaid.parse(sanitizeMermaidSource(source))).resolves.toMatchObject({
      diagramType: 'timeline',
    });
  });
});
