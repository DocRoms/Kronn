// KT-1693 — the « + Add a fallback » / change menu: never clipped by the card that
// holds the row (drawn in the page body, placed inside the viewport), a search
// field at the top (agent and model), and keyboard navigation.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { I18nProvider } from '../../../lib/I18nContext';
import { AgentFallbackControl } from '../AgentFallbackControl';
import { placeMenu, type FallbackOption } from '../../../lib/fallbackChain';
import type { ChainView } from '../../../types/generated';

const { CODEX_STANDARD } = vi.hoisted(() => ({
  CODEX_STANDARD: {
    agent: 'ClaudeCode',
    mode: 'standard',
    links: [{ agent: 'Codex', model: 'gpt-5.6', mode: 'standard', transport: 'cli', gaps: [] }],
    inherited: [{
      source: 'Codex',
      link: { agent: 'Ollama', model: 'qwen3', mode: 'standard', transport: 'local', gaps: [] },
    }],
  } as unknown as ChainView,
}));

vi.mock('../../../lib/api', () => buildApiMock());

const option = (agent: FallbackOption['agent'], model: string | null, mode: FallbackOption['mode']): FallbackOption => ({
  agent, model, mode,
});

const OPTIONS: FallbackOption[] = [
  option('ClaudeCode', 'sonnet', 'standard'),
  option('Codex', 'gpt-5.6', 'standard'),
  option('Codex', 'gpt-5.6-mini', 'eco'),
  option('OpenCode', 'big-pickle', 'standard'),
  option('OpenCode', null, 'standard'),
  option('Ollama', 'qwen3', 'standard'),
];

const ROW = 'agent-fallback-ClaudeCode-standard';
const ITEM = (suffix: string) => `${ROW}-menu-${suffix}`;

const mount = async (chains: ChainView[] = []) => {
  const onChange = vi.fn().mockResolvedValue(undefined);
  await act(async () => {
    render(
      <I18nProvider>
        <AgentFallbackControl
          agentType="ClaudeCode"
          modes={[{ mode: 'standard', model: 'sonnet' }]}
          options={OPTIONS}
          chains={chains}
          onChange={onChange}
        />
      </I18nProvider>,
    );
  });
  return onChange;
};

type Box = { left: number; right: number; top: number; bottom: number };

const realRect = Element.prototype.getBoundingClientRect;
const realWidth = window.innerWidth;
const realHeight = window.innerHeight;
const MENU = { width: 300, height: 200 };
let anchorBox: Box;

// jsdom has no layout: the anchor and the menu get the boxes a narrow card would give them.
beforeEach(() => {
  anchorBox = { left: 40, right: 140, top: 100, bottom: 122 };
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    const box = this.classList.contains('fbm')
      ? { left: 0, top: 0, right: MENU.width, bottom: MENU.height, ...MENU }
      : this.classList.contains('fbr-anchor')
        ? { ...anchorBox, width: anchorBox.right - anchorBox.left, height: anchorBox.bottom - anchorBox.top }
        : realRect.call(this);
    return { x: box.left, y: box.top, toJSON: () => box, ...box } as DOMRect;
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.innerWidth = realWidth;
  window.innerHeight = realHeight;
});

const open = async () => {
  const onChange = await mount();
  await act(async () => { fireEvent.click(screen.getByTestId(`${ROW}-add`)); });
  return { onChange, menu: screen.getByTestId(`${ROW}-menu`), search: screen.getByTestId(`${ROW}-menu-search`) };
};
const press = (element: HTMLElement, key: string) => act(async () => { fireEvent.keyDown(element, { key }); });
const type = (element: HTMLElement, value: string) => act(async () => { fireEvent.change(element, { target: { value } }); });
const active = (menu: HTMLElement) => menu.querySelector('[data-active="true"]')?.getAttribute('data-testid');

describe('the menu is never clipped', () => {
  it('is drawn in the page body, outside the card that would clip it', async () => {
    const { menu } = await open();
    expect(menu.parentElement).toBe(document.body);
    expect(screen.getByTestId(ROW).contains(menu)).toBe(false);
  });

  it('stays inside the viewport for a card at the right edge, aligned on the anchor\'s right side', async () => {
    window.innerWidth = 1024;
    anchorBox = { left: 960, right: 1010, top: 100, bottom: 122 };
    const { menu } = await open();
    const left = parseFloat(menu.style.left);
    expect(left).toBe(710); // the anchor's right edge − the menu's width
    expect(left).toBeGreaterThanOrEqual(8);
    expect(left + MENU.width).toBeLessThanOrEqual(1024 - 8);
    expect(parseFloat(menu.style.top)).toBeGreaterThan(anchorBox.bottom);
  });

  it('stays inside a viewport narrower than its natural width', async () => {
    window.innerWidth = 280;
    anchorBox = { left: 200, right: 260, top: 100, bottom: 122 };
    const { menu } = await open();
    const left = parseFloat(menu.style.left);
    expect(left).toBeGreaterThanOrEqual(8);
    expect(left).toBeLessThanOrEqual(280 - 8);
  });

  it('opens above the anchor when there is no room below', async () => {
    window.innerHeight = 768;
    anchorBox = { left: 40, right: 140, top: 700, bottom: 722 };
    const { menu } = await open();
    const top = parseFloat(menu.style.top);
    expect(top + MENU.height).toBeLessThanOrEqual(anchorBox.top);
    expect(top).toBeGreaterThanOrEqual(8);
  });

  it('is placed again when the window is resized', async () => {
    window.innerWidth = 1024;
    const { menu } = await open();
    expect(parseFloat(menu.style.left)).toBe(40);
    anchorBox = { left: 960, right: 1010, top: 100, bottom: 122 };
    await act(async () => { window.dispatchEvent(new Event('resize')); });
    expect(parseFloat(menu.style.left)).toBe(710);
  });

  it('computes the place from the boxes only', () => {
    const viewport = { width: 1000, height: 700 };
    // Room below and to the right: under the anchor, left-aligned.
    expect(placeMenu({ left: 50, right: 150, top: 10, bottom: 30 }, MENU, viewport))
      .toEqual({ left: 50, top: 36, maxHeight: 320 });
    // Never past either edge, whatever the anchor.
    for (const left of [-50, 0, 400, 900, 990]) {
      const place = placeMenu({ left, right: left + 60, top: 650, bottom: 672 }, MENU, viewport);
      expect(place.left).toBeGreaterThanOrEqual(8);
      expect(place.left + MENU.width).toBeLessThanOrEqual(1000 - 8);
      expect(place.top).toBeGreaterThanOrEqual(8);
      expect(place.top + Math.min(MENU.height, place.maxHeight)).toBeLessThanOrEqual(700);
    }
  });
});

describe('the search field of the menu', () => {
  it('is on top of the list, focused, with its placeholder', async () => {
    const { menu, search } = await open();
    expect(document.activeElement).toBe(search);
    expect(search.getAttribute('placeholder')).toBe('Rechercher un agent ou un modèle…');
    expect(menu.firstElementChild).toBe(search);
  });

  it('filters by model name', async () => {
    const { menu, search } = await open();
    await type(search, 'gpt-5.6-mini');
    expect(within(menu).getAllByRole('menuitem').map((item) => item.getAttribute('data-testid')))
      .toEqual([ITEM('Codex-eco-gpt-5.6-mini')]);
    expect(within(menu).getAllByRole('group').map((group) => group.getAttribute('aria-label'))).toEqual(['Codex']);
  });

  it('filters by agent name, whatever the case, keeping every model of that agent', async () => {
    const { menu, search } = await open();
    await type(search, 'OPENCODE');
    expect(within(menu).getAllByRole('group').map((group) => group.getAttribute('aria-label'))).toEqual(['OpenCode']);
    expect(within(menu).getAllByRole('menuitem')).toHaveLength(2);
  });

  it('says when nothing matches, and lists everything again once cleared', async () => {
    const { menu, search } = await open();
    await type(search, 'no-such-model');
    expect(within(menu).queryAllByRole('menuitem')).toHaveLength(0);
    expect(within(menu).getByTestId(`${ROW}-menu-nomatch`).textContent).toContain('Aucun agent ni modèle');
    await type(search, '');
    expect(within(menu).getAllByRole('group')).toHaveLength(3);
  });
});

describe('the keyboard in the menu', () => {
  it('moves with the arrows and picks with Enter', async () => {
    const { menu, search, onChange } = await open();
    expect(active(menu)).toBe(ITEM('Codex-standard-gpt-5.6'));
    await press(search, 'ArrowDown');
    expect(active(menu)).toBe(ITEM('Codex-eco-gpt-5.6-mini'));
    await press(search, 'ArrowUp');
    expect(active(menu)).toBe(ITEM('Codex-standard-gpt-5.6'));
    await press(search, 'ArrowDown');
    await press(search, 'Enter');
    expect(onChange).toHaveBeenLastCalledWith('standard', { agent: 'Codex', model: 'gpt-5.6-mini', mode: 'eco' });
    expect(screen.queryByTestId(`${ROW}-menu`)).toBeNull();
  });

  it('starts on the first match of a search and wraps around at both ends', async () => {
    const { menu, search, onChange } = await open();
    await type(search, 'ollama');
    expect(active(menu)).toBe(ITEM('Ollama-standard-qwen3'));
    await type(search, '');
    await press(search, 'ArrowUp');
    expect(active(menu)).toBe(ITEM('Ollama-standard-qwen3'));
    await press(search, 'ArrowDown');
    expect(active(menu)).toBe(ITEM('Codex-standard-gpt-5.6'));
    await press(search, 'Enter');
    expect(onChange).toHaveBeenLastCalledWith('standard', { agent: 'Codex', model: 'gpt-5.6' });
  });

  it('walks past the models that cannot be picked, and Enter with no match does nothing', async () => {
    const onChange = await mount([CODEX_STANDARD]);
    await act(async () => {
      fireEvent.click(within(screen.getByTestId(`${ROW}-link-0`)).getAllByRole('button')[0]);
    });
    const menu = screen.getByTestId(`${ROW}-menu`);
    const search = screen.getByTestId(`${ROW}-menu-search`);
    // Ollama is already in the chain (disabled): up from the top lands on the last model that can be picked.
    await press(search, 'ArrowUp');
    expect(active(menu)).toBe(ITEM('OpenCode-standard'));
    await type(search, 'no-such-model');
    await press(search, 'Enter');
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByTestId(`${ROW}-menu`)).toBeTruthy();
  });

  it('closes on Escape and when clicking away, not when the menu itself is clicked', async () => {
    const { menu, search } = await open();
    await act(async () => { fireEvent.mouseDown(menu); });
    expect(screen.getByTestId(`${ROW}-menu`)).toBeTruthy();
    await press(search, 'Escape');
    expect(screen.queryByTestId(`${ROW}-menu`)).toBeNull();

    await act(async () => { fireEvent.click(screen.getByTestId(`${ROW}-add`)); });
    expect(screen.getByTestId(`${ROW}-menu`)).toBeTruthy();
    await act(async () => { fireEvent.mouseDown(document.body); });
    expect(screen.queryByTestId(`${ROW}-menu`)).toBeNull();
  });
});
