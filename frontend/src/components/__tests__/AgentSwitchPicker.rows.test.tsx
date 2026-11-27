// KT-1885 review 2 — a row with tier choices, as the principal decided it
// (the field and chip modes, the API assistants, the composer's @ palette, one
// implementation): the agent with ONE state badge, then its tiers on equal
// tracks, each a one-line button. The state the tiers share is written once,
// next to the agent; a tier carries its own only when it differs. Every tier
// keeps the full state in its tooltip and its description. The assistants'
// « agent default » is a fourth track with a short visible label.
import { cleanup, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());

import { AgentPickerPalette, AgentPickerTierChoices, type AgentPickerPaletteRow } from '../AgentSwitchPicker';
import { composerMentionBoundary } from '../../lib/agentPickerPalette';
import { rowAvailability, sharedTierState, tierChoiceState } from '../../lib/agentAvailability';
import type { ResolvedCatalogTier } from '../../lib/modelCatalogSelection';
import type { ModelTier } from '../../types/generated';

afterEach(cleanup);

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}(${args.join('|')})` : key);

/** What the catalogue resolves for one tier: a `provenance` model, or an unavailable one. */
function resolution(provenance: 'live' | 'cached' | 'migrated', unavailable = false): ResolvedCatalogTier {
  return {
    configured: `model-${provenance}`,
    entry: { model_id: `model-${provenance}`, last_checked_at: '2026-11-01T00:00:00Z' },
    view: undefined,
    model: `model-${provenance}`,
    unavailable,
    replacement: null,
    refusal: unavailable ? 'not launchable' : null,
    notice: null,
    provenance,
  } as unknown as ResolvedCatalogTier;
}

const tierLabel = (tier: ModelTier) => ({ economy: 'Éco', default: 'Standard', reasoning: 'Avancé' })[tier];

function row(key: string, resolve: (tier: ModelTier) => ResolvedCatalogTier, extra: Partial<AgentPickerPaletteRow> = {}): AgentPickerPaletteRow {
  return {
    key,
    agent: 'ClaudeCode',
    label: `@${key}`,
    detail: 'Claude Code',
    onSelect: vi.fn(),
    tiers: {
      targetLabel: `@${key}`,
      resolve,
      tierLabel,
      modelLabel: tier => resolve(tier).model,
      defaultLabel: 'Modèle par défaut de l’agent',
      onSelect: vi.fn(),
    },
    ...extra,
  };
}

const tierButtons = (scope: HTMLElement) =>
  Array.from(scope.querySelectorAll<HTMLButtonElement>('.kr-agent-switch-tier-option'));

describe('a tier row: the shared state once, next to the agent (KT-1885 review 2)', () => {
  it('three tiers in the same catalogue state: written once, in the row’s badge, never in a tier', () => {
    render(<AgentPickerPalette ariaLabel="Agents" t={t} rows={[row('claude', () => resolution('cached'))]} />);
    const option = screen.getByRole('option');
    expect(within(option).getAllByText('modelCatalog.provenance.cached')).toHaveLength(1);
    const badge = within(option).getByTestId('agent-catalog-state');
    expect(badge).toHaveAttribute('data-state', 'catalog');
    // Next to the name, on the agent's side of the row.
    expect(badge.closest('.kr-agent-switch-palette-main')).not.toBeNull();
    for (const button of tierButtons(option)) {
      expect(button.querySelector('.kr-agent-switch-catalog-meta')).toBeNull();
      // Still in its tooltip and its description.
      expect(button.title).toContain('modelCatalog.provenance.cached');
      expect(button).toHaveAccessibleDescription(expect.stringContaining('modelCatalog.provenance.cached'));
    }
  });

  it('one tier differs: only that tier carries its own state, under its name', () => {
    render(<AgentPickerPalette ariaLabel="Agents" t={t} rows={[
      row('claude', tier => resolution(tier === 'reasoning' ? 'migrated' : 'cached')),
    ]} />);
    const option = screen.getByRole('option');
    expect(option.querySelector('.kr-agent-switch-palette-main .kr-agent-switch-badge'))
      .toHaveTextContent('modelCatalog.provenance.cached');
    const [economy, standard, advanced] = tierButtons(option);
    expect(economy.querySelector('.kr-agent-switch-catalog-meta')).toBeNull();
    expect(standard.querySelector('.kr-agent-switch-catalog-meta')).toBeNull();
    expect(advanced.querySelector('.kr-agent-switch-catalog-meta')).toHaveTextContent('modelCatalog.provenance.migrated');
    expect(within(option).getAllByText('modelCatalog.provenance.cached')).toHaveLength(1);
  });

  it('an unavailable tier among detected ones keeps the danger state on that tier, and cannot be chosen', () => {
    render(<AgentPickerPalette ariaLabel="Agents" t={t} rows={[
      row('claude', tier => resolution('live', tier === 'reasoning')),
    ]} />);
    const option = screen.getByRole('option');
    const advanced = tierButtons(option)[2];
    expect(advanced).toBeDisabled();
    expect(within(advanced).getByTestId('agent-unavailable')).toHaveAttribute('data-state', 'unavailable');
    expect(within(option).getByTestId('agent-catalog-state')).toHaveTextContent('modelCatalog.provenance.live');
  });

  it('the agent’s own state outranks the tiers’ neutral one: still one badge', () => {
    render(<AgentPickerPalette ariaLabel="Agents" t={t} rows={[
      row('claude', () => resolution('cached'), { availability: { state: 'unavailable', label: 'Indisponible' } }),
    ]} />);
    const option = screen.getByRole('option');
    const badges = option.querySelectorAll('.kr-agent-switch-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0]).toHaveAttribute('data-state', 'unavailable');
  });

  // KT-1994 — the @ menu writes the model under each tier's name; nothing else is visible.
  it('a tier button is its icon and its name on one line, then the model it runs, nothing else visible', () => {
    render(<AgentPickerPalette ariaLabel="Agents" t={t} rows={[row('claude', () => resolution('live'))]} />);
    for (const button of tierButtons(screen.getByRole('option'))) {
      const visible = Array.from(button.children).filter(child => !(child as HTMLElement).hidden && child.tagName !== 'svg');
      expect(visible.map(child => child.className)).toEqual(['kr-agent-switch-tier-label', 'kr-agent-switch-tier-model']);
      expect(button.querySelector('.kr-agent-switch-tier-model')).toHaveTextContent('model-live');
    }
  });
});

describe('tier tracks and the agent default (KT-1885 review 2)', () => {
  const common = {
    targetLabel: 'Claude Code',
    resolve: () => resolution('live'),
    tierLabel,
    modelLabel: () => 'model-live',
    defaultLabel: 'Modèle par défaut de l’agent',
    onSelect: vi.fn(),
    t,
  };

  it('three tiers make three tracks', () => {
    const { container } = render(<AgentPickerTierChoices {...common} tiers={['economy', 'default', 'reasoning']} />);
    const group = container.querySelector<HTMLElement>('.kr-agent-switch-tier-choices')!;
    expect(group.style.getPropertyValue('--agent-switch-tier-count')).toBe('3');
  });

  it('the agent default is a fourth track: « Défaut » shown, the full name in its tooltip and accessible name', () => {
    const { container } = render(<AgentPickerTierChoices {...common} tiers={[null, 'economy', 'default', 'reasoning']} />);
    const group = container.querySelector<HTMLElement>('.kr-agent-switch-tier-choices')!;
    expect(group.style.getPropertyValue('--agent-switch-tier-count')).toBe('4');
    const fallback = container.querySelector<HTMLButtonElement>('[data-tier="agent-default"]')!;
    // The visible label is the short one; the long one is only in its name and tooltip.
    expect(fallback.querySelector('.kr-agent-switch-tier-label')).toHaveTextContent(/^agentPicker\.defaultTier$/);
    expect(fallback).toHaveAccessibleName('Claude Code · Modèle par défaut de l’agent');
    expect(fallback).toHaveAttribute('title', 'Modèle par défaut de l’agent');
  });

  it('every locale has a short visible « default »', async () => {
    const locales = await Promise.all(['fr', 'en', 'es', 'zh'].map(locale => import(`../../lib/i18n/locales/${locale}.ts`)));
    for (const { default: dictionary } of locales) {
      const label = (dictionary as Record<string, string>)['agentPicker.defaultTier'];
      expect(label.length).toBeGreaterThan(0);
      expect(label.length).toBeLessThanOrEqual(8);
    }
  });
});

describe('the palette’s measured room (KT-1885 review 2)', () => {
  const rect = (top: number, bottom: number) => ({
    top, bottom, left: 0, right: 0, width: 0, height: bottom - top, x: 0, y: top, toJSON: () => ({}),
  }) as DOMRect;

  it('opened above: from its bottom up to the boundary’s top, minus --kr-space-2', () => {
    const area = document.createElement('div');
    const spy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this === area) return rect(120, 800);
      return this.classList.contains('kr-agent-switch-palette') ? rect(-300, 640) : rect(0, 0);
    });
    try {
      render(<AgentPickerPalette ariaLabel="Agents" t={t} boundary={() => area} rows={[row('claude', () => resolution('live'))]} />);
      expect(screen.getByRole('listbox').style.maxHeight).toBe('512px');
    } finally {
      spy.mockRestore();
    }
  });

  it('opened below: from its top down to the boundary’s bottom, never past the window', () => {
    const area = document.createElement('div');
    const spy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this === area) return rect(0, 10_000);
      return this.classList.contains('kr-agent-switch-palette') ? rect(300, 1300) : rect(0, 0);
    });
    try {
      render(<AgentPickerPalette ariaLabel="Agents" t={t} placement="below" boundary={() => area} rows={[row('claude', () => resolution('live'))]} />);
      // jsdom's window is 768 px high: 768 − 300 − 8.
      expect(screen.getByRole('listbox').style.maxHeight).toBe(`${window.innerHeight - 300 - 8}px`);
    } finally {
      spy.mockRestore();
    }
  });

  it('without a boundary it keeps its caller’s height', () => {
    render(<AgentPickerPalette ariaLabel="Agents" t={t} style={{ maxHeight: 250 }} rows={[row('claude', () => resolution('live'))]} />);
    expect(screen.getByRole('listbox').style.maxHeight).toBe('250px');
  });
});

describe('composerMentionBoundary (KT-1885 review 2)', () => {
  it('is the message list of the composer’s column, else the column, else nothing', () => {
    document.body.innerHTML = `
      <div class="disc-messages-col" id="col">
        <div class="disc-messages" id="messages"></div>
        <div class="disc-composer"><textarea id="field"></textarea></div>
      </div>
      <div class="disc-messages-col" id="bare"><textarea id="bare-field"></textarea></div>
      <textarea id="alone"></textarea>`;
    expect(composerMentionBoundary(document.getElementById('field'))?.id).toBe('messages');
    expect(composerMentionBoundary(document.getElementById('bare-field'))?.id).toBe('bare');
    expect(composerMentionBoundary(document.getElementById('alone'))).toBeNull();
    expect(composerMentionBoundary(null)).toBeNull();
    document.body.innerHTML = '';
  });
});

describe('agentAvailability helpers (KT-1885 review 2)', () => {
  const cached = { state: 'catalog' as const, label: 'cached' };
  const migrated = { state: 'catalog' as const, label: 'migrated' };

  it('a state is shared when more than half of the tiers carry it', () => {
    expect(sharedTierState([cached, cached, cached])).toEqual(cached);
    expect(sharedTierState([cached, cached, migrated])).toEqual(cached);
    expect(sharedTierState([cached, migrated, null])).toBeNull();
    expect(sharedTierState([null, null, null])).toBeNull();
  });

  it('a row shows the more serious of its own state and its tiers’ shared one', () => {
    const unavailable = { state: 'unavailable' as const, label: 'Indisponible' };
    const fullAccess = { state: 'needs-full-access' as const, label: 'Accès complet' };
    expect(rowAvailability(null, cached)).toEqual(cached);
    expect(rowAvailability(fullAccess, cached)).toEqual(fullAccess);
    expect(rowAvailability(fullAccess, unavailable)).toEqual(unavailable);
    expect(rowAvailability(null, null)).toBeNull();
  });

  it('a tier’s state: its availability first, else where the catalogue knows its model from', () => {
    expect(tierChoiceState(resolution('cached'), t)).toEqual({ state: 'catalog', label: 'modelCatalog.provenance.cached' });
    expect(tierChoiceState(resolution('live', true), t)?.state).toBe('unavailable');
    expect(tierChoiceState({ ...resolution('live'), entry: undefined, provenance: null, configured: 'x' } as unknown as ResolvedCatalogTier, t))
      .toEqual({ state: 'catalog', label: 'modelCatalog.notInCatalog' });
    expect(tierChoiceState(null, t)).toBeNull();
  });
});
