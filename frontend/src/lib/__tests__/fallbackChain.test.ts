// KT-1693 — the rules of a fallback, mirrored from the backend so the owner sees
// a refusal before anything is sent: same kind of mode, no loop, bounded depth.

import { describe, expect, it } from 'vitest';
import {
  chainProblem,
  inheritedAfter,
  modeKind,
  modeOfTier,
  optionsFor,
  toLink,
  transportOf,
  type FallbackOption,
} from '../fallbackChain';
import { buildFallbackOptions, modelOfMode, modesOf } from '../fallbackOptions';
import type { AgentFallback, ChainView, ModelTiersConfig } from '../../types/generated';

const link = (agent: AgentFallback['agent'], extra: Partial<AgentFallback> = {}): AgentFallback => ({
  agent, model: null, ...extra,
});

describe('chainProblem', () => {
  it('flags a loop, a chain past the limit, a self fallback and a fallback of another kind', () => {
    expect(chainProblem('ClaudeCode', 'standard', [link('Codex'), link('Codex')])?.key).toBe('fallback.loop');
    expect(chainProblem('ClaudeCode', 'standard', [link('Codex'), link('ClaudeCode')])?.key).toBe('fallback.self');
    expect(chainProblem('ClaudeCode', 'standard', [
      link('Codex'), link('Ollama'), link('GeminiCli'), link('Kiro'), link('Vibe'),
    ])?.key).toBe('fallback.tooDeep');
    expect(chainProblem('ClaudeCode', 'standard', [link('Codex', { mode: 'image' })])?.key).toBe('fallback.wrongKind');
    expect(chainProblem('ClaudeCode', 'image', [link('Codex', { mode: 'standard' })])?.key).toBe('fallback.wrongKind');
  });

  it('accepts text to any text mode, and image, video or decision to their own kind', () => {
    for (const mode of ['eco', 'standard', 'advanced'] as const) {
      for (const target of ['eco', 'standard', 'advanced'] as const) {
        expect(chainProblem('ClaudeCode', mode, [link('Codex', { mode: target })])).toBeNull();
      }
    }
    for (const mode of ['image', 'video', 'decision'] as const) {
      expect(chainProblem('ClaudeCode', mode, [link('Codex', { mode })])).toBeNull();
    }
  });
});

describe('modes and transports', () => {
  it('maps a tier to its mode, and a mode to its kind', () => {
    expect(['economy', 'default', 'reasoning', undefined].map((tier) => modeOfTier(tier as never)))
      .toEqual(['eco', 'standard', 'advanced', 'standard']);
    expect(['eco', 'standard', 'advanced', 'decision', 'image', 'video'].map((mode) => modeKind(mode as never)))
      .toEqual(['text', 'text', 'text', 'decision', 'image', 'video']);
  });

  it('names how each agent is reached: CLI, HTTP or Local', () => {
    expect(transportOf('ClaudeCode')).toBe('cli');
    expect(transportOf('OpenCode')).toBe('cli');
    expect(transportOf('LiteLlm')).toBe('http');
    expect(transportOf('Nvidia')).toBe('http');
    expect(transportOf('Ollama')).toBe('local');
  });

  it('states the mode of a link only when it is not the one of its row', () => {
    expect(toLink({ agent: 'Codex', model: 'gpt', mode: 'standard' }, 'standard')).toEqual({ agent: 'Codex', model: 'gpt' });
    expect(toLink({ agent: 'Codex', model: 'gpt', mode: 'eco' }, 'standard')).toEqual({ agent: 'Codex', model: 'gpt', mode: 'eco' });
  });
});

describe('what a fallback can be', () => {
  const tiers = {
    claude_code: { economy: 'haiku', default: 'sonnet', reasoning: 'opus' },
    open_code: { default: 'big-pickle' },
    ollama: { default: 'qwen3', decision: 'clef-flash' },
  } as unknown as ModelTiersConfig;
  const agents = [
    { agent_type: 'ClaudeCode', installed: true, runtime_available: false },
    { agent_type: 'OpenCode', installed: true, runtime_available: false },
    { agent_type: 'Ollama', installed: true, runtime_available: false },
    { agent_type: 'Codex', installed: false, runtime_available: false },
    { agent_type: 'Custom', installed: true, runtime_available: false },
  ] as never;

  it('offers every installed agent, OpenCode included, never one that is not installed or a named connection', () => {
    const options = buildFallbackOptions(agents, tiers);
    const offered = new Set(options.map((option) => option.agent));
    expect([...offered].sort()).toEqual(['ClaudeCode', 'Ollama', 'OpenCode']);
    expect(options).toContainEqual({ agent: 'OpenCode', model: 'big-pickle', mode: 'standard' });
  });

  it('gives each agent the modes it has: the three text modes, and decision where a decision model is set', () => {
    expect(modesOf('ClaudeCode', tiers).map((entry) => entry.mode)).toEqual(['eco', 'standard', 'advanced']);
    expect(modesOf('Ollama', tiers)).toContainEqual({ mode: 'decision', model: 'clef-flash' });
    expect(modelOfMode('ClaudeCode', 'decision', tiers)).toBeUndefined();
    expect(modelOfMode('OpenCode', 'eco', tiers)).toBeNull();
  });

  it('keeps the kind of the row, never the agent the row belongs to, and a model-less choice on its own mode', () => {
    const all: FallbackOption[] = buildFallbackOptions(agents, tiers);
    const text = optionsFor(all, 'ClaudeCode', 'eco');
    expect(text.every((option) => option.agent !== 'ClaudeCode' && modeKind(option.mode) === 'text')).toBe(true);
    expect(text).toContainEqual({ agent: 'OpenCode', model: null, mode: 'eco' });
    expect(text).toContainEqual({ agent: 'OpenCode', model: 'big-pickle', mode: 'standard' });
    expect(text).not.toContainEqual({ agent: 'OpenCode', model: null, mode: 'advanced' });
    expect(optionsFor(all, 'ClaudeCode', 'decision')).toEqual([{ agent: 'Ollama', model: 'clef-flash', mode: 'decision' }]);
    expect(optionsFor(all, 'ClaudeCode', 'image')).toEqual([]);
  });
});

describe('inheritedAfter', () => {
  const row = (agent: ChainView['agent'], mode: ChainView['mode'], to: ChainView['agent'], toMode = mode): ChainView => ({
    agent,
    mode,
    links: [{ agent: to, mode: toMode, transport: transportOf(to), gaps: [] }],
    inherited: [],
  });

  it('follows the rows of the fallbacks, in the mode the previous link runs in', () => {
    const chains = [row('Codex', 'standard', 'Kiro'), row('Kiro', 'standard', 'Vibe'), row('Codex', 'eco', 'Ollama')];
    expect(inheritedAfter(chains, 'ClaudeCode', 'standard', [link('Codex')]).map((entry) => [entry.source, entry.link.agent]))
      .toEqual([['Codex', 'Kiro'], ['Kiro', 'Vibe']]);
    expect(inheritedAfter(chains, 'ClaudeCode', 'standard', [link('Codex', { mode: 'eco' })]).map((entry) => entry.link.agent))
      .toEqual(['Ollama']);
  });

  it('stops at an agent already on the walk and at the depth limit', () => {
    const loop = [row('Codex', 'standard', 'ClaudeCode')];
    expect(inheritedAfter(loop, 'ClaudeCode', 'standard', [link('Codex')])).toEqual([]);
    expect(inheritedAfter([], 'ClaudeCode', 'standard', [link('Codex')])).toEqual([]);
    const long = [
      row('Codex', 'standard', 'Kiro'), row('Kiro', 'standard', 'Vibe'),
      row('Vibe', 'standard', 'CopilotCli'), row('CopilotCli', 'standard', 'GeminiCli'),
    ];
    expect(inheritedAfter(long, 'ClaudeCode', 'standard', [link('Codex')])).toHaveLength(3);
  });
});
