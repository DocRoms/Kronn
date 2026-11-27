// KT-2013 — the independent review of the UI lot 1: a « Récents » row obeys
// the guards of the row of the same choice in its group (the step's own model
// in the shadow picker, an unavailable mode, an unavailable catalogue entry)
// with the same title; a refused recent exact model shows the refusal next to
// it; the exact-model list says when the catalogue could not be loaded; the
// greyed main row follows the step's mode; the popover is not placed again
// when its own list scrolls.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { CatalogModelEntry, ModelCatalogView, ModelTier } from '../../types/generated';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({ modelCatalogApi: { list: list as never } }));

import { AgentSwitchPicker, type AgentSwitchTarget, type MainAgent } from '../AgentSwitchPicker';
import { AGENT_PICKER_RECENTS_KEY, type AgentPickerRecent } from '../../lib/agentPickerRecents';

function model(runtime: string, id: string, extra: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return {
    id: `${runtime}/${id}`, runtime_target_id: runtime, agent_type: 'ClaudeCode', model_id: id, display_name: id,
    tier_assignment: null, provenance: 'live', availability: 'available', capabilities: ['chat'], reasoning_modes: [],
    manual_origin: false, first_seen_at: '2026-11-21T00:00:00Z', last_checked_at: '2026-11-21T00:00:00Z',
    created_at: '2026-11-21T00:00:00Z', updated_at: '2026-11-21T00:00:00Z', ...extra,
  } as CatalogModelEntry;
}

const view = (runtime: string, agent: CatalogModelEntry['agent_type'], models: CatalogModelEntry[]): ModelCatalogView =>
  ({ runtime_target_id: runtime, agent_type: agent, stale: false, live_refresh_ok: true, models }) as ModelCatalogView;

const CATALOG = {
  targets: [
    view('agent:claude-code', 'ClaudeCode', [
      model('agent:claude-code', 'claude-haiku-4-5', { display_name: 'Haiku', tier_assignment: 'economy' }),
      model('agent:claude-code', 'claude-sonnet-4-5', { display_name: 'Sonnet', tier_assignment: 'default' }),
      model('agent:claude-code', 'claude-opus-4-5', { display_name: 'Opus', tier_assignment: 'reasoning' }),
      model('agent:claude-code', 'claude-fable-a', { display_name: 'Fable' }),
      // Listed, but the catalogue marks it unavailable.
      model('agent:claude-code', 'claude-retired', { display_name: 'Retired', availability: 'unavailable' }),
    ]),
    view('agent:codex', 'Codex', [
      // The economy mode's own model is unavailable: the mode cannot be chosen.
      model('agent:codex', 'gpt-5.5-mini', { agent_type: 'Codex', tier_assignment: 'economy', availability: 'unavailable' }),
      model('agent:codex', 'gpt-5.5', { agent_type: 'Codex', tier_assignment: 'default' }),
      model('agent:codex', 'gpt-5.5-codex-max', { agent_type: 'Codex', tier_assignment: 'reasoning' }),
    ]),
  ],
};

const TARGETS: AgentSwitchTarget[] = [{ agent: 'ClaudeCode' }, { agent: 'Codex' }];
const TIER_LABELS: Record<ModelTier, string> = { economy: 'Éco', default: 'Standard', reasoning: 'Avancé' };

function storeRecents(recents: AgentPickerRecent[]) {
  localStorage.setItem(AGENT_PICKER_RECENTS_KEY, JSON.stringify(recents));
}

function FullList({ onModel, model: held = null }: {
  onModel?: (target: AgentSwitchTarget, model: string) => Promise<void>;
  model?: string | null;
}) {
  return (
    <AgentSwitchPicker
      currentAgent="ClaudeCode"
      currentTier="default"
      availableAgents={[]}
      availableTargets={TARGETS}
      tierLabels={TIER_LABELS}
      defaultModelLabel="Default agent model"
      onTargetSelectionChange={vi.fn().mockResolvedValue(undefined)}
      customModel={{ model: held, onSelect: onModel ?? vi.fn().mockResolvedValue(undefined) }}
      title="Agent"
      ariaLabel="Agent"
    />
  );
}

function ShadowPicker({ main, onChange = vi.fn() }: { main: MainAgent; onChange?: () => void }) {
  return (
    <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} title="Agent" ariaLabel="Agent"
      exactModel={{ value: undefined, onChange, main, testId: 'exact' }} />
  );
}

async function open() {
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: /^Agent( — |$)/ })); });
  const popover = screen.getByRole('dialog', { name: 'Agent' });
  await waitFor(() => expect(list).toHaveBeenCalled());
  await act(async () => {});
  return popover;
}

const group = (name: string) => screen.getByRole('group', { name });
const recentRow = (name: string) => within(group('agentPicker.recent')).getByRole('menuitem', { name });

beforeEach(() => {
  localStorage.clear();
  list.mockReset().mockResolvedValue(CATALOG);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('a recent row obeys the guards of its row (KT-2013, point 1)', () => {
  it('in the shadow picker, a recent equal to the step\'s own agent and model is greyed like its row', async () => {
    storeRecents([
      { agent: 'ClaudeCode', tier: 'reasoning' },
      { agent: 'ClaudeCode', model: 'claude-opus-4-5' },
      { agent: 'ClaudeCode', tier: 'default' },
    ]);
    // The step runs Claude Code on its reasoning mode, no explicit model: Opus.
    render(<ShadowPicker main={{ agent: 'ClaudeCode', tier: 'reasoning' }} />);
    await open();
    const mainRow = within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · disc.tier.reasoning' });
    expect(mainRow).toBeDisabled();
    expect(mainRow).toHaveAttribute('title', 'wf.shadow.rows.mainModel');
    for (const name of [
      'agentPicker.recent: Claude Code · disc.tier.reasoning · claude-opus-4-5',
      'agentPicker.recent: Claude Code · agentPicker.custom · claude-opus-4-5',
    ]) {
      const row = recentRow(name);
      expect(row).toBeDisabled();
      expect(row).toHaveAttribute('data-main', 'true');
      expect(row).toHaveAttribute('title', mainRow.getAttribute('title')!);
    }
    // Another mode of the same agent stays a choice, in the list and as a recent.
    expect(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · disc.tier.default' })).toBeEnabled();
    expect(recentRow('agentPicker.recent: Claude Code · disc.tier.default · claude-sonnet-4-5')).toBeEnabled();
  });

  it('a recent mode the catalogue makes unavailable is disabled like its mode row, with its title', async () => {
    storeRecents([{ agent: 'Codex', tier: 'economy' }, { agent: 'Codex', tier: 'reasoning' }]);
    render(<FullList />);
    await open();
    const modeRow = within(group('Codex')).getByRole('menuitem', { name: 'Codex · Éco' });
    expect(modeRow).toBeDisabled();
    const recent = recentRow('agentPicker.recent: Codex · Éco · gpt-5.5-mini');
    expect(recent).toBeDisabled();
    expect(recent).toHaveAttribute('title', modeRow.getAttribute('title')!);
    expect(recentRow('agentPicker.recent: Codex · Avancé · gpt-5.5-codex-max')).toBeEnabled();
  });

  it('a recent exact model whose catalogue entry is unavailable is disabled like its model row, with its title', async () => {
    storeRecents([
      { agent: 'ClaudeCode', model: 'claude-retired' },
      { agent: 'ClaudeCode', model: 'claude-fable-a' },
    ]);
    render(<FullList />);
    await open();
    fireEvent.click(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · agentPicker.otherModels' }));
    const modelRow = within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · claude-retired' });
    expect(modelRow).toBeDisabled();
    const recent = recentRow('agentPicker.recent: Claude Code · agentPicker.custom · claude-retired');
    expect(recent).toBeDisabled();
    expect(recent).toHaveAttribute('title', modelRow.getAttribute('title')!);
    expect(recent.getAttribute('title')).toContain('modelCatalog.unavailable');
    expect(recentRow('agentPicker.recent: Claude Code · agentPicker.custom · claude-fable-a')).toBeEnabled();
  });
});

describe('refusals and failures are said (KT-2013, points 2 and 3)', () => {
  it('a refused recent exact model shows the refusal next to the recent row and keeps the previous choice', async () => {
    storeRecents([
      { agent: 'ClaudeCode', model: 'claude-fable-a' },
      { agent: 'Codex', tier: 'reasoning' },
    ]);
    const onModel = vi.fn().mockRejectedValue(new Error('This model is not allowed here'));
    render(<FullList onModel={onModel} />);
    await open();
    const recent = recentRow('agentPicker.recent: Claude Code · agentPicker.custom · claude-fable-a');
    await act(async () => { fireEvent.click(recent); });
    expect(onModel).toHaveBeenCalledWith({ agent: 'ClaudeCode' }, 'claude-fable-a');
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('This model is not allowed here');
    expect(recent.nextElementSibling).toBe(alert);
    // Only next to that row; the picker stays open on the previous choice.
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByRole('dialog', { name: 'Agent' })).toBeInTheDocument();
    const trigger = screen.getByRole('button', { name: /^Agent( — |$)/ });
    expect(trigger.querySelector('.kr-agent-switch-custom')).toBeNull();
    expect(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · Standard' }))
      .toHaveAttribute('data-current', 'true');
  });

  it('the exact-model list says the catalogue could not be loaded', async () => {
    list.mockReset().mockRejectedValue(new Error('offline'));
    render(<ShadowPicker main={{ agent: 'Codex' }} />);
    await open();
    expect(await screen.findByText('modelCatalog.loadError')).toHaveAttribute('role', 'status');
  });
});

describe('the greyed main row follows the step\'s mode (KT-2013, point 4)', () => {
  it('a step on a mode without an explicit model greys that mode\'s row, not the agent\'s default row', async () => {
    render(<ShadowPicker main={{ agent: 'ClaudeCode', tier: 'economy' }} />);
    await open();
    const claude = group('Claude Code');
    const economy = within(claude).getByRole('menuitem', { name: 'Claude Code · disc.tier.economy' });
    expect(economy).toBeDisabled();
    expect(economy).toHaveAttribute('data-main', 'true');
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · wf.shadow.rows.defaultModel' })).toBeEnabled();
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · disc.tier.reasoning' })).toBeEnabled();
  });

  it('a mode that names no model is greyed as that mode', async () => {
    list.mockReset().mockResolvedValue({ targets: [] });
    render(<ShadowPicker main={{ agent: 'ClaudeCode', tier: 'reasoning' }} />);
    await open();
    const claude = group('Claude Code');
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · disc.tier.reasoning' })).toBeDisabled();
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · wf.shadow.rows.defaultModel' })).toBeEnabled();
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · disc.tier.default' })).toBeEnabled();
  });

  it('an explicit model greys the rows that run it', async () => {
    render(<ShadowPicker main={{ agent: 'ClaudeCode', model: 'claude-sonnet-4-5', tier: 'reasoning' }} />);
    await open();
    const claude = group('Claude Code');
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · disc.tier.default' })).toBeDisabled();
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · disc.tier.reasoning' })).toBeEnabled();
  });
});

describe('the popover and its own list\'s scroll (KT-2013, point 4)', () => {
  it('is placed again on a page scroll, never on its own list\'s', async () => {
    render(<FullList />);
    const popover = await open();
    const root = document.querySelector('.kr-agent-switch')!;
    const measure = vi.spyOn(root, 'getBoundingClientRect');
    fireEvent.scroll(within(popover).getByRole('menu'));
    expect(measure).not.toHaveBeenCalled();
    fireEvent.scroll(document);
    expect(measure).toHaveBeenCalled();
  });
});
