// KT-1757 — the « Models and fallbacks » rows, proposal A: the mode label on the left, and on
// the right ONE column — the model, « If unavailable: », the fallbacks numbered in the order
// Kronn tries them (↑ ↓ ✕ on the ones stored on this row), then « + Add a fallback ». The same
// shape with 0, 1 or n fallbacks, on a CLI, a Local and an HTTP card.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import { AgentFallbackControl } from '../AgentFallbackControl';
import { ConnectionFallbackRow } from '../ConnectionFallbackRow';
import { FallbackRow } from '../FallbackRow';
import { buildMediaOptions } from '../../../lib/mediaFallbackOptions';
import type { FallbackOption } from '../../../lib/fallbackChain';
import type { AgentFallback, ChainView, ConnectionChainView } from '../../../types/generated';

afterEach(cleanup);

const OPTIONS: FallbackOption[] = [
  { agent: 'ClaudeCode', model: 'sonnet', mode: 'standard' },
  { agent: 'Codex', model: 'gpt-5.6', mode: 'standard' },
  { agent: 'OpenCode', model: 'big-pickle', mode: 'standard' },
  { agent: 'Ollama', model: 'qwen3', mode: 'standard' },
  { agent: 'Ollama', model: 'qwen3-big', mode: 'advanced' },
];

const chain = (agent: ChainView['agent'], mode: ChainView['mode'], extra: Partial<ChainView>): ChainView => ({
  agent, mode, links: [], inherited: [], ...extra,
});

const codexThenOllama = (agent: ChainView['agent'], mode: ChainView['mode']) => chain(agent, mode, {
  links: [{ agent: 'Codex', model: 'gpt-5.6', mode, transport: 'cli', gaps: [] }],
  inherited: [{
    source: 'Codex',
    link: { agent: 'Ollama', model: 'qwen3-big', mode: 'advanced', transport: 'local', gaps: ['capped context window'] },
  }],
});

const mountAgent = async (
  agentType: 'ClaudeCode' | 'Ollama' | 'Nvidia',
  chains: ChainView[],
  modes: { mode: 'eco' | 'standard' | 'advanced'; model: string | null }[],
  onChange = vi.fn().mockResolvedValue(undefined),
) => {
  await act(async () => {
    render(
      <I18nProvider>
        <AgentFallbackControl
          agentType={agentType}
          modes={modes}
          pickers={Object.fromEntries(modes.map(({ mode }) => [mode, <span key={mode}>picker-{mode}</span>]))}
          options={OPTIONS}
          chains={chains}
          onChange={onChange}
        />
      </I18nProvider>,
    );
  });
  return onChange;
};

const MODES = [
  { mode: 'eco' as const, model: 'haiku' },
  { mode: 'standard' as const, model: 'sonnet' },
  { mode: 'advanced' as const, model: 'opus' },
];

describe('a mode row is ONE column: model, caption, numbered chain, add', () => {
  it.each([['ClaudeCode', 'CLI'], ['Ollama', 'Local'], ['Nvidia', 'HTTP']] as const)(
    'has the same shape on a %s card (%s)',
    async (agent, _transport) => {
      await mountAgent(agent, [codexThenOllama(agent, 'standard')], MODES);
      for (const { mode } of MODES) {
        const row = screen.getByTestId(`agent-fallback-${agent}-${mode}`);
        const column = row.querySelector('.fbr-track') as HTMLElement;
        // The model picker first, then the caption, then the chain (or its empty text), then the add button.
        const order = Array.from(column.children).map((child) => child.className);
        expect(order[0]).toContain('fbr-model');
        expect(order[1]).toContain('fbr-caption');
        expect(column.querySelector('.fbr-caption')?.textContent).toBe('Si indisponible :');
        expect(order[order.length - 1]).toContain('fbr-foot');
        // One vertical axis: nothing sits to the right of the picker, outside the column.
        expect(row.children).toHaveLength(2);
        expect(row.children[0].className).toContain('fbr-mode');
        expect(within(row).getByTestId(`agent-fallback-${agent}-${mode}-add`)).toBeTruthy();
      }
    },
  );

  it('numbers the fallbacks in the order Kronn tries them: the stored one, then the inherited ones', async () => {
    await mountAgent('ClaudeCode', [codexThenOllama('ClaudeCode', 'standard')], MODES);
    const list = screen.getByTestId('agent-fallback-ClaudeCode-standard-chain');
    const items = within(list).getAllByRole('listitem');
    expect(items).toHaveLength(2);
    expect(items[0].getAttribute('data-testid')).toBe('agent-fallback-ClaudeCode-standard-link-0');
    expect(items[0].querySelector('.fbc-n')?.textContent).toBe('1');
    expect(items[0].textContent).toContain('Codex');
    expect(items[0].textContent).toContain('gpt-5.6');
    expect(within(items[0]).getByTestId('fallback-transport-cli').textContent).toBe('CLI');
    expect(items[1].getAttribute('data-testid')).toBe('agent-fallback-ClaudeCode-standard-inherited-0');
    expect(items[1].querySelector('.fbc-n')?.textContent).toBe('2');
    expect(items[1].textContent).toContain('Ollama');
    expect(items[1].textContent).toContain('qwen3-big');
    // The tier tag and the transport tag of each entry.
    expect(items[0].querySelector('.fbp-mode')?.textContent).toBe('Standard');
    expect(items[1].querySelector('.fbp-mode')?.textContent).toBe('Avancé');
    expect(within(items[1]).getByTestId('fallback-transport-local').textContent).toBe('Local');
    // The picker is the model of the row, above the caption.
    const row = screen.getByTestId('agent-fallback-ClaudeCode-standard');
    expect(within(row).getByTestId('agent-fallback-ClaudeCode-standard-model').textContent).toBe('picker-standard');
  });

  it('shows the empty state with no fallback, and one numbered entry with a single one', async () => {
    const one = chain('ClaudeCode', 'eco', {
      links: [{ agent: 'OpenCode', model: 'big-pickle', mode: 'eco', transport: 'cli', gaps: [] }],
    });
    await mountAgent('ClaudeCode', [one], MODES);
    expect(screen.getByTestId('agent-fallback-ClaudeCode-standard-none').textContent)
      .toBe('Aucun repli : Kronn s’arrête si ce modèle est indisponible');
    expect(screen.queryByTestId('agent-fallback-ClaudeCode-standard-chain')).toBeNull();
    const items = within(screen.getByTestId('agent-fallback-ClaudeCode-eco-chain')).getAllByRole('listitem');
    expect(items).toHaveLength(1);
    expect(items[0].querySelector('.fbc-n')?.textContent).toBe('1');
    expect(screen.queryByTestId('agent-fallback-ClaudeCode-eco-none')).toBeNull();
  });
});

describe('a reachability warning sits on its own fallback', () => {
  it('draws it beside the fallback that lacks something, and on no other', async () => {
    await mountAgent('ClaudeCode', [codexThenOllama('ClaudeCode', 'standard')], MODES);
    const own = screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0');
    const inherited = screen.getByTestId('agent-fallback-ClaudeCode-standard-inherited-0');
    expect(within(own).queryByTestId('agent-fallback-ClaudeCode-standard-link-0-warning')).toBeNull();
    const warning = within(inherited).getByTestId('agent-fallback-ClaudeCode-standard-inherited-0-warning');
    expect(warning.textContent).toContain('capped context window');
    // Beside the fallback's tags, not inside the model picker nor inside the other entry.
    expect(warning.parentElement).toBe(inherited);
    const row = screen.getByTestId('agent-fallback-ClaudeCode-standard');
    expect(row.querySelectorAll('.fbp-gap')).toHaveLength(1);
    expect(within(row).getByTestId('agent-fallback-ClaudeCode-standard-model').querySelector('.fbp-gap')).toBeNull();
  });
});

describe('the chain of a row that stores several fallbacks (a discussion or step override)', () => {
  const link = (agent: AgentFallback['agent'], model: string): AgentFallback => ({ agent, model });
  const mountRow = async (links: AgentFallback[], onChange = vi.fn().mockResolvedValue(undefined)) => {
    await act(async () => {
      render(
        <I18nProvider>
          <FallbackRow testId="row" mode="standard" origin="ClaudeCode" links={links} options={OPTIONS} onChange={onChange} />
        </I18nProvider>,
      );
    });
    return onChange;
  };
  const AB = [link('Codex', 'gpt-5.6'), link('OpenCode', 'big-pickle')];

  it('moves a fallback up and down, and persists the new order', async () => {
    const onChange = await mountRow(AB);
    await act(async () => { fireEvent.click(screen.getByTestId('row-link-1-up')); });
    expect(onChange).toHaveBeenLastCalledWith([AB[1], AB[0]]);
    await act(async () => { fireEvent.click(screen.getByTestId('row-link-0-down')); });
    expect(onChange).toHaveBeenLastCalledWith([AB[1], AB[0]]);
  });

  it('cannot move the first one up nor the last one down', async () => {
    await mountRow(AB);
    expect((screen.getByTestId('row-link-0-up') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('row-link-1-down') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('row-link-0-down') as HTMLButtonElement).disabled).toBe(false);
  });

  it('labels the controls for the keyboard and a screen reader', async () => {
    await mountRow(AB);
    const item = screen.getByTestId('row-link-0');
    expect(within(item).getByRole('button', { name: 'Monter ce repli' })).toBeTruthy();
    expect(within(item).getByRole('button', { name: 'Descendre ce repli' })).toBeTruthy();
    expect(within(item).getByRole('button', { name: 'Retirer ce repli' })).toBeTruthy();
    const up = within(item).getByRole('button', { name: 'Descendre ce repli' });
    up.focus();
    expect(document.activeElement).toBe(up);
  });

  it('removes one fallback and keeps the others in order', async () => {
    const onChange = await mountRow([...AB, link('Ollama', 'qwen3')]);
    await act(async () => { fireEvent.click(screen.getByTestId('row-link-1-remove')); });
    expect(onChange).toHaveBeenLastCalledWith([AB[0], link('Ollama', 'qwen3')]);
  });

  it('adds a fallback at the end of the chain', async () => {
    const onChange = await mountRow([AB[0]]);
    await act(async () => { fireEvent.click(screen.getByTestId('row-add')); });
    await act(async () => { fireEvent.click(screen.getByTestId('row-menu-OpenCode-standard-big-pickle')); });
    expect(onChange).toHaveBeenLastCalledWith([AB[0], link('OpenCode', 'big-pickle')]);
  });

  it('numbers the stored fallbacks, then the inherited ones after them', async () => {
    await act(async () => {
      render(
        <I18nProvider>
          <FallbackRow
            testId="row"
            mode="standard"
            origin="ClaudeCode"
            links={AB}
            inherited={[{ source: 'OpenCode', link: { agent: 'Ollama', model: 'qwen3', mode: 'standard', transport: 'local', gaps: [] } }]}
            options={OPTIONS}
            onChange={vi.fn()}
          />
        </I18nProvider>,
      );
    });
    const numbers = within(screen.getByTestId('row-chain')).getAllByRole('listitem').map((item) => item.querySelector('.fbc-n')?.textContent);
    expect(numbers).toEqual(['1', '2', '3']);
    // An inherited fallback lives on another card: no reorder, no remove here.
    const inherited = screen.getByTestId('row-inherited-0');
    expect(within(inherited).queryByTestId('row-inherited-0-up')).toBeNull();
    expect(within(inherited).queryByTestId('row-inherited-0-remove')).toBeNull();
  });
});

describe('an agent card row stores ONE fallback of its own (the engine keeps one per mode row)', () => {
  it('writes add, change and remove to that row, and offers no second own fallback', async () => {
    const onChange = await mountAgent('ClaudeCode', [codexThenOllama('ClaudeCode', 'standard')], MODES);
    // Add on a row with none: the fallback of that mode row is set.
    await act(async () => { fireEvent.click(screen.getByTestId('agent-fallback-ClaudeCode-eco-add')); });
    await act(async () => { fireEvent.click(screen.getByTestId('agent-fallback-ClaudeCode-eco-menu-OpenCode-standard-big-pickle')); });
    expect(onChange).toHaveBeenLastCalledWith('eco', { agent: 'OpenCode', model: 'big-pickle', mode: 'standard' });
    // Remove on a row that has one: the row is cleared (null).
    await act(async () => { fireEvent.click(screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0-remove')); });
    expect(onChange).toHaveBeenLastCalledWith('standard', null);
    // ↑ ↓ have no neighbour of its own to swap with: disabled, not hidden (same shape on every row).
    expect((screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0-up') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0-down') as HTMLButtonElement).disabled).toBe(true);
    // Its row already holds its own fallback: the next ones are set on that fallback's card.
    const add = screen.getByTestId('agent-fallback-ClaudeCode-standard-add') as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(add.title).toContain('carte du repli');
  });
});

describe('the media rows of ComfyUI and of an HTTP connection use the same chain', () => {
  const connections = [
    { id: 'conn-a', display_name: 'OpenRouter', image_model: 'flux-pro', video_model: 'seedance' },
    { id: 'conn-b', display_name: 'LiteLLM', image_model: 'sdxl', video_model: 'veo' },
  ] as never;
  const mountMedia = async (view: ConnectionChainView | undefined, onChange = vi.fn().mockResolvedValue(undefined)) => {
    await act(async () => {
      render(
        <I18nProvider>
          <ConnectionFallbackRow
            testId="media"
            mode="image"
            owner="conn-x"
            model={<span>picker-image</span>}
            options={buildMediaOptions(connections, null)}
            view={view}
            onChange={onChange}
          />
        </I18nProvider>,
      );
    });
    return onChange;
  };

  it('shows the empty state and a working add', async () => {
    const onChange = await mountMedia(undefined);
    expect(screen.getByTestId('media-none').textContent).toContain('Aucun repli : Kronn s’arrête');
    expect(screen.getByText('Si indisponible :')).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByTestId('media-add')); });
    await act(async () => { fireEvent.click(screen.getByTestId('media-menu-conn-a-image-flux-pro')); });
    expect(onChange).toHaveBeenLastCalledWith({ connection: 'conn-a', model: 'flux-pro' });
  });

  it('numbers the stored fallback, then the inherited one, and removes the stored one', async () => {
    const view: ConnectionChainView = {
      owner: 'conn-x',
      mode: 'image',
      links: [{ connection: 'conn-a', model: 'flux-pro', transport: 'http' }],
      inherited: [{ connection: 'conn-b', model: 'sdxl', transport: 'http' }],
    };
    const onChange = await mountMedia(view);
    const items = within(screen.getByTestId('media-chain')).getAllByRole('listitem');
    expect(items.map((item) => item.querySelector('.fbc-n')?.textContent)).toEqual(['1', '2']);
    expect(items[0].textContent).toContain('OpenRouter');
    expect(items[1].textContent).toContain('LiteLLM');
    expect(within(items[0]).getByTestId('fallback-transport-http').textContent).toBe('HTTP');
    expect(screen.getByTestId('media-model').textContent).toBe('picker-image');
    await act(async () => { fireEvent.click(screen.getByTestId('media-link-0-remove')); });
    expect(onChange).toHaveBeenLastCalledWith(null);
  });
});
