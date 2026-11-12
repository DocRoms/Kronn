// KT-1693 — the line a room or a run shows when a turn or a step continued on a
// fallback is read back into its parts: from which agent/model, why, until when,
// to which agent/model, over which transport.

import { describe, expect, it } from 'vitest';
import { parseFallbackNotice, splitStepNotices } from '../fallbackNotice';

describe('parseFallbackNotice', () => {
  it('reads from which agent/model, why, until when, to which agent/model and its transport', () => {
    expect(parseFallbackNotice(
      'Codex · gpt-5.6-sol: quota exhausted until 14:00 UTC → continued with Claude Code · sonnet [CLI]',
    )).toEqual({
      fromAgent: 'Codex',
      fromModel: 'gpt-5.6-sol',
      reason: 'quota exhausted',
      until: '14:00 UTC',
      toAgent: 'Claude Code',
      toModel: 'sonnet',
      toTransport: 'cli',
    });
  });

  it('reads the three transports, and a mode instead of a model', () => {
    expect(parseFallbackNotice(
      'Claude Code · eco mode: rate limited → continued with Ollama · qwen3:8b [Local]',
    )).toMatchObject({ reason: 'rate limited', until: null, toAgent: 'Ollama', toModel: 'qwen3:8b', toTransport: 'local' });
    expect(parseFallbackNotice(
      'OpenCode · big-pickle: provider unavailable → continued with NVIDIA · advanced mode [HTTP]',
    )).toMatchObject({ fromAgent: 'OpenCode', toAgent: 'NVIDIA', toModel: 'advanced mode', toTransport: 'http' });
  });

  it('still reads a line written before transports were named, with no transport', () => {
    expect(parseFallbackNotice(
      'Claude Code (economy tier): rate limited -> continued with Codex (economy tier)',
    )).toMatchObject({ reason: 'rate limited', until: null, toAgent: 'Codex', toModel: 'economy tier', toTransport: null });
  });

  it('does not read a stop line or anything else as a switch', () => {
    expect(parseFallbackNotice('Codex: quota exhausted → no fallback (nothing usable)')).toBeNull();
    expect(parseFallbackNotice('Codex: quota exhausted -> no fallback (nothing usable)')).toBeNull();
    expect(parseFallbackNotice('summary cached')).toBeNull();
    expect(parseFallbackNotice('')).toBeNull();
  });
});

describe('splitStepNotices', () => {
  it('splits the quoted notices off the output of a step', () => {
    const output = [
      '> Claude Code · eco mode: rate limited → continued with Codex · eco mode [CLI]',
      '> Codex · eco mode: quota exhausted until 14:00 UTC → continued with Ollama · llama3 [Local]',
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
