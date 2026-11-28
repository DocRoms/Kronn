// KT-2010 — the agent picker's list: each agent's header is a band of its own
// that stays written at the top of the list while its rows scroll under it
// (never over the focused row), and a free model (its id's last word is
// `free`, as in `:free` or `-free`, or its display name's) carries a
// « Gratuit » / "Free" badge on every row that shows it, in that row's
// accessible name and in the search.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { CatalogModelEntry, ModelCatalogView, ModelTier } from '../../types/generated';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({ modelCatalogApi: { list: list as never } }));

import { AgentSwitchPicker, type AgentSwitchTarget } from '../AgentSwitchPicker';
import { AGENT_PICKER_RECENTS_KEY } from '../../lib/agentPickerRecents';
import { isFreeDisplayName, isFreeModel } from '../../lib/freeModel';
import { I18nProvider } from '../../lib/I18nContext';
import { loadLocale, setUILocale } from '../../lib/i18n';
import fr from '../../lib/i18n/locales/fr';
import en from '../../lib/i18n/locales/en';
import es from '../../lib/i18n/locales/es';
import zh from '../../lib/i18n/locales/zh';

function model(runtime: string, id: string, extra: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return {
    id: `${runtime}/${id}`, runtime_target_id: runtime, agent_type: 'Custom', model_id: id, display_name: id,
    tier_assignment: null, provenance: 'live', availability: 'available', capabilities: ['chat'], reasoning_modes: [],
    manual_origin: false, first_seen_at: '2026-11-22T00:00:00Z', last_checked_at: '2026-11-22T00:00:00Z',
    created_at: '2026-11-22T00:00:00Z', updated_at: '2026-11-22T00:00:00Z', ...extra,
  } as CatalogModelEntry;
}

function view(runtime: string, agent: CatalogModelEntry['agent_type'], models: CatalogModelEntry[]): ModelCatalogView {
  return { runtime_target_id: runtime, agent_type: agent, stale: false, live_refresh_ok: true, models } as ModelCatalogView;
}

const OR = 'http:openrouter';
const PROXY = 'http:my-proxy';
const OC = 'agent:opencode';
const CATALOG = {
  targets: [
    view('agent:claude-code', 'ClaudeCode', [
      model('agent:claude-code', 'claude-haiku-4-5', { agent_type: 'ClaudeCode', tier_assignment: 'economy' }),
      model('agent:claude-code', 'claude-sonnet-4-5', { agent_type: 'ClaudeCode', tier_assignment: 'default' }),
      model('agent:claude-code', 'claude-opus-4-5', { agent_type: 'ClaudeCode', tier_assignment: 'reasoning' }),
    ]),
    view(OR, 'Custom', [
      model(OR, 'vendor/cheap:free', { display_name: 'Vendor: Cheap (free)' }),
      model(OR, 'vendor/model-a'),
      model(OR, 'vendor/model-b'),
      model(OR, 'vendor/other:free', { display_name: 'Vendor: Other (free)' }),
      model(OR, 'vendor/paid'),
      // `free` inside the id is no variant suffix.
      model(OR, 'vendor/freedom'),
      model(OR, 'vendor/held:free'),
    ]),
    // A proxy that lists the same kind of ids gets the badge too: the id decides, not the connection's name.
    view(PROXY, 'Custom', [
      model(PROXY, 'meta/llama:free'), model(PROXY, 'meta/llama'),
      // Free by its display name only.
      model(PROXY, 'meta/quiet', { display_name: 'Meta: Quiet (free)' }),
    ]),
    // OpenCode writes its free models `-free`, and one says it in its display name only.
    view(OC, 'OpenCode', [
      model(OC, 'opencode-go/beam-free', { agent_type: 'OpenCode', tier_assignment: 'economy', display_name: 'Beam Free' }),
      model(OC, 'opencode/zen-paid', { agent_type: 'OpenCode', tier_assignment: 'default' }),
      model(OC, 'opencode/union-alpha', {
        agent_type: 'OpenCode', tier_assignment: 'reasoning', display_name: 'OpenCode Zen/Union Alpha Free',
      }),
      model(OC, 'opencode-go/cat-jam-free', { agent_type: 'OpenCode' }),
      // `free` that is not the last word, in the id or in the name.
      model(OC, 'opencode-go/model-free-preview', { agent_type: 'OpenCode', display_name: 'Model Free Preview' }),
      model(OC, 'opencode/carefree', { agent_type: 'OpenCode', display_name: 'Carefree' }),
    ]),
  ],
};

const OPENROUTER: AgentSwitchTarget = {
  agent: 'Custom', connectionId: 'openrouter', label: 'OpenRouter',
  modelTiers: { economy: 'vendor/cheap:free', default: 'vendor/model-a', reasoning: 'vendor/model-b' },
};
const MY_PROXY: AgentSwitchTarget = {
  agent: 'Custom', connectionId: 'my-proxy', label: 'My proxy',
  modelTiers: { economy: 'meta/llama', default: 'meta/llama', reasoning: 'meta/llama' },
};
const TARGETS: AgentSwitchTarget[] = [{ agent: 'ClaudeCode' }, OPENROUTER, MY_PROXY, { agent: 'OpenCode' }];
const TIER_LABELS: Record<ModelTier, string> = { economy: 'Éco', default: 'Standard', reasoning: 'Avancé' };

function FullList({ model: held = null, currentAgent = 'ClaudeCode', currentConnectionId = null }: {
  model?: string | null; currentAgent?: AgentSwitchTarget['agent']; currentConnectionId?: string | null;
}) {
  return (
    <AgentSwitchPicker
      currentAgent={currentAgent}
      currentConnectionId={currentConnectionId}
      currentTier="default"
      availableAgents={[]}
      availableTargets={TARGETS}
      tierLabels={TIER_LABELS}
      defaultModelLabel="Default agent model"
      onTargetSelectionChange={vi.fn().mockResolvedValue(undefined)}
      customModel={{ model: held, onSelect: vi.fn().mockResolvedValue(undefined) }}
      title="Agent"
      ariaLabel="Agent"
    />
  );
}

async function open() {
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: /^Agent( — |$)/ })); });
  await waitFor(() => expect(list).toHaveBeenCalled());
  await act(async () => {});
}

const group = (name: string) => screen.getByRole('group', { name });
const badgeOf = (row: HTMLElement) => row.querySelector('[data-testid="agent-switch-free-badge"]');
const unfold = (name: string) =>
  fireEvent.click(within(group(name)).getByRole('menuitem', { name: `${name} · agentPicker.otherModels` }));
const rect = (top: number, height: number) =>
  () => ({ top, bottom: top + height, left: 0, right: 440, width: 440, height, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;

beforeEach(() => {
  localStorage.clear();
  list.mockReset().mockResolvedValue(CATALOG);
});
afterEach(() => {
  cleanup();
  setUILocale('fr');
});

describe('isFreeModel: the id\'s last word, or the display name\'s, decides (KT-2010)', () => {
  it.each([
    'vendor/model:free', 'meta-llama/llama-3.3-70b-instruct:free', ' vendor/cheap:FREE ',
    'opencode-go/beam-free', 'opencode-go/cat-jam-free', 'openrouter/free', 'x_free', 'free', 'FREE', ':free',
  ])('%j is free', id => {
    expect(isFreeModel(id)).toBe(true);
  });

  it.each([
    'freedom-1', 'vendor/freeze', 'carefree', 'model-free-preview', 'vendor/x:free-preview', 'vendor/freedom',
    'vendor/paid', 'opencode/union-alpha', 'free-model', 'vendor/free.1', '', '   ', null, undefined,
  ])('%j is not free', id => {
    expect(isFreeModel(id)).toBe(false);
  });

  it.each([
    ['opencode/union-alpha', 'OpenCode Zen/Union Alpha Free'],
    ['vendor/cheap', 'Vendor: Cheap (free)'],
    ['vendor/cheap', '  Vendor: Cheap (FREE)  '],
    ['vendor/model', 'Free'],
  ])('%j is free by its display name %j', (id, name) => {
    expect(isFreeDisplayName(name)).toBe(true);
    expect(isFreeModel(id, name)).toBe(true);
  });

  it.each([
    'Carefree', 'Model Free Preview', 'Freedom', 'Free tier (beta)', 'Union Alpha', '', null, undefined,
  ])('the display name %j says nothing', name => {
    expect(isFreeDisplayName(name)).toBe(false);
    expect(isFreeModel('vendor/paid', name)).toBe(false);
  });

  it('a display name alone, with no id, names no model', () => {
    expect(isFreeModel('', 'Alpha Free')).toBe(false);
    expect(isFreeModel(null, 'Alpha Free')).toBe(false);
  });
});

describe('the agent header is a band of its own (KT-2010)', () => {
  it('renders with its own class, the name in its own element, the kind, state and count kept on the line', async () => {
    render(<FullList />);
    await open();
    for (const name of ['Claude Code', 'OpenRouter', 'My proxy']) {
      const block = group(name);
      const header = block.querySelector<HTMLElement>(':scope > .kr-agent-switch-group-header');
      expect(header, name).not.toBeNull();
      // The band is the block's first child: the rows sit under it.
      expect(block.firstElementChild).toBe(header);
      expect(header!.querySelector('.kr-agent-switch-group-name')).toHaveTextContent(name);
      expect(header!.querySelector('.kr-agent-switch-option-dot')).not.toBeNull();
      expect(header!.querySelector('.kr-agent-switch-kind')).not.toBeNull();
      expect(header!.querySelector('.kr-agent-switch-model-count')).not.toBeNull();
      // Not a row: the keyboard never stops on it.
      expect(header!.querySelector('button, [role="menuitem"]')).toBeNull();
    }
  });

  it('a row taking the focus under the band is scrolled to just below it; a row in the clear is left where it is', async () => {
    render(<FullList />);
    await open();
    unfold('OpenRouter');
    const menu = screen.getByRole('menu');
    let scrollTop = 200;
    Object.defineProperty(menu, 'scrollTop', { configurable: true, get: () => scrollTop, set: value => { scrollTop = value; } });
    menu.getBoundingClientRect = rect(100, 300);
    const block = group('OpenRouter');
    const header = block.querySelector<HTMLElement>('.kr-agent-switch-group-header')!;
    // The band sticks at the top of the list, 30 px high.
    header.getBoundingClientRect = rect(100, 30);
    const row = within(block).getByRole('menuitem', { name: 'OpenRouter · vendor/paid' });
    // Its top 10 px under the band's bottom edge (130): 20 px are covered.
    row.getBoundingClientRect = rect(110, 26);
    act(() => { row.focus(); });
    expect(document.activeElement).toBe(row);
    expect(scrollTop).toBe(200 - (100 + 30 - 110));
    // Above the list entirely (the arrows move up past the fold): under the band, not at the list's edge.
    scrollTop = 500;
    const above = within(block).getByRole('menuitem', { name: 'OpenRouter · Standard' });
    above.getBoundingClientRect = rect(40, 26);
    act(() => { above.focus(); });
    expect(scrollTop).toBe(500 - (100 + 30 - 40));
    // A row already clear of the band does not move the list.
    scrollTop = 77;
    const clear = within(block).getByRole('menuitem', { name: 'OpenRouter · Avancé' });
    clear.getBoundingClientRect = rect(140, 26);
    act(() => { clear.focus(); });
    expect(scrollTop).toBe(77);
  });

  it('the arrow keys keep the focused row clear of its band', async () => {
    render(<FullList />);
    await open();
    const menu = screen.getByRole('menu');
    let scrollTop = 300;
    Object.defineProperty(menu, 'scrollTop', { configurable: true, get: () => scrollTop, set: value => { scrollTop = value; } });
    menu.getBoundingClientRect = rect(100, 300);
    const block = group('OpenRouter');
    block.querySelector<HTMLElement>('.kr-agent-switch-group-header')!.getBoundingClientRect = rect(100, 28);
    const second = within(block).getByRole('menuitem', { name: 'OpenRouter · Standard' });
    const first = within(block).getByRole('menuitem', { name: /^OpenRouter · Éco/ });
    second.getBoundingClientRect = rect(130, 26);
    first.getBoundingClientRect = rect(104, 26);
    act(() => { second.focus(); });
    expect(scrollTop).toBe(300);
    // ArrowUp from Standard lands on Éco, under the band.
    fireEvent.keyDown(second, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(first);
    expect(scrollTop).toBe(300 - (100 + 28 - 104));
  });

  it('unfolding puts the control right under its band, not under it', async () => {
    render(<FullList />);
    await open();
    const menu = screen.getByRole('menu');
    let scrollTop = 40;
    Object.defineProperty(menu, 'scrollTop', { configurable: true, get: () => scrollTop, set: value => { scrollTop = value; } });
    menu.getBoundingClientRect = rect(100, 300);
    const block = group('OpenRouter');
    block.querySelector<HTMLElement>('.kr-agent-switch-group-header')!.getBoundingClientRect = rect(150, 30);
    const toggle = within(block).getByRole('menuitem', { name: 'OpenRouter · agentPicker.otherModels' });
    toggle.getBoundingClientRect = rect(380, 24);
    fireEvent.click(toggle, { detail: 0 });
    expect(scrollTop).toBe(40 + (380 - 100) - 30);
  });
});

describe('a free model carries the « Free » badge on every row that shows it (KT-2010)', () => {
  it('a mode row whose model is free: badge after the id, word in the accessible name; the other modes have none', async () => {
    render(<FullList />);
    await open();
    const openrouter = group('OpenRouter');
    const eco = within(openrouter).getByRole('menuitem', { name: 'OpenRouter · Éco · agentPicker.freeBadge' });
    const badge = badgeOf(eco);
    expect(badge).toHaveTextContent('agentPicker.freeBadge');
    // Right after the id.
    expect(badge?.previousElementSibling).toHaveClass('kr-agent-switch-mode-model');
    expect(badge?.previousElementSibling).toHaveTextContent('vendor/cheap:free');
    for (const name of ['OpenRouter · Standard', 'OpenRouter · Avancé']) {
      expect(badgeOf(within(openrouter).getByRole('menuitem', { name }))).toBeNull();
    }
    // No other agent's mode carries one.
    for (const row of within(group('Claude Code')).getAllByRole('menuitem')) expect(badgeOf(row)).toBeNull();
  });

  it('other models: the free ones only, on any connection listing such an id', async () => {
    render(<FullList />);
    await open();
    unfold('OpenRouter');
    const openrouter = group('OpenRouter');
    expect(badgeOf(within(openrouter).getByRole('menuitem', { name: 'OpenRouter · vendor/other:free · agentPicker.freeBadge' })))
      .not.toBeNull();
    for (const id of ['vendor/paid', 'vendor/freedom']) {
      expect(badgeOf(within(openrouter).getByRole('menuitem', { name: `OpenRouter · ${id}` })), id).toBeNull();
    }
    unfold('My proxy');
    expect(badgeOf(within(group('My proxy')).getByRole('menuitem', { name: 'My proxy · meta/llama:free · agentPicker.freeBadge' })))
      .not.toBeNull();
    // The proxy's paid model is its modes' model: none of them is badged.
    for (const row of within(group('My proxy')).getAllByRole('menuitem').filter(item => item.dataset.kind === 'mode')) {
      expect(badgeOf(row)).toBeNull();
    }
  });

  it('the custom-model row and the recents', async () => {
    localStorage.setItem(AGENT_PICKER_RECENTS_KEY, JSON.stringify([
      { agent: 'Custom', connectionId: 'openrouter', model: 'vendor/other:free' },
      { agent: 'Custom', connectionId: 'openrouter', model: 'vendor/paid' },
      { agent: 'Custom', connectionId: 'openrouter', tier: 'economy' },
    ]));
    render(<FullList currentAgent="Custom" currentConnectionId="openrouter" model="vendor/held:free" />);
    await open();
    const custom = within(group('OpenRouter')).getByRole('menuitem', {
      name: 'OpenRouter · agentPicker.custom · vendor/held:free · agentPicker.freeBadge',
    });
    expect(custom).toHaveAttribute('data-kind', 'custom');
    expect(badgeOf(custom)).not.toBeNull();
    const recents = within(group('agentPicker.recent')).getAllByRole('menuitem');
    expect(recents.map(row => row.getAttribute('aria-label'))).toEqual([
      'agentPicker.recent: OpenRouter · agentPicker.custom · vendor/other:free · agentPicker.freeBadge',
      'agentPicker.recent: OpenRouter · agentPicker.custom · vendor/paid',
      'agentPicker.recent: OpenRouter · Éco · vendor/cheap:free · agentPicker.freeBadge',
    ]);
    expect(recents.map(row => Boolean(badgeOf(row)))).toEqual([true, false, true]);
  });

  it('says « Gratuit » in French, in the row and in its accessible name', async () => {
    await loadLocale('fr');
    setUILocale('fr');
    render(<I18nProvider><FullList /></I18nProvider>);
    await open();
    const eco = within(group('OpenRouter')).getByRole('menuitem', { name: 'OpenRouter · Éco · Gratuit' });
    expect(badgeOf(eco)).toHaveTextContent('Gratuit');
  });

  it('the four locales name the badge', () => {
    expect([fr, en, es, zh].map(dict => dict['agentPicker.freeBadge'])).toEqual(['Gratuit', 'Free', 'Gratis', '免费']);
  });
});

describe('the other spellings of free: `-free` ids and a display name (KT-2010 round 2)', () => {
  const oc = () => group('OpenCode');

  it('a `-free` mode row and a mode free by its display name only: badge and accessible name; the paid mode has none', async () => {
    render(<FullList />);
    await open();
    const eco = within(oc()).getByRole('menuitem', { name: 'OpenCode · Éco · agentPicker.freeBadge' });
    expect(eco.querySelector('.kr-agent-switch-mode-model')).toHaveTextContent('opencode-go/beam-free');
    expect(badgeOf(eco)).toHaveTextContent('agentPicker.freeBadge');
    const advanced = within(oc()).getByRole('menuitem', { name: 'OpenCode · Avancé · agentPicker.freeBadge' });
    expect(advanced.querySelector('.kr-agent-switch-mode-model')).toHaveTextContent('opencode/union-alpha');
    expect(badgeOf(advanced)).not.toBeNull();
    expect(badgeOf(within(oc()).getByRole('menuitem', { name: 'OpenCode · Standard' }))).toBeNull();
  });

  it('other models: `-free` is free, `free` that is not the last word is not', async () => {
    render(<FullList />);
    await open();
    unfold('OpenCode');
    expect(badgeOf(within(oc()).getByRole('menuitem', { name: 'OpenCode · opencode-go/cat-jam-free · agentPicker.freeBadge' })))
      .not.toBeNull();
    for (const id of ['opencode-go/model-free-preview', 'opencode/carefree']) {
      expect(badgeOf(within(oc()).getByRole('menuitem', { name: `OpenCode · ${id}` })), id).toBeNull();
    }
  });

  it('a recent row of `opencode-go/beam-free`, and a recent judged by its group\'s catalogue entry', async () => {
    localStorage.setItem(AGENT_PICKER_RECENTS_KEY, JSON.stringify([
      { agent: 'OpenCode', model: 'opencode-go/beam-free' },
      { agent: 'OpenCode', model: 'opencode/union-alpha' },
      { agent: 'OpenCode', model: 'opencode/zen-paid' },
    ]));
    render(<FullList />);
    await open();
    const recents = within(group('agentPicker.recent')).getAllByRole('menuitem');
    expect(recents.map(row => row.getAttribute('aria-label'))).toEqual([
      'agentPicker.recent: OpenCode · agentPicker.custom · opencode-go/beam-free · agentPicker.freeBadge',
      'agentPicker.recent: OpenCode · agentPicker.custom · opencode/union-alpha · agentPicker.freeBadge',
      'agentPicker.recent: OpenCode · agentPicker.custom · opencode/zen-paid',
    ]);
    expect(recents.map(row => Boolean(badgeOf(row)))).toEqual([true, true, false]);
  });

  it('the custom row: a `-free` id, and an id free by its catalogue entry\'s display name', async () => {
    const { unmount } = render(<FullList currentAgent="OpenCode" model="opencode-go/cat-jam-free" />);
    await open();
    expect(badgeOf(within(oc()).getByRole('menuitem', {
      name: 'OpenCode · agentPicker.custom · opencode-go/cat-jam-free · agentPicker.freeBadge',
    }))).not.toBeNull();
    unmount();
    render(<FullList currentAgent="Custom" currentConnectionId="my-proxy" model="meta/quiet" />);
    await open();
    const custom = within(group('My proxy')).getByRole('menuitem', {
      name: 'My proxy · agentPicker.custom · meta/quiet · agentPicker.freeBadge',
    });
    expect(custom).toHaveAttribute('data-kind', 'custom');
    expect(badgeOf(custom)).not.toBeNull();
  });

  it('the badge word and `free` find them; Claude Code stays out', async () => {
    await loadLocale('fr');
    setUILocale('fr');
    render(<I18nProvider><FullList /></I18nProvider>);
    await open();
    const search = screen.getByRole('searchbox');
    fireEvent.change(search, { target: { value: 'gratuit' } });
    const ids = () => within(oc()).getAllByRole('menuitem')
      .map(item => `${item.dataset.kind}:${item.querySelector('.kr-agent-switch-mode-model')?.textContent ?? ''}`);
    expect(ids()).toEqual(['mode:opencode-go/beam-free', 'mode:opencode/union-alpha', 'model:opencode-go/cat-jam-free']);
    expect(within(oc()).getByRole('menuitem', { name: 'OpenCode · Éco · Gratuit' })).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'Claude Code' })).toBeNull();
    // `free` is a text search: the free ones, and the ids or names that hold the word.
    fireEvent.change(search, { target: { value: 'free' } });
    expect(ids()).toEqual([
      'mode:opencode-go/beam-free', 'mode:opencode/union-alpha',
      'model:opencode-go/cat-jam-free', 'model:opencode-go/model-free-preview', 'model:opencode/carefree',
    ]);
  });
});

describe('the search finds free models by the badge word (KT-2010)', () => {
  const kinds = (name: string) => within(group(name)).getAllByRole('menuitem')
    .map(item => `${item.dataset.kind}:${item.querySelector('.kr-agent-switch-mode-model')?.textContent ?? ''}`);

  it('in the interface language and as `free`: every free model, folded or not, and nothing else', async () => {
    await loadLocale('fr');
    setUILocale('fr');
    render(<I18nProvider><FullList currentAgent="Custom" currentConnectionId="openrouter" model="vendor/held:free" /></I18nProvider>);
    await open();
    const search = screen.getByRole('searchbox');
    for (const word of ['gratuit', 'Gratuit']) {
      fireEvent.change(search, { target: { value: word } });
      expect(kinds('OpenRouter'), word).toEqual(['mode:vendor/cheap:free', 'custom:vendor/held:free', 'model:vendor/other:free']);
      // `meta/quiet` by its display name « Meta: Quiet (free) ».
      expect(kinds('My proxy'), word).toEqual(['model:meta/llama:free', 'model:meta/quiet']);
      expect(screen.queryByRole('group', { name: 'Claude Code' }), word).toBeNull();
    }
    // `free` is also any id that holds it (a text search): the free models, and `vendor/freedom`.
    fireEvent.change(search, { target: { value: 'free' } });
    expect(kinds('OpenRouter')).toEqual([
      'mode:vendor/cheap:free', 'custom:vendor/held:free', 'model:vendor/other:free', 'model:vendor/freedom',
    ]);
    expect(kinds('My proxy')).toEqual(['model:meta/llama:free', 'model:meta/quiet']);
  });
});
