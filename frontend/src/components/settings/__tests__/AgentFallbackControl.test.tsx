// KT-1518 — the fallback chain as a timeline: add, change, reorder, stop at a
// point, cut it; a loop, a chain past its limit and the gaps of a link are
// flagged inline, a refused chain shows the backend's reason, and a discussion
// or a step can override or cancel the agent's default chain.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { I18nProvider } from '../../../lib/I18nContext';
import { AgentFallbackControl } from '../AgentFallbackControl';
import { FallbackOverride } from '../FallbackOverride';
import { chainProblem } from '../../../lib/fallbackChain';
import type { AgentFallback, ChainView, Cooldown } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock({
  config: {
    getAgentFallback: vi.fn().mockResolvedValue({
      chains: [{
        agent: 'ClaudeCode',
        links: [{ agent: 'Codex', paid: false, allow_paid: false, gaps: [] }],
      }],
      cooldowns: [],
    }) as never,
  },
  agents: {
    detect: vi.fn().mockResolvedValue([
      { agent_type: 'Codex', installed: true },
      { agent_type: 'GeminiCli', installed: true },
      { agent_type: 'Ollama', installed: true },
    ]) as never,
  },
}));

afterEach(cleanup);

const link = (agent: AgentFallback['agent'], extra: Partial<AgentFallback> = {}): AgentFallback => ({
  agent, model: null, allow_paid: false, ...extra,
});

const mount = async (props: Partial<React.ComponentProps<typeof AgentFallbackControl>> = {}) => {
  const onChange = props.onChange ?? vi.fn().mockResolvedValue(undefined);
  await act(async () => {
    render(
      <I18nProvider>
        <AgentFallbackControl
          agentType="ClaudeCode"
          current={[]}
          candidates={['Codex', 'Ollama', 'LiteLlm', 'GeminiCli']}
          onChange={onChange}
          {...props}
        />
      </I18nProvider>,
    );
  });
  return onChange as ReturnType<typeof vi.fn>;
};

const T = 'agent-fallback-timeline-ClaudeCode';

describe('the fallback timeline of an agent', () => {
  it('starts with no fallback and adds entries one after the other', async () => {
    const onChange = await mount();
    expect(screen.getByTestId(`${T}-origin`).textContent).toBe('Claude Code');
    await act(async () => { fireEvent.change(screen.getByTestId(`${T}-add`), { target: { value: 'Codex' } }); });
    expect(onChange).toHaveBeenLastCalledWith([link('Codex')]);
    cleanup();

    const next = await mount({ current: [link('Codex')] });
    await act(async () => { fireEvent.change(screen.getByTestId(`${T}-add`), { target: { value: 'Ollama' } }); });
    expect(next).toHaveBeenLastCalledWith([link('Codex'), link('Ollama')]);
  });

  it('reads left to right and an entry can be moved earlier or later', async () => {
    const onChange = await mount({ current: [link('Codex'), link('Ollama'), link('GeminiCli')] });
    const track = screen.getByRole('list');
    expect(Array.from(track.querySelectorAll('select')).map((s) => (s as HTMLSelectElement).value).filter(Boolean))
      .toEqual(['Codex', 'Ollama', 'GeminiCli']);

    await act(async () => { fireEvent.click(screen.getByTestId(`${T}-later-0`)); });
    expect(onChange).toHaveBeenLastCalledWith([link('Ollama'), link('Codex'), link('GeminiCli')]);
    await act(async () => { fireEvent.click(screen.getByTestId(`${T}-earlier-2`)); });
    expect(onChange).toHaveBeenLastCalledWith([link('Codex'), link('GeminiCli'), link('Ollama')]);
    expect((screen.getByTestId(`${T}-earlier-0`) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId(`${T}-later-2`) as HTMLButtonElement).disabled).toBe(true);
  });

  it('changes an entry, removes one, stops the chain at a point and cuts it entirely', async () => {
    const onChange = await mount({ current: [link('Codex'), link('Ollama'), link('GeminiCli')] });
    await act(async () => { fireEvent.change(screen.getByTestId(`${T}-agent-1`), { target: { value: 'LiteLlm' } }); });
    expect(onChange).toHaveBeenLastCalledWith([link('Codex'), link('LiteLlm'), link('GeminiCli')]);
    await act(async () => { fireEvent.click(screen.getByTestId(`${T}-remove-1`)); });
    expect(onChange).toHaveBeenLastCalledWith([link('Codex'), link('GeminiCli')]);
    await act(async () => { fireEvent.click(screen.getByTestId(`${T}-stop-0`)); });
    expect(onChange).toHaveBeenLastCalledWith([link('Codex')]);
    await act(async () => { fireEvent.click(screen.getByTestId(`${T}-cut`)); });
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it('stores an explicit model on blur, trimmed, and clears it back to the same tier', async () => {
    const onChange = await mount({ current: [link('Codex')] });
    const input = screen.getByTestId(`${T}-model-0`);
    fireEvent.change(input, { target: { value: '  gpt-5.6-sol ' } });
    await act(async () => { fireEvent.blur(input); });
    expect(onChange).toHaveBeenLastCalledWith([link('Codex', { model: 'gpt-5.6-sol' })]);
    cleanup();

    const again = await mount({ current: [link('Codex', { model: 'gpt-5.6-sol' })] });
    const field = screen.getByTestId(`${T}-model-0`);
    fireEvent.change(field, { target: { value: '' } });
    await act(async () => { fireEvent.blur(field); });
    expect(again).toHaveBeenLastCalledWith([link('Codex')]);
  });

  it('asks for the owner opt-in only on a fallback billed by API key', async () => {
    await mount({ current: [link('Codex')] });
    expect(screen.queryByTestId(`${T}-paid-0`)).toBeNull();
    cleanup();

    const onChange = await mount({ current: [link('LiteLlm')] });
    const optIn = screen.getByTestId(`${T}-paid-0`) as HTMLInputElement;
    expect(optIn.checked).toBe(false);
    await act(async () => { fireEvent.click(optIn); });
    expect(onChange).toHaveBeenLastCalledWith([link('LiteLlm', { allow_paid: true })]);
  });

  it('offers neither the agent itself nor an agent already in the chain', async () => {
    await mount({ current: [link('Codex')], candidates: ['ClaudeCode', 'Codex', 'Ollama'] });
    const add = screen.getByTestId(`${T}-add`) as HTMLSelectElement;
    expect(Array.from(add.querySelectorAll('option')).map((o) => o.value)).toEqual(['', 'Ollama']);
  });

  it('flags a loop, a chain past the limit and a self fallback inline, and sends nothing', () => {
    expect(chainProblem('ClaudeCode', [link('Codex'), link('Codex')])?.key).toBe('fallback.loop');
    expect(chainProblem('ClaudeCode', [link('Codex'), link('ClaudeCode')])?.key).toBe('fallback.self');
    expect(chainProblem('ClaudeCode', [
      link('Codex'), link('Ollama'), link('GeminiCli'), link('Kiro'), link('Vibe'),
    ])?.key).toBe('fallback.tooDeep');
    expect(chainProblem('ClaudeCode', [link('Codex'), link('Ollama')])).toBeNull();
  });

  it('shows the reason the backend refused a chain, nothing being stored', async () => {
    const onChange = vi.fn().mockRejectedValue(new Error('Codex has full access and Claude Code does not.'));
    await mount({ onChange });
    await act(async () => { fireEvent.change(screen.getByTestId(`${T}-add`), { target: { value: 'Codex' } }); });
    const refused = screen.getByTestId(`${T}-refused`);
    expect(refused.textContent).toContain('Codex has full access');
  });

  it('shows what each link may lack and the cooldown of the agent', async () => {
    const chain: ChainView = {
      agent: 'ClaudeCode',
      links: [{ agent: 'Ollama', paid: false, allow_paid: false, gaps: ['no MCP tools', 'capped context window'] }],
    };
    const cooldown: Cooldown = { agent: 'ClaudeCode', until: '2026-11-10T14:00:00Z', reason: 'quota_exhausted' };
    await mount({ current: [link('Ollama')], chain, cooldown });
    expect(screen.getByRole('note').textContent).toContain('no MCP tools, capped context window');
    expect(screen.getByTestId('agent-fallback-cooldown-ClaudeCode').textContent).toContain('quota exhausted');
  });
});

describe('the override of a discussion or a step', () => {
  const mountOverride = async (value: AgentFallback[] | null, onChange = vi.fn().mockResolvedValue(undefined)) => {
    await act(async () => {
      render(
        <I18nProvider>
          <FallbackOverride testId="ov" origin="ClaudeCode" value={value} onChange={onChange} />
        </I18nProvider>,
      );
    });
    return onChange;
  };

  it('uses the agent default chain when there is no override, and says which', async () => {
    await mountOverride(null);
    expect((screen.getByTestId('ov-mode-default') as HTMLInputElement).checked).toBe(true);
    expect(screen.getByTestId('ov-default').textContent).toContain('Claude Code → Codex');
  });

  it('cancels the fallback with an empty override, and goes back to the default with null', async () => {
    const onChange = await mountOverride(null);
    await act(async () => { fireEvent.click(screen.getByTestId('ov-mode-none')); });
    expect(onChange).toHaveBeenLastCalledWith([]);
    cleanup();

    const back = await mountOverride([]);
    expect((screen.getByTestId('ov-mode-none') as HTMLInputElement).checked).toBe(true);
    await act(async () => { fireEvent.click(screen.getByTestId('ov-mode-default')); });
    expect(back).toHaveBeenLastCalledWith(null);
  });

  it('gives the discussion or the step its own chain, starting from the default one', async () => {
    const onChange = await mountOverride(null);
    await act(async () => { fireEvent.click(screen.getByTestId('ov-mode-custom')); });
    expect(onChange).toHaveBeenLastCalledWith([link('Codex')]);
    cleanup();

    const own = await mountOverride([link('GeminiCli')]);
    expect((screen.getByTestId('ov-mode-custom') as HTMLInputElement).checked).toBe(true);
    await act(async () => { fireEvent.change(screen.getByTestId('ov-timeline-add'), { target: { value: 'Ollama' } }); });
    expect(own).toHaveBeenLastCalledWith([link('GeminiCli'), link('Ollama')]);
  });
});
