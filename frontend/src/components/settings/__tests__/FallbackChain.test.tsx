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
    link: { agent: 'Ollama', model: 'qwen3-big', mode: 'advanced', transport: 'local', gaps: ['capped_context'] },
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
        // The add foot closes the column; a row that holds its one own fallback has none (no dead add).
        expect(order[order.length - 1]).toContain(mode === 'standard' ? 'fbc-list' : 'fbr-foot');
        // One vertical axis: nothing sits to the right of the picker, outside the column.
        expect(row.children).toHaveLength(2);
        expect(row.children[0].className).toContain('fbr-mode');
        // The row that holds its one own fallback has no add button (it would be dead): the others do.
        const hasOwn = mode === 'standard';
        expect(within(row).queryByTestId(`agent-fallback-${agent}-${mode}-add`) !== null).toBe(!hasOwn);
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
    expect(warning.textContent).toContain('fenêtre de contexte réduite');
    expect(warning.textContent).not.toContain('capped');
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

  it('draws no ↑ on the first one nor ↓ on the last one: no dead control', async () => {
    await mountRow(AB);
    expect(screen.queryByTestId('row-link-0-up')).toBeNull();
    expect(screen.queryByTestId('row-link-1-down')).toBeNull();
    expect((screen.getByTestId('row-link-0-down') as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByTestId('row-link-1-up') as HTMLButtonElement).disabled).toBe(false);
  });

  it('draws no ↑ ↓ at all on a lone fallback, but keeps its × and the add button', async () => {
    await mountRow([AB[0]]);
    const item = screen.getByTestId('row-link-0');
    expect(within(item).queryByRole('button', { name: 'Monter ce repli' })).toBeNull();
    expect(within(item).queryByRole('button', { name: 'Descendre ce repli' })).toBeNull();
    expect(within(item).getByRole('button', { name: 'Retirer ce repli' })).toBeTruthy();
    expect(screen.getByTestId('row-add')).toBeTruthy();
  });

  it('labels the controls for the keyboard and a screen reader', async () => {
    await mountRow(AB);
    const item = screen.getByTestId('row-link-0');
    const second = screen.getByTestId('row-link-1');
    expect(within(second).getByRole('button', { name: 'Monter ce repli' })).toBeTruthy();
    expect(within(item).getByRole('button', { name: 'Descendre ce repli' })).toBeTruthy();
    expect(within(item).getByRole('button', { name: 'Retirer ce repli' })).toBeTruthy();
    const down = within(item).getByRole('button', { name: 'Descendre ce repli' });
    down.focus();
    expect(document.activeElement).toBe(down);
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
    // ↑ ↓ have no neighbour of its own to swap with: not drawn at all, × stays beside the chip.
    const own = screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0');
    expect(screen.queryByTestId('agent-fallback-ClaudeCode-standard-link-0-up')).toBeNull();
    expect(screen.queryByTestId('agent-fallback-ClaudeCode-standard-link-0-down')).toBeNull();
    const remove = within(own).getByTestId('agent-fallback-ClaudeCode-standard-link-0-remove');
    const anchor = own.querySelector('.fbc-anchor') as HTMLElement;
    expect(remove.closest('.fbc-controls')?.previousElementSibling).toBe(anchor);
    // Its row already holds its own fallback: no add button, a « Changer » action next to the chip instead.
    expect(screen.queryByTestId('agent-fallback-ClaudeCode-standard-add')).toBeNull();
    expect(anchor.contains(within(own).getByTestId('agent-fallback-ClaudeCode-standard-link-0-change'))).toBe(true);
  });

  it('changes the one own fallback from its chip, and from « Changer »', async () => {
    const onChange = await mountAgent('ClaudeCode', [codexThenOllama('ClaudeCode', 'standard')], MODES);
    const own = screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0');
    const triggers = [own.querySelector('.fbc-main') as HTMLElement, within(own).getByTestId('agent-fallback-ClaudeCode-standard-link-0-change')];
    for (const trigger of triggers) {
      await act(async () => { fireEvent.click(trigger); });
      expect(screen.getByTestId('agent-fallback-ClaudeCode-standard-menu')).toBeTruthy();
      await act(async () => { fireEvent.click(screen.getByTestId('agent-fallback-ClaudeCode-standard-menu-OpenCode-standard-big-pickle')); });
      expect(onChange).toHaveBeenLastCalledWith('standard', { agent: 'OpenCode', model: 'big-pickle' });
    }
  });

  it('names the card an inherited fallback comes from, and opens it on click', async () => {
    const onOpenAgent = vi.fn();
    await act(async () => {
      render(
        <I18nProvider>
          <AgentFallbackControl
            agentType="ClaudeCode"
            modes={MODES}
            pickers={{}}
            options={OPTIONS}
            chains={[codexThenOllama('ClaudeCode', 'standard')]}
            onChange={vi.fn()}
            onOpenAgent={onOpenAgent}
          />
        </I18nProvider>,
      );
    });
    const inherited = screen.getByTestId('agent-fallback-ClaudeCode-standard-inherited-0');
    const via = within(inherited).getByTestId('agent-fallback-ClaudeCode-standard-inherited-0-via');
    expect(via.textContent).toBe('via Codex');
    expect(via.title).toBe('Défini sur la carte Codex : clique pour l’ouvrir');
    // Next to its number: right after it, before the chip.
    expect(inherited.children[0].className).toContain('fbc-n');
    expect(inherited.children[1]).toBe(via);
    await act(async () => { fireEvent.click(via); });
    expect(onOpenAgent).toHaveBeenCalledWith('Codex');
    // An own fallback is set here: it says « via » nowhere.
    expect(screen.queryByTestId('agent-fallback-ClaudeCode-standard-link-0-via')).toBeNull();
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
    // One own link: no ↑ ↓, no add button; × sits right after the chip.
    expect(screen.queryByTestId('media-link-0-up')).toBeNull();
    expect(screen.queryByTestId('media-link-0-down')).toBeNull();
    expect(screen.queryByTestId('media-add')).toBeNull();
    expect(screen.getByTestId('media-link-0-change').textContent).toBe('Changer');
    const items = within(screen.getByTestId('media-chain')).getAllByRole('listitem');
    expect(items.map((item) => item.querySelector('.fbc-n')?.textContent)).toEqual(['1', '2']);
    expect(items[0].textContent).toContain('OpenRouter');
    expect(items[1].textContent).toContain('LiteLLM');
    expect(within(items[0]).getByTestId('fallback-transport-http').textContent).toBe('HTTP');
    expect(screen.getByTestId('media-model').textContent).toBe('picker-image');
    await act(async () => { fireEvent.click(screen.getByTestId('media-link-0-remove')); });
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it('names the card of the inherited connection, and opens it on click', async () => {
    const onOpenConnection = vi.fn();
    const view: ConnectionChainView = {
      owner: 'conn-x',
      mode: 'image',
      links: [{ connection: 'conn-a', model: 'flux-pro', transport: 'http' }],
      inherited: [{ connection: 'conn-b', model: 'sdxl', transport: 'http' }],
    };
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
            onChange={vi.fn()}
            onOpenConnection={onOpenConnection}
          />
        </I18nProvider>,
      );
    });
    const via = screen.getByTestId('media-inherited-0-via');
    expect(via.textContent).toBe('via LiteLLM');
    expect(via.title).toBe('Défini sur la carte LiteLLM : clique pour l’ouvrir');
    await act(async () => { fireEvent.click(via); });
    expect(onOpenConnection).toHaveBeenCalledWith('conn-b');
  });

  it('changes the one stored fallback from « Changer » instead of a disabled add', async () => {
    const view: ConnectionChainView = {
      owner: 'conn-x', mode: 'image', links: [{ connection: 'conn-a', model: 'flux-pro', transport: 'http' }], inherited: [],
    };
    const onChange = await mountMedia(view);
    await act(async () => { fireEvent.click(screen.getByTestId('media-link-0-change')); });
    await act(async () => { fireEvent.click(screen.getByTestId('media-menu-conn-b-image-sdxl')); });
    expect(onChange).toHaveBeenLastCalledWith({ connection: 'conn-b', model: 'sdxl' });
  });
});

describe('the capability warnings are translated from a stable code', () => {
  const GAPS = ['no_mcp_tools', 'no_write_access', 'no_image_input', 'capped_context'] as const;

  it('maps every gap code to a text in the UI language, never a backend sentence', async () => {
    const view = chain('ClaudeCode', 'standard', {
      links: [{ agent: 'Ollama', model: 'qwen3', mode: 'standard', transport: 'local', gaps: [...GAPS] }],
    });
    await mountAgent('ClaudeCode', [view], MODES);
    expect(screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0-warning').textContent)
      .toBe('pas d’outils MCP, écriture impossible dans le worktree, images non garanties, fenêtre de contexte réduite');
  });

  it('shows a code this build does not know as it came, never as an i18n key', async () => {
    const view = chain('ClaudeCode', 'standard', {
      links: [{ agent: 'Ollama', model: 'qwen3', mode: 'standard', transport: 'local', gaps: ['brand_new_gap' as never] }],
    });
    await mountAgent('ClaudeCode', [view], MODES);
    expect(screen.getByTestId('agent-fallback-ClaudeCode-standard-link-0-warning').textContent).toBe('brand_new_gap');
  });

  it('has a text for every gap code in fr, en, es and zh', async () => {
    const dicts = await Promise.all([
      import('../../../lib/i18n/locales/fr'),
      import('../../../lib/i18n/locales/en'),
      import('../../../lib/i18n/locales/es'),
      import('../../../lib/i18n/locales/zh'),
    ]);
    for (const { default: dict } of dicts) {
      for (const gap of GAPS) expect(dict[`fallback.gap.${gap}`], gap).toBeTruthy();
      expect(dict['fallback.via']).toContain('{0}');
      expect(dict['fallback.openCard']).toContain('{0}');
    }
  });
});
