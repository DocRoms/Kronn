import { describe, expect, it } from 'vitest';
import { stripAcpToolMarkers } from '../acpToolMarkers';

describe('stripAcpToolMarkers', () => {
  it('gives back the sentence the agent actually wrote', () => {
    // Verbatim from the reported message: the markers are welded between two
    // sentences, with no newline of their own.
    const { content, tools } = stripAcpToolMarkers(
      "Je vais essayer d'accéder à ce lien pour vérifier." +
        '[ClaudeCode tool: ToolSearch][ClaudeCode tool: WebFetch]' +
        'Réponse courte : **non, pas au contenu réel.**',
    );

    expect(content).toBe(
      "Je vais essayer d'accéder à ce lien pour vérifier." +
        'Réponse courte : **non, pas au contenu réel.**',
    );
    expect(tools).toEqual([
      { name: 'ToolSearch', count: 1 },
      { name: 'WebFetch', count: 1 },
    ]);
  });

  it('counts a run of identical calls instead of listing it five times', () => {
    const { tools } = stripAcpToolMarkers(
      '[ClaudeCode tool: ToolSearch]'.repeat(5) + 'Voilà.',
    );
    expect(tools).toEqual([{ name: 'ToolSearch', count: 5 }]);
  });

  it('never touches prose that merely talks about tools', () => {
    // Two real messages on the reporting instance say this. A looser pattern
    // would eat a chunk of a legitimate reply.
    const prose = '/PLANIFIK_WEB (using tool: read, max depth: 1, max entries: 40)';
    expect(stripAcpToolMarkers(prose).content).toBe(prose);
    expect(stripAcpToolMarkers(prose).tools).toEqual([]);
  });

  it('leaves a message without markers exactly as it was', () => {
    const clean = 'Une réponse normale, avec **du gras** et un [lien](http://x).';
    const result = stripAcpToolMarkers(clean);
    expect(result.content).toBe(clean);
    expect(result.tools).toEqual([]);
  });

  it('is not fooled by a bracket that spans lines or never closes', () => {
    const tricky = '[ClaudeCode tool: unterminated\nstill the message';
    expect(stripAcpToolMarkers(tricky).content).toBe(tricky);
  });

  it('does not leave a blank hole where a marker had its own line', () => {
    const { content } = stripAcpToolMarkers(
      'Avant.\n\n[OpenCode tool: bash]\n\nAprès.',
    );
    expect(content).toBe('Avant.\n\nAprès.');
  });

  it('is stateless across calls', () => {
    // The regex is module-level and global; a forgotten lastIndex would make
    // every other call miss its first marker.
    const input = 'a[Codex tool: read]b';
    expect(stripAcpToolMarkers(input).tools).toHaveLength(1);
    expect(stripAcpToolMarkers(input).tools).toHaveLength(1);
    expect(stripAcpToolMarkers(input).tools).toHaveLength(1);
  });
});
