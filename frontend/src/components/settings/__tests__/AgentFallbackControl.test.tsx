// KT-1693 / KT-1757 — the fallback of each mode of an agent, as proposal A draws it: one row
// per mode, the current model, « If unavailable: », then ONE numbered column of fallbacks
// (transport badge, tier tag, ↑ ↓ ✕), the inherited ones dashed after the own one;
// « + Add a fallback » opens a menu filtered by kind of mode and grouped by agent. The same
// row serves the override of a discussion or a step.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { I18nProvider } from '../../../lib/I18nContext';
import { AgentFallbackControl } from '../AgentFallbackControl';
import { FallbackOverride } from '../FallbackOverride';
import type { FallbackOption } from '../../../lib/fallbackChain';
import type { AgentFallback, ChainView, Cooldown, ModelTiersConfig } from '../../../types/generated';

const { CODEX_STANDARD, tiers } = vi.hoisted(() => ({
  CODEX_STANDARD: {
    agent: 'ClaudeCode',
    mode: 'standard',
    links: [{ agent: 'Codex', model: 'gpt-5.6', mode: 'standard', transport: 'cli', gaps: [] }],
    inherited: [{
      source: 'Codex',
      link: { agent: 'Ollama', model: 'qwen3', mode: 'standard', transport: 'local', gaps: [] },
    }],
  } as unknown as ChainView,
  tiers: {
    claude_code: { economy: 'haiku', default: 'sonnet', reasoning: 'opus' },
    open_code: { default: 'big-pickle' },
    ollama: { default: 'qwen3' },
  } as unknown as ModelTiersConfig,
}));

const view = (overrides: Partial<ChainView> & Pick<ChainView, 'agent' | 'mode'>): ChainView => ({
  links: [],
  inherited: [],
  ...overrides,
});

vi.mock('../../../lib/api', () => buildApiMock({
  config: {
    getAgentFallback: vi.fn().mockResolvedValue({ chains: [CODEX_STANDARD], cooldowns: [] }) as never,
    getModelTiers: vi.fn().mockResolvedValue(tiers) as never,
  },
  agents: {
    detect: vi.fn().mockResolvedValue([
      { agent_type: 'ClaudeCode', installed: true, runtime_available: false },
      { agent_type: 'Codex', installed: true, runtime_available: false },
      { agent_type: 'OpenCode', installed: true, runtime_available: false },
      { agent_type: 'Ollama', installed: true, runtime_available: false },
    ]) as never,
  },
  modelCatalogApi: {
    list: vi.fn().mockResolvedValue({ targets: [] }) as never,
  },
}));

afterEach(cleanup);

const option = (agent: FallbackOption['agent'], model: string | null, mode: FallbackOption['mode']): FallbackOption => ({
  agent, model, mode,
});

const OPTIONS: FallbackOption[] = [
  option('ClaudeCode', 'sonnet', 'standard'),
  option('Codex', 'gpt-5.6', 'standard'),
  option('Codex', 'gpt-5.6-mini', 'eco'),
  option('OpenCode', 'big-pickle', 'standard'),
  option('OpenCode', null, 'standard'),
  option('OpenCode', null, 'advanced'),
  option('Ollama', 'qwen3', 'standard'),
  option('Ollama', 'clef-flash', 'decision'),
  option('LiteLlm', 'flux', 'image'),
  option('LiteLlm', 'wan', 'video'),
];

const mountControl = async (props: Partial<React.ComponentProps<typeof AgentFallbackControl>> = {}) => {
  const onChange = props.onChange ?? vi.fn().mockResolvedValue(undefined);
  await act(async () => {
    render(
      <I18nProvider>
        <AgentFallbackControl
          agentType="ClaudeCode"
          modes={[
            { mode: 'eco', model: 'haiku' },
            { mode: 'standard', model: 'sonnet' },
            { mode: 'advanced', model: 'opus' },
          ]}
          options={OPTIONS}
          chains={[]}
          onChange={onChange}
          {...props}
        />
      </I18nProvider>,
    );
  });
  return onChange as ReturnType<typeof vi.fn>;
};

const ROW = (mode: string) => `agent-fallback-ClaudeCode-${mode}`;

describe('the fallback rows of an agent card', () => {
  it('shows one row per mode the agent has, off by default, each with its current model', async () => {
    await mountControl();
    for (const [mode, model] of [['eco', 'haiku'], ['standard', 'sonnet'], ['advanced', 'opus']]) {
      const row = screen.getByTestId(ROW(mode));
      expect(within(row).getByTestId(`${ROW(mode)}-current`).textContent).toContain(model);
      expect(within(row).getByTestId(`${ROW(mode)}-none`).textContent).toBe('Aucun repli : Kronn s’arrête si ce modèle est indisponible');
      expect(within(row).getByTestId(`${ROW(mode)}-add`)).toBeTruthy();
    }
    expect(screen.queryByTestId(ROW('image'))).toBeNull();
  });

  it('draws the current model, the fallback with its transport badge and the inherited one dashed', async () => {
    await mountControl({ chains: [CODEX_STANDARD] });
    const row = screen.getByTestId(ROW('standard'));
    const own = within(row).getByTestId(`${ROW('standard')}-link-0`);
    expect(own.textContent).toContain('Codex');
    expect(own.textContent).toContain('gpt-5.6');
    expect(within(own).getByTestId('fallback-transport-cli').textContent).toBe('CLI');
    const inherited = within(row).getByTestId(`${ROW('standard')}-inherited-0`);
    expect(inherited.getAttribute('data-tone')).toBe('inherited');
    expect(within(inherited).getByTestId('fallback-transport-local').textContent).toBe('Local');
    expect(inherited.querySelector('.fbc-main')?.getAttribute('title')).toContain('Défini sur la carte de Codex');
    expect(row.textContent).not.toMatch(/paid/i);
  });

  it('opens the card that holds an inherited fallback when its pill is clicked', async () => {
    const onOpenAgent = vi.fn();
    await mountControl({ chains: [CODEX_STANDARD], onOpenAgent });
    await act(async () => {
      fireEvent.click(within(screen.getByTestId(`${ROW('standard')}-inherited-0`)).getByRole('button'));
    });
    expect(onOpenAgent).toHaveBeenCalledWith('Codex');
  });

  it('adds a fallback from a menu grouped by agent, with each model, its mode and its transport', async () => {
    const onChange = await mountControl();
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('standard')}-add`)); });
    const menu = screen.getByTestId(`${ROW('standard')}-menu`);
    const groups = within(menu).getAllByRole('group').map((group) => group.getAttribute('aria-label'));
    expect(groups).toEqual(['Codex', 'OpenCode', 'Ollama']);
    const codex = within(menu).getByTestId(`${ROW('standard')}-menu-Codex-standard-gpt-5.6`);
    expect(codex.textContent).toContain('gpt-5.6');
    expect(within(codex).getByTestId('fallback-transport-cli')).toBeTruthy();
    // A text row also offers the other text modes of an agent, never another kind.
    expect(within(menu).getByTestId(`${ROW('standard')}-menu-Codex-eco-gpt-5.6-mini`)).toBeTruthy();
    expect(within(menu).queryByText('flux')).toBeNull();
    expect(within(menu).queryByText('clef-flash')).toBeNull();

    await act(async () => { fireEvent.click(codex); });
    expect(onChange).toHaveBeenLastCalledWith('standard', { agent: 'Codex', model: 'gpt-5.6' });
  });

  it('says the mode of the model when it is not the mode of the row', async () => {
    const onChange = await mountControl();
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('standard')}-add`)); });
    await act(async () => {
      fireEvent.click(screen.getByTestId(`${ROW('standard')}-menu-Codex-eco-gpt-5.6-mini`));
    });
    expect(onChange).toHaveBeenLastCalledWith('standard', { agent: 'Codex', model: 'gpt-5.6-mini', mode: 'eco' });
  });

  it('offers a model-less choice on the row\'s own mode only', async () => {
    await mountControl();
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('standard')}-add`)); });
    const menu = screen.getByTestId(`${ROW('standard')}-menu`);
    expect(within(menu).getByTestId(`${ROW('standard')}-menu-OpenCode-standard`)).toBeTruthy();
    expect(within(menu).queryByTestId(`${ROW('standard')}-menu-OpenCode-advanced`)).toBeNull();
  });

  it('never offers the agent itself and disables the agents already in the chain', async () => {
    await mountControl({ chains: [CODEX_STANDARD] });
    await act(async () => { fireEvent.click(within(screen.getByTestId(`${ROW('standard')}-link-0`)).getAllByRole('button')[0]); });
    const menu = screen.getByTestId(`${ROW('standard')}-menu`);
    expect(within(menu).queryByRole('group', { name: 'Claude Code' })).toBeNull();
    // Codex is the one being changed (marked), Ollama is already in the chain as the inherited link.
    const ollama = within(menu).getByTestId(`${ROW('standard')}-menu-Ollama-standard-qwen3`) as HTMLButtonElement;
    expect(ollama.disabled).toBe(true);
    const current = within(menu).getByTestId(`${ROW('standard')}-menu-Codex-standard-gpt-5.6`);
    expect(current.getAttribute('aria-current')).toBe('true');
    expect((within(menu).getByTestId(`${ROW('standard')}-menu-OpenCode-standard-big-pickle`) as HTMLButtonElement).disabled).toBe(false);
  });

  it('changes a fallback by clicking its pill and removes it with its cross', async () => {
    const onChange = await mountControl({ chains: [CODEX_STANDARD] });
    await act(async () => { fireEvent.click(within(screen.getByTestId(`${ROW('standard')}-link-0`)).getAllByRole('button')[0]); });
    await act(async () => {
      fireEvent.click(screen.getByTestId(`${ROW('standard')}-menu-OpenCode-standard-big-pickle`));
    });
    expect(onChange).toHaveBeenLastCalledWith('standard', { agent: 'OpenCode', model: 'big-pickle' });
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('standard')}-link-0-remove`)); });
    expect(onChange).toHaveBeenLastCalledWith('standard', null);
  });

  it('gives every installed agent its rows, OpenCode included, and offers it as a target', async () => {
    await act(async () => {
      render(
        <I18nProvider>
          <AgentFallbackControl
            agentType="OpenCode"
            modes={[{ mode: 'standard', model: null }]}
            options={OPTIONS}
            chains={[]}
            onChange={vi.fn().mockResolvedValue(undefined)}
          />
        </I18nProvider>,
      );
    });
    const row = screen.getByTestId('agent-fallback-OpenCode-standard');
    await act(async () => { fireEvent.click(within(row).getByTestId('agent-fallback-OpenCode-standard-add')); });
    const menu = screen.getByTestId('agent-fallback-OpenCode-standard-menu');
    expect(within(menu).queryByRole('group', { name: 'OpenCode' })).toBeNull();
    expect(within(menu).getByRole('group', { name: 'Claude Code' })).toBeTruthy();
  });

  it('keeps the image, video and decision rows to their own kind', async () => {
    await mountControl({ modes: [{ mode: 'image', model: 'flux' }, { mode: 'decision', model: 'clef-flash' }] });
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('image')}-add`)); });
    const image = screen.getByTestId(`${ROW('image')}-menu`);
    expect(within(image).getAllByRole('menuitem').map((item) => item.textContent)).toEqual([expect.stringContaining('flux')]);
    cleanup();

    await mountControl({ modes: [{ mode: 'decision', model: 'clef-flash' }], options: [...OPTIONS, option('Nvidia', 'judge', 'decision')] });
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('decision')}-add`)); });
    const decision = screen.getByTestId(`${ROW('decision')}-menu`);
    expect(within(decision).getAllByRole('menuitem').map((item) => item.textContent).join(' ')).toContain('judge');
    expect(within(decision).queryByText('qwen3')).toBeNull();
  });

  it('shows the reason the backend refused a fallback, nothing being stored', async () => {
    const onChange = vi.fn().mockRejectedValue(new Error('Codex has full access and Claude Code does not.'));
    await mountControl({ onChange });
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('standard')}-add`)); });
    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW('standard')}-menu-Codex-standard-gpt-5.6`)); });
    expect(screen.getByTestId(`${ROW('standard')}-refused`).textContent).toContain('Codex has full access');
  });

  it('shows what a fallback may lack and the cooldown of the agent', async () => {
    const chain = view({
      agent: 'ClaudeCode',
      mode: 'standard',
      links: [{ agent: 'Ollama', model: 'qwen3', mode: 'standard', transport: 'local', gaps: ['no MCP tools', 'capped context window'] }],
    });
    const cooldown: Cooldown = { agent: 'ClaudeCode', until: '2026-11-10T14:00:00Z', reason: 'quota_exhausted' };
    await mountControl({ chains: [chain], cooldown });
    const pill = screen.getByTestId(`${ROW('standard')}-link-0`);
    expect(pill.querySelector('.fbp-gap')?.getAttribute('title')).toContain('no MCP tools, capped context window');
    expect(screen.getByTestId('agent-fallback-cooldown-ClaudeCode').textContent).toContain('quota exhausted');
  });
});

describe('the override of a discussion or a step', () => {
  const link = (agent: AgentFallback['agent'], extra: Partial<AgentFallback> = {}): AgentFallback => ({
    agent, model: null, ...extra,
  });
  const mountOverride = async (value: AgentFallback[] | null, onChange = vi.fn().mockResolvedValue(undefined)) => {
    await act(async () => {
      render(
        <I18nProvider>
          <FallbackOverride testId="ov" origin="ClaudeCode" mode="standard" value={value} onChange={onChange} />
        </I18nProvider>,
      );
    });
    return onChange;
  };

  it('shows the default of the agent for this mode when there is no override', async () => {
    await mountOverride(null);
    const defaults = screen.getByTestId('ov-default-chain');
    expect(defaults.textContent).toContain('Codex');
    expect(defaults.textContent).toContain('Ollama');
    expect(screen.queryByTestId('ov-mode-default')).toBeNull();
  });

  it('cuts the fallback here with an empty override, and goes back to the default with null', async () => {
    const onChange = await mountOverride(null);
    await act(async () => { fireEvent.click(screen.getByTestId('ov-mode-none')); });
    expect(onChange).toHaveBeenLastCalledWith([]);
    cleanup();

    const back = await mountOverride([]);
    expect(screen.getByTestId('ov-cancelled')).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByTestId('ov-mode-default')); });
    expect(back).toHaveBeenLastCalledWith(null);
  });

  it('replaces the default with its own fallback from the same menu', async () => {
    const onChange = await mountOverride(null);
    await act(async () => { fireEvent.click(screen.getByTestId('ov-row-add')); });
    await act(async () => { fireEvent.click(screen.getByTestId('ov-row-menu-OpenCode-standard-big-pickle')); });
    expect(onChange).toHaveBeenLastCalledWith([{ agent: 'OpenCode', model: 'big-pickle' }]);
    cleanup();

    const own = await mountOverride([link('OpenCode', { model: 'big-pickle' })]);
    expect(screen.getByTestId('ov-row-link-0').textContent).toContain('OpenCode');
    expect(screen.getByTestId('ov-mode-default')).toBeTruthy();
    // Removing the pill is « no fallback here »: the empty override.
    await act(async () => { fireEvent.click(screen.getByTestId('ov-row-link-0-remove')); });
    expect(own).toHaveBeenLastCalledWith([]);
  });
});
