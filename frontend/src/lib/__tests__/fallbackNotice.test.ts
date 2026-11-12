// KT-1518 — the line a room or a run shows when a turn or a step continued on
// the next agent of a fallback chain is read back into its parts.

import { describe, expect, it } from 'vitest';
import { parseFallbackNotice, splitStepNotices } from '../fallbackNotice';

describe('parseFallbackNotice', () => {
  it('reads from which agent/model, why, until when and to which agent/model', () => {
    expect(parseFallbackNotice(
      'Codex (gpt-5.6-sol): quota exhausted until 14:00 UTC -> continued with Claude Code (sonnet)',
    )).toEqual({
      fromAgent: 'Codex',
      fromModel: 'gpt-5.6-sol',
      reason: 'quota exhausted',
      until: '14:00 UTC',
      toAgent: 'Claude Code',
      toModel: 'sonnet',
    });
  });

  it('reads a switch with no announced reset and a tier instead of a model', () => {
    const notice = parseFallbackNotice(
      'Claude Code (economy tier): rate limited -> continued with Codex (economy tier)',
    );
    expect(notice).toMatchObject({ reason: 'rate limited', until: null, toAgent: 'Codex', toModel: 'economy tier' });
  });

  it('does not read a stop line or anything else as a switch', () => {
    expect(parseFallbackNotice('Codex: quota exhausted -> no fallback (nothing usable)')).toBeNull();
    expect(parseFallbackNotice('summary cached')).toBeNull();
    expect(parseFallbackNotice('')).toBeNull();
  });
});

describe('splitStepNotices', () => {
  it('splits the quoted notices off the output of a step', () => {
    const output = [
      '> Claude Code (economy tier): rate limited -> continued with Codex (economy tier)',
      '> Codex (economy tier): quota exhausted until 14:00 UTC -> continued with Ollama (llama3)',
      '',
      'the review',
    ].join('\n');
    const { notices, rest } = splitStepNotices(output);
    expect(notices.map((notice) => notice.toAgent)).toEqual(['Codex', 'Ollama']);
    expect(rest).toBe('the review');
  });

  it('leaves an output with no notice, or a quote that is not one, untouched', () => {
    expect(splitStepNotices('plain output')).toEqual({ notices: [], rest: 'plain output' });
    expect(splitStepNotices('> a quote\nbody')).toEqual({ notices: [], rest: '> a quote\nbody' });
  });
});
