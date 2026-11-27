// KT-1994 — disposition A of the agent picker: per agent a header (dot, name,
// kind, one state badge, model count), its three modes with the model id
// written, then its other models folded behind one control. A model outside
// the modes is « Personnalisé » / "Custom"; the search reaches every model
// without unfolding; a « Récents » section keeps the last three choices in the
// UI preferences; the composer's @ menu writes the model of each mode and
// offers no exact model.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { CatalogModelEntry, ModelCatalogView, ModelTier } from '../../types/generated';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({ modelCatalogApi: { list: list as never } }));

import {
  AgentPickerPalette, AgentSwitchPicker, OTHER_MODELS_RENDER_LIMIT, type AgentSwitchTarget,
} from '../AgentSwitchPicker';
import { AGENT_PICKER_RECENTS_KEY, readAgentPickerRecents } from '../../lib/agentPickerRecents';
import { SYNCED_KEYS, isSyncedKey } from '../../lib/uiPreferences';
import { setStorageWriteListener } from '../../lib/safeStorage';
import { I18nProvider } from '../../lib/I18nContext';
import { loadLocale, setUILocale } from '../../lib/i18n';
import type { ResolvedCatalogTier } from '../../lib/modelCatalogSelection';

function model(runtime: string, id: string, extra: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return {
    id: `${runtime}/${id}`, runtime_target_id: runtime, agent_type: 'ClaudeCode', model_id: id, display_name: id,
    tier_assignment: null, provenance: 'live', availability: 'available', capabilities: ['chat'], reasoning_modes: [],
    manual_origin: false, first_seen_at: '2026-11-21T00:00:00Z', last_checked_at: '2026-11-21T00:00:00Z',
    created_at: '2026-11-21T00:00:00Z', updated_at: '2026-11-21T00:00:00Z', ...extra,
  } as CatalogModelEntry;
}

function view(runtime: string, agent: CatalogModelEntry['agent_type'], models: CatalogModelEntry[], extra: Partial<ModelCatalogView> = {}): ModelCatalogView {
  return { runtime_target_id: runtime, agent_type: agent, stale: false, live_refresh_ok: true, models, ...extra } as ModelCatalogView;
}

const OPENROUTER_MODELS = Array.from({ length: 120 }, (_, index) =>
  model('http:openrouter', `vendor/model-${String(index).padStart(3, '0')}`, { agent_type: 'Custom' }));

const CATALOG = {
  targets: [
    view('agent:claude-code', 'ClaudeCode', [
      model('agent:claude-code', 'claude-haiku-4-5', { display_name: 'Haiku', tier_assignment: 'economy' }),
      model('agent:claude-code', 'claude-sonnet-4-5', { display_name: 'Sonnet', tier_assignment: 'default' }),
      model('agent:claude-code', 'claude-opus-4-5', { display_name: 'Opus', tier_assignment: 'reasoning' }),
      model('agent:claude-code', 'claude-fable-a', { display_name: 'Fable' }),
      model('agent:claude-code', 'claude-fable-b', { display_name: 'Fable' }),
    ]),
    // No tier assignment: the backend's launch verdicts name each mode's model.
    view('agent:codex', 'Codex', [
      model('agent:codex', 'gpt-5.5-mini', { agent_type: 'Codex' }),
      model('agent:codex', 'gpt-5.5', { agent_type: 'Codex' }),
      model('agent:codex', 'gpt-5.5-codex-max', { agent_type: 'Codex' }),
      // The display name only differs by case and punctuation: not written.
      model('agent:codex', 'gpt-6-luna', { agent_type: 'Codex', display_name: 'GPT-6-Luna' }),
      model('agent:codex', 'opencode-go/mimo-v2.6-pro', { agent_type: 'Codex', display_name: 'opencode-go/MiMo-V2.6-Pro' }),
      model('agent:codex', 'gpt-6-sol-preview', { agent_type: 'Codex', display_name: 'Sol Preview' }),
    ], {
      tier_verdicts: [
        { tier: 'economy', requested_model: 'gpt-5.5-mini', effective_model: 'gpt-5.5-mini', launchable: true },
        { tier: 'default', requested_model: 'gpt-5.5-old', effective_model: 'gpt-5.5', launchable: true },
        { tier: 'reasoning', requested_model: 'gpt-5.5-codex-max', effective_model: null, launchable: true },
      ],
    } as Partial<ModelCatalogView>),
    view('http:openrouter', 'Custom', OPENROUTER_MODELS),
  ],
};

const OPENROUTER: AgentSwitchTarget = {
  agent: 'Custom', connectionId: 'openrouter', label: 'OpenRouter',
  modelTiers: { economy: 'vendor/model-001', default: 'vendor/model-002', reasoning: 'vendor/model-003' },
};
const TARGETS: AgentSwitchTarget[] = [{ agent: 'ClaudeCode' }, { agent: 'Codex' }, { agent: 'GeminiCli' }, OPENROUTER];
const TIER_LABELS: Record<ModelTier, string> = { economy: 'Éco', default: 'Standard', reasoning: 'Avancé' };

function FullList(props: {
  currentAgent?: AgentSwitchTarget['agent'];
  currentConnectionId?: string | null;
  currentTier?: ModelTier;
  model?: string | null;
  onTier?: (target: AgentSwitchTarget, tier: ModelTier) => Promise<void>;
  onModel?: (target: AgentSwitchTarget, model: string) => Promise<void>;
}) {
  return (
    <AgentSwitchPicker
      currentAgent={props.currentAgent ?? 'ClaudeCode'}
      currentConnectionId={props.currentConnectionId}
      currentTier={props.currentTier ?? 'default'}
      availableAgents={[]}
      availableTargets={TARGETS}
      tierLabels={TIER_LABELS}
      defaultModelLabel="Default agent model"
      onTargetSelectionChange={props.onTier ?? vi.fn().mockResolvedValue(undefined)}
      customModel={{ model: props.model ?? null, onSelect: props.onModel ?? vi.fn().mockResolvedValue(undefined) }}
      title="Agent"
      ariaLabel="Agent"
    />
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
const modelText = (row: HTMLElement) => row.querySelector('.kr-agent-switch-mode-model')?.textContent;

beforeEach(() => {
  localStorage.clear();
  list.mockReset().mockResolvedValue(CATALOG);
});
afterEach(() => {
  cleanup();
  setStorageWriteListener(null);
  setUILocale('fr');
});

describe('mode rows write their model (KT-1994)', () => {
  it('each agent shows its header, then its three modes with the model id written in the code face', async () => {
    render(<FullList />);
    await open();
    const claude = group('Claude Code');
    const header = claude.querySelector('.kr-agent-switch-group-header')!;
    expect(header.querySelector('.kr-agent-switch-option-dot')).not.toBeNull();
    expect(header).toHaveTextContent('Claude Code');
    expect(within(header as HTMLElement).getByText('agentPicker.kind.cli')).toBeInTheDocument();
    expect(header.querySelector('.kr-agent-switch-model-count')).toHaveTextContent('agentPicker.modelCount');
    expect(header.querySelector('.kr-agent-switch-model-count')).toHaveAttribute('data-count', '5');
    const modes = ['economy', 'default', 'reasoning'] as const;
    const rows = modes.map(tier => within(claude).getByRole('menuitem', { name: `Claude Code · ${TIER_LABELS[tier]}` }));
    expect(rows.map(modelText)).toEqual(['claude-haiku-4-5', 'claude-sonnet-4-5', 'claude-opus-4-5']);
    for (const row of rows) expect(row.querySelector('.kr-agent-switch-mode-model')?.tagName).toBe('CODE');
    // The current mode carries the check and cannot be chosen again.
    expect(rows[1]).toHaveAttribute('data-current', 'true');
    expect(rows[1]).toBeDisabled();
    expect(rows[1].querySelector('svg')).not.toBeNull();
    // The display name is a secondary text, only when it differs from the id.
    expect(rows[0].querySelector('.kr-agent-switch-mode-name')).toHaveTextContent('Haiku');
  });

  it('a CLI agent takes its modes from the launch verdicts, an HTTP target from its connection', async () => {
    render(<FullList />);
    await open();
    const codex = group('Codex');
    expect(['economy', 'default', 'reasoning'].map(tier =>
      modelText(within(codex).getByRole('menuitem', { name: `Codex · ${TIER_LABELS[tier as ModelTier]}` }))))
      .toEqual(['gpt-5.5-mini', 'gpt-5.5', 'gpt-5.5-codex-max']);
    const openrouter = group('OpenRouter');
    expect(within(openrouter.querySelector('.kr-agent-switch-group-header') as HTMLElement).getByText('agentPicker.kind.api'))
      .toBeInTheDocument();
    expect(modelText(within(openrouter).getByRole('menuitem', { name: 'OpenRouter · Avancé' }))).toBe('vendor/model-003');
  });

  it('a mode without a declared model says « agent default model »', async () => {
    render(<FullList />);
    await open();
    const row = within(group('Gemini CLI')).getByRole('menuitem', { name: 'Gemini CLI · Éco' });
    expect(row.querySelector('.kr-agent-switch-mode-default')).toHaveTextContent('wf.shadow.rows.defaultModel');
    expect(row.querySelector('.kr-agent-switch-mode-model')).toBeNull();
  });

  it('lists the current agent first, then the configured order', async () => {
    render(<FullList currentAgent="Codex" />);
    await open();
    const names = screen.getAllByRole('group')
      .filter(node => node.classList.contains('kr-agent-switch-group-block'))
      .map(node => node.getAttribute('aria-label'));
    expect(names).toEqual(['Codex', 'Claude Code', 'Gemini CLI', 'OpenRouter']);
  });
});

describe('other models: folded, one agent at a time, bounded (KT-1994)', () => {
  it('are folded for every agent; unfolding one agent leaves the others folded', async () => {
    render(<FullList />);
    await open();
    expect(within(group('Claude Code')).queryByRole('menuitem', { name: 'Claude Code · claude-fable-a' })).toBeNull();
    const toggle = within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · agentPicker.otherModels' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    // Only the models no mode points to are « other ».
    expect(toggle.querySelector('.kr-agent-switch-model-count')).toHaveAttribute('data-count', '2');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const fable = within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · claude-fable-a' });
    expect(modelText(fable)).toBe('claude-fable-a');
    expect(fable.querySelector('.kr-agent-switch-mode-name')).toHaveTextContent('Fable');
    expect(within(group('Claude Code')).queryByRole('menuitem', { name: 'Claude Code · claude-sonnet-4-5' })).toBeNull();
    expect(within(group('OpenRouter')).getByRole('menuitem', { name: 'OpenRouter · agentPicker.otherModels' }))
      .toHaveAttribute('aria-expanded', 'false');
    expect(within(group('OpenRouter')).queryByRole('menuitem', { name: 'OpenRouter · vendor/model-050' })).toBeNull();
    // Folded again on the same control.
    fireEvent.click(toggle);
    expect(within(group('Claude Code')).queryByRole('menuitem', { name: 'Claude Code · claude-fable-a' })).toBeNull();
  });

  it(`never mounts more than ${OTHER_MODELS_RENDER_LIMIT} other models per agent; the search reaches the rest`, async () => {
    expect(OTHER_MODELS_RENDER_LIMIT).toBeGreaterThan(0);
    expect(OTHER_MODELS_RENDER_LIMIT).toBeLessThan(117);
    render(<FullList />);
    await open();
    const openrouter = group('OpenRouter');
    fireEvent.click(within(openrouter).getByRole('menuitem', { name: 'OpenRouter · agentPicker.otherModels' }));
    const rendered = openrouter.querySelectorAll('[data-kind="model"]');
    expect(rendered).toHaveLength(OTHER_MODELS_RENDER_LIMIT);
    // 120 models, 3 of them behind a mode: the rest is announced, not mounted.
    const more = openrouter.querySelector('.kr-agent-switch-more')!;
    expect(more).toHaveAttribute('data-count', String(117 - OTHER_MODELS_RENDER_LIMIT));
    expect(within(openrouter).queryByRole('menuitem', { name: 'OpenRouter · vendor/model-119' })).toBeNull();
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'model-119' } });
    expect(within(group('OpenRouter')).getByRole('menuitem', { name: 'OpenRouter · vendor/model-119' })).toBeInTheDocument();
  });

  it('the search finds a folded model without unfolding its agent, and keeps the bound', async () => {
    render(<FullList />);
    await open();
    const search = screen.getByRole('searchbox');
    fireEvent.change(search, { target: { value: 'fable-b' } });
    const claude = group('Claude Code');
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · claude-fable-b' })).toBeInTheDocument();
    expect(within(claude).queryByRole('menuitem', { name: 'Claude Code · claude-fable-a' })).toBeNull();
    // KT-1994 review — no fold control while searching, and no mode the text does not hit.
    expect(within(claude).queryByRole('menuitem', { name: 'Claude Code · agentPicker.otherModels' })).toBeNull();
    expect(within(claude).queryByRole('menuitem', { name: 'Claude Code · Avancé' })).toBeNull();
    // An agent the search does not match is not listed.
    expect(screen.queryByRole('group', { name: 'Codex' })).toBeNull();
    fireEvent.change(search, { target: { value: 'vendor/model' } });
    expect(group('OpenRouter').querySelectorAll('[data-kind="model"]')).toHaveLength(OTHER_MODELS_RENDER_LIMIT);
  });

  // KT-1994 review, point 2 — unfolding at the bottom of the list showed nothing but the chevron.
  it('unfolding scrolls the list so the control is at the top of its viewport; the focus stays on it', async () => {
    render(<FullList />);
    await open();
    const list = screen.getByRole('menu');
    let scrollTop = 40;
    Object.defineProperty(list, 'scrollTop', { configurable: true, get: () => scrollTop, set: value => { scrollTop = value; } });
    list.getBoundingClientRect = () => ({ top: 100, bottom: 400, left: 0, right: 440, width: 440, height: 300 }) as DOMRect;
    const toggle = within(group('OpenRouter')).getByRole('menuitem', { name: 'OpenRouter · agentPicker.otherModels' });
    toggle.getBoundingClientRect = () => ({ top: 380, bottom: 404, left: 0, right: 440, width: 440, height: 24 }) as DOMRect;
    toggle.focus();
    // A keyboard activation is a click with no pointer detail.
    fireEvent.click(toggle, { detail: 0 });
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(scrollTop).toBe(40 + (380 - 100));
    expect(document.activeElement).toBe(toggle);
    // Folding back leaves the scroll where it is.
    scrollTop = 7;
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(scrollTop).toBe(7);
  });

  // KT-1994 review, point 3.
  it('writes a display name only when it says more than the id once case and punctuation are set aside', async () => {
    render(<FullList />);
    await open();
    fireEvent.click(within(group('Codex')).getByRole('menuitem', { name: 'Codex · agentPicker.otherModels' }));
    const codex = group('Codex');
    for (const id of ['gpt-6-luna', 'opencode-go/mimo-v2.6-pro']) {
      expect(within(codex).getByRole('menuitem', { name: `Codex · ${id}` }).querySelector('.kr-agent-switch-mode-name')).toBeNull();
    }
    expect(within(codex).getByRole('menuitem', { name: 'Codex · gpt-6-sol-preview' }).querySelector('.kr-agent-switch-mode-name'))
      .toHaveTextContent('Sol Preview');
    // « Fable » is no id: it stays, on a mode row as on another model.
    expect(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · Éco' }).querySelector('.kr-agent-switch-mode-name'))
      .toHaveTextContent('Haiku');
  });
});

describe('search: only the rows the text hits (KT-1994 review, point 1b)', () => {
  const blocks = () => screen.queryAllByRole('group')
    .filter(node => node.classList.contains('kr-agent-switch-group-block'))
    .map(node => node.getAttribute('aria-label'));

  it('a text that hits one model lists that model alone: no mode row of its agent, no other agent', async () => {
    render(<FullList />);
    await open();
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'model-077' } });
    expect(blocks()).toEqual(['OpenRouter']);
    expect(within(group('OpenRouter')).getAllByRole('menuitem').map(item => item.dataset.kind)).toEqual(['model']);
  });

  it('a mode name lists that mode on every agent; an agent name lists its modes', async () => {
    render(<FullList />);
    await open();
    const search = screen.getByRole('searchbox');
    fireEvent.change(search, { target: { value: 'avance' } });
    expect(blocks()).toEqual(['Claude Code', 'Codex', 'Gemini CLI', 'OpenRouter']);
    for (const name of blocks()) {
      expect(within(group(name!)).getAllByRole('menuitem').map(item => item.dataset.tier)).toEqual(['reasoning']);
    }
    fireEvent.change(search, { target: { value: 'codex' } });
    expect(blocks()).toEqual(['Codex']);
    expect(within(group('Codex')).getAllByRole('menuitem').map(item => item.dataset.tier))
      .toEqual(['economy', 'default', 'reasoning']);
  });

  it('a display name finds its model; a mode is found by its model', async () => {
    render(<FullList />);
    await open();
    const search = screen.getByRole('searchbox');
    fireEvent.change(search, { target: { value: 'Sol Preview' } });
    expect(blocks()).toEqual(['Codex']);
    expect(within(group('Codex')).getByRole('menuitem', { name: 'Codex · gpt-6-sol-preview' })).toBeInTheDocument();
    fireEvent.change(search, { target: { value: 'haiku' } });
    expect(blocks()).toEqual(['Claude Code']);
    expect(within(group('Claude Code')).getAllByRole('menuitem').map(item => item.dataset.tier)).toEqual(['economy']);
  });

  it('when nothing matches, one line says so', async () => {
    render(<FullList />);
    await open();
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'zzz-nothing' } });
    expect(blocks()).toEqual([]);
    expect(screen.getByRole('status')).toHaveTextContent('agentPicker.noMatch');
  });
});

describe('a custom model: chosen, saved, labelled (KT-1994)', () => {
  it('choosing another model saves it as an exact model, never as a mode', async () => {
    const onTier = vi.fn().mockResolvedValue(undefined);
    const onModel = vi.fn().mockResolvedValue(undefined);
    render(<FullList onTier={onTier} onModel={onModel} />);
    await open();
    fireEvent.click(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · agentPicker.otherModels' }));
    await act(async () => {
      fireEvent.click(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · claude-fable-a' }));
    });
    expect(onModel).toHaveBeenCalledWith({ agent: 'ClaudeCode' }, 'claude-fable-a');
    expect(onTier).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog', { name: 'Agent' })).toBeNull();
  });

  it('a held custom model reads « Personnalisé » on the trigger and stays visible under its agent while folded', async () => {
    render(<FullList model="claude-fable-b" />);
    const trigger = screen.getByRole('button', { name: /^Agent( — |$)/ });
    const chip = trigger.querySelector('.kr-agent-switch-custom')!;
    expect(chip).toHaveTextContent('agentPicker.custom');
    expect(trigger.querySelector('.ui-agent-identity-tier')).toBeNull();
    expect(trigger.querySelector('[data-testid="agent-identity-model"]')).toHaveTextContent('claude-fable-b');
    await open();
    const claude = group('Claude Code');
    const custom = within(claude).getByRole('menuitem', { name: 'Claude Code · agentPicker.custom · claude-fable-b' });
    expect(custom).toHaveAttribute('data-current', 'true');
    expect(modelText(custom)).toBe('claude-fable-b');
    expect(within(claude).getByRole('menuitem', { name: 'Claude Code · agentPicker.otherModels' }))
      .toHaveAttribute('aria-expanded', 'false');
    // No mode is checked; the current mode can be chosen again to leave the custom model.
    const standard = within(claude).getByRole('menuitem', { name: 'Claude Code · Standard' });
    expect(standard).toHaveAttribute('data-current', 'false');
    expect(standard).toBeEnabled();
  });

  it('choosing a mode while a custom model is held sends the mode, which clears the model', async () => {
    const onTier = vi.fn().mockResolvedValue(undefined);
    render(<FullList model="claude-fable-b" onTier={onTier} />);
    await open();
    await act(async () => {
      fireEvent.click(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · Standard' }));
    });
    expect(onTier).toHaveBeenCalledWith({ agent: 'ClaudeCode' }, 'default');
  });

  it('a typed id that matches no model is offered as is; a refusal is shown next to it and the choice is kept', async () => {
    const onModel = vi.fn().mockRejectedValue(new Error('The model id must not start with "-"'));
    render(<FullList onModel={onModel} />);
    await open();
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: '  -rogue  ' } });
    const free = screen.getByRole('menuitem', { name: 'agentPicker.freeModel' });
    await act(async () => { fireEvent.click(free); });
    expect(onModel).toHaveBeenCalledWith({ agent: 'ClaudeCode' }, '-rogue');
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('The model id must not start with "-"');
    expect(free.nextElementSibling).toBe(alert);
    // Still open, the previous choice untouched.
    expect(screen.getByRole('dialog', { name: 'Agent' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Agent( — |$)/ }).querySelector('.kr-agent-switch-custom')).toBeNull();
  });

  it('a typed id equal to a model the current agent lists is that model, not a free one', async () => {
    render(<FullList />);
    await open();
    const search = screen.getByRole('searchbox');
    fireEvent.change(search, { target: { value: 'claude-fable-a' } });
    expect(screen.queryByRole('menuitem', { name: /freeModel/ })).toBeNull();
    expect(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · claude-fable-a' })).toBeInTheDocument();
    // A mode's own model counts as listed too.
    fireEvent.change(search, { target: { value: 'claude-sonnet-4-5' } });
    expect(screen.queryByRole('menuitem', { name: /freeModel/ })).toBeNull();
  });

  // KT-1994 review, point 1a — one line at the end, for the current agent only, naming it.
  it('the typed id is one entry at the end of the list, for the current agent, named in it', async () => {
    await loadLocale('fr');
    setUILocale('fr');
    render(<I18nProvider><FullList currentAgent="Codex" /></I18nProvider>);
    await open();
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'neon' } });
    const free = document.querySelectorAll<HTMLElement>('[data-kind="free"]');
    expect(free).toHaveLength(1);
    expect(free[0]).toHaveTextContent('Identifiant absent du catalogue ? Utiliser « neon » avec Codex');
    expect(free[0]).toHaveAccessibleName('Identifiant absent du catalogue ? Utiliser « neon » avec Codex');
    const items = screen.getAllByRole('menuitem');
    expect(items[items.length - 1]).toBe(free[0]);
    expect(screen.queryByText('Autres agents')).toBeNull();
    // A model another agent lists is still free for the current one.
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'vendor/model-010' } });
    expect(document.querySelectorAll('[data-kind="free"]')).toHaveLength(1);
    expect(group('OpenRouter').querySelectorAll('[data-kind="model"]')).toHaveLength(1);
  });
});

describe('« Récents »: the last three choices, in the UI preferences (KT-1994)', () => {
  it('is a synced UI preference in the kronn namespace', () => {
    expect(AGENT_PICKER_RECENTS_KEY.startsWith('kronn:')).toBe(true);
    expect(SYNCED_KEYS).toContain(AGENT_PICKER_RECENTS_KEY);
    expect(isSyncedKey(AGENT_PICKER_RECENTS_KEY, false)).toBe(true);
  });

  it('a choice applied is written, mirrored to the server store, and read first on the next opening', async () => {
    const mirrored = vi.fn();
    setStorageWriteListener(mirrored);
    const onTier = vi.fn().mockResolvedValue(undefined);
    const onModel = vi.fn().mockResolvedValue(undefined);
    render(<FullList onTier={onTier} onModel={onModel} />);
    await open();
    // Nothing chosen yet: no section.
    expect(screen.queryByRole('group', { name: 'agentPicker.recent' })).toBeNull();
    await act(async () => {
      fireEvent.click(within(group('Codex')).getByRole('menuitem', { name: 'Codex · Avancé' }));
    });
    expect(mirrored).toHaveBeenCalledWith(AGENT_PICKER_RECENTS_KEY);
    expect(readAgentPickerRecents()).toEqual([{ agent: 'Codex', tier: 'reasoning' }]);
    await open();
    fireEvent.click(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · agentPicker.otherModels' }));
    await act(async () => {
      fireEvent.click(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · claude-fable-a' }));
    });
    await open();
    const recent = group('agentPicker.recent');
    const rows = within(recent).getAllByRole('menuitem');
    expect(rows.map(row => row.getAttribute('aria-label'))).toEqual([
      'agentPicker.recent: Claude Code · agentPicker.custom · claude-fable-a',
      'agentPicker.recent: Codex · Avancé · gpt-5.5-codex-max',
    ]);
    // On top of the list.
    expect(screen.getByRole('menu').firstElementChild).toBe(recent);
    await act(async () => { fireEvent.click(rows[1]); });
    expect(onTier).toHaveBeenLastCalledWith({ agent: 'Codex' }, 'reasoning');
  });

  it('keeps three distinct choices, most recent first, and hides them while searching', async () => {
    localStorage.setItem(AGENT_PICKER_RECENTS_KEY, JSON.stringify([
      { agent: 'Codex', tier: 'economy' },
      { agent: 'Codex', tier: 'economy' },
      { agent: 'Custom', connectionId: 'openrouter', model: 'vendor/model-077' },
      { agent: 'ClaudeCode', tier: 'reasoning' },
      { agent: 'GeminiCli', tier: 'default' },
      { agent: 'NotAnAgent', tier: 'default' },
    ]));
    render(<FullList />);
    await open();
    const rows = within(group('agentPicker.recent')).getAllByRole('menuitem');
    expect(rows.map(row => row.getAttribute('aria-label'))).toEqual([
      'agentPicker.recent: Codex · Éco · gpt-5.5-mini',
      'agentPicker.recent: OpenRouter · agentPicker.custom · vendor/model-077',
      'agentPicker.recent: Claude Code · Avancé · claude-opus-4-5',
    ]);
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'codex' } });
    expect(screen.queryByRole('group', { name: 'agentPicker.recent' })).toBeNull();
  });
});

describe('the shadow editor: one list, custom labelled (KT-1994)', () => {
  it('modes first, other models after; a model outside the modes reads « Personnalisé »', async () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} title="Agent" ariaLabel="Agent"
        exactModel={{ value: { agent: 'ClaudeCode', model: 'claude-opus-4-5' }, onChange, main: { agent: 'Codex' }, testId: 'exact' }} />,
    );
    await open();
    // The catalogue names Opus as the Avancé mode: a mode, not a custom model.
    const trigger = screen.getByRole('button', { name: /^Agent( — |$)/ });
    expect(trigger.querySelector('.kr-agent-switch-custom')).toBeNull();
    expect(trigger.querySelector('.kr-agent-switch-current-tier')).toHaveAttribute('data-tier', 'reasoning');
    const claude = group('Claude Code');
    const items = within(claude).getAllByRole('menuitem');
    const names = items.map(item => item.getAttribute('aria-label'));
    const indexOf = (name: string) => names.indexOf(name);
    expect(indexOf('Claude Code · disc.tier.reasoning')).toBeGreaterThan(-1);
    expect(indexOf('Claude Code · agentPicker.otherModels')).toBeGreaterThan(indexOf('Claude Code · disc.tier.reasoning'));
    expect(items[indexOf('Claude Code · disc.tier.reasoning')]).toHaveAttribute('data-current', 'true');
    fireEvent.click(items[indexOf('Claude Code · agentPicker.otherModels')]);
    fireEvent.click(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · claude-fable-b' }));
    expect(onChange).toHaveBeenCalledWith({ agent: 'ClaudeCode', model: 'claude-fable-b' });
    // Saved, then shown again from the stored value.
    rerender(
      <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} title="Agent" ariaLabel="Agent"
        exactModel={{ value: { agent: 'ClaudeCode', model: 'claude-fable-b' }, onChange, main: { agent: 'Codex' }, testId: 'exact' }} />,
    );
    expect(screen.getByRole('button', { name: /^Agent( — |$)/ }).querySelector('.kr-agent-switch-custom')).toHaveTextContent('agentPicker.custom');
  });

  it('a mode row picks the model that mode names', async () => {
    const onChange = vi.fn();
    render(
      <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} title="Agent" ariaLabel="Agent"
        exactModel={{ value: undefined, onChange, main: { agent: 'ClaudeCode' }, testId: 'exact' }} />,
    );
    await open();
    fireEvent.click(within(group('Codex')).getByRole('menuitem', { name: 'Codex · disc.tier.economy' }));
    expect(onChange).toHaveBeenCalledWith({ agent: 'Codex', model: 'gpt-5.5-mini' });
  });
});

describe('the composer’s @ menu (KT-1994)', () => {
  const resolution = (id: string, name = id): ResolvedCatalogTier => ({
    configured: id, entry: { model_id: id, display_name: name, last_checked_at: '2026-11-21T00:00:00Z' },
    view: undefined, model: name, unavailable: false, replacement: null, refusal: null, notice: null, provenance: 'live',
  }) as unknown as ResolvedCatalogTier;
  const ids: Record<ModelTier, string> = { economy: 'claude-haiku-4-5', default: 'claude-sonnet-4-5', reasoning: 'claude-opus-4-5' };

  it('writes the model id next to each mode, and offers no other model nor a free id', () => {
    const t = (key: string) => key;
    render(<AgentPickerPalette ariaLabel="Mention" t={t} rows={[{
      key: 'claude', agent: 'ClaudeCode', label: '@claude', onSelect: vi.fn(),
      tiers: {
        targetLabel: '@claude', resolve: tier => resolution(ids[tier], 'Sonnet'), tierLabel: tier => TIER_LABELS[tier],
        modelLabel: tier => ids[tier], defaultLabel: 'default', onSelect: vi.fn(),
      },
    }]} />);
    const option = screen.getByRole('option');
    const tiers = Array.from(option.querySelectorAll<HTMLElement>('.kr-agent-switch-tier-option'));
    expect(tiers.map(tier => tier.querySelector('.kr-agent-switch-tier-model')?.textContent))
      .toEqual(['claude-haiku-4-5', 'claude-sonnet-4-5', 'claude-opus-4-5']);
    expect(tiers[0].querySelector('.kr-agent-switch-tier-model')?.tagName).toBe('CODE');
    expect(within(option).queryByText('agentPicker.otherModels')).toBeNull();
    expect(within(option).queryByText(/agentPicker\.(custom|freeModel)/)).toBeNull();
    expect(option.querySelector('[data-kind="model"], [data-free]')).toBeNull();
  });

  // KT-1994 review, point 5c — a short form in the cell, the full sentence in its title and name.
  it('a mode without a declared model writes « par défaut » in its cell, the full sentence in its title', () => {
    render(<AgentPickerPalette ariaLabel="Mention" t={key => key} rows={[{
      key: 'gemini', agent: 'GeminiCli', label: '@gemini', onSelect: vi.fn(),
      tiers: {
        targetLabel: '@gemini', resolve: () => ({ ...resolution(''), entry: undefined, configured: '', model: '' }) as ResolvedCatalogTier,
        tierLabel: tier => TIER_LABELS[tier], modelLabel: () => 'default', defaultLabel: 'default', onSelect: vi.fn(),
      },
    }]} />);
    for (const tier of screen.getByRole('option').querySelectorAll<HTMLElement>('.kr-agent-switch-tier-option')) {
      expect(tier.querySelector('.kr-agent-switch-tier-model')).toHaveTextContent(/^agentPicker\.defaultShort$/);
      expect(tier.title).toContain('wf.shadow.rows.defaultModel');
      expect(tier.getAttribute('aria-label')).toContain('wf.shadow.rows.defaultModel');
    }
  });

  // KT-1994 review, point 5a — no break inside a token: only after `/` and `:`.
  it('an id breaks only after « / » and « : », its parts unbroken; title and name keep it whole', () => {
    const long: Record<ModelTier, string> = {
      economy: 'opencode-go/mimo-v2.6-pro', default: 'vertex_ai/gemini-3.8-flash', reasoning: 'ollama:qwen3:32b',
    };
    render(<AgentPickerPalette ariaLabel="Mention" t={key => key} rows={[{
      key: 'oc', agent: 'OpenCode', label: '@opencode', onSelect: vi.fn(),
      tiers: {
        targetLabel: '@opencode', resolve: tier => resolution(long[tier], `Name ${tier}`), tierLabel: tier => TIER_LABELS[tier],
        modelLabel: tier => `Name ${tier}`, defaultLabel: 'default', onSelect: vi.fn(),
      },
    }]} />);
    const cells = Array.from(screen.getByRole('option').querySelectorAll<HTMLElement>('.kr-agent-switch-tier-option'));
    const parts = (cell: HTMLElement) => Array.from(cell.querySelectorAll('.kr-agent-switch-id-part')).map(part => part.textContent);
    expect(parts(cells[0])).toEqual(['opencode-go/', 'mimo-v2.6-pro']);
    expect(parts(cells[1])).toEqual(['vertex_ai/', 'gemini-3.8-flash']);
    expect(parts(cells[2])).toEqual(['ollama:', 'qwen3:', '32b']);
    expect(cells[2].querySelectorAll('wbr')).toHaveLength(2);
    cells.forEach((cell, index) => {
      const id = Object.values(long)[index];
      expect(cell.querySelector('.kr-agent-switch-tier-model')?.textContent).toBe(id);
      expect(cell.title).toContain(id);
      expect(cell.getAttribute('aria-label')).toContain(id);
    });
  });
});

describe('the trigger writes the model id, catalogue loaded or not (KT-1994 review, point 4)', () => {
  it('a configured mode: the id before and after the list opens; the display name in the title', async () => {
    list.mockResolvedValue({ targets: [view('http:openrouter', 'Custom', [
      model('http:openrouter', 'vendor/model-002', { agent_type: 'Custom', display_name: 'Vendor Two' }),
    ])] });
    render(<FullList currentAgent="Custom" currentConnectionId="openrouter" />);
    const trigger = screen.getByRole('button', { name: /^Agent( — |$)/ });
    const suffix = () => trigger.querySelector('[data-testid="agent-identity-model"]');
    expect(list).not.toHaveBeenCalled();
    expect(suffix()).toHaveTextContent(/^vendor\/model-002$/);
    await open();
    expect(suffix()).toHaveTextContent(/^vendor\/model-002$/);
    expect(trigger).toHaveAttribute('title', 'Agent · Standard · Vendor Two');
  });

  it('a mode the configuration leaves to the agent keeps its fallback once the catalogue names a model (compact too)', async () => {
    render(
      <AgentSwitchPicker currentAgent="ClaudeCode" currentTier="default" availableAgents={[]} availableTargets={TARGETS}
        tierLabels={TIER_LABELS} onTargetSelectionChange={vi.fn()} currentModelFallback="Default model of Claude Code"
        compact title="Agent" ariaLabel="Agent" />,
    );
    const trigger = screen.getByRole('button', { name: /^Agent( — |$)/ });
    const suffix = () => trigger.querySelector('[data-testid="agent-identity-model"]');
    expect(suffix()).toHaveTextContent(/^Default model of Claude Code$/);
    await open();
    // The catalogue names Sonnet for the Standard mode: the list says so, the trigger does not move.
    expect(modelText(within(group('Claude Code')).getByRole('menuitem', { name: 'Claude Code · Standard' }))).toBe('claude-sonnet-4-5');
    expect(suffix()).toHaveTextContent(/^Default model of Claude Code$/);
  });
});

describe('an unavailable agent (KT-1994 review, point 6c)', () => {
  const refused = (detail: string) => (['economy', 'default', 'reasoning'] as const).map(tier => ({
    tier, requested_model: null, effective_model: null, launchable: false, detail,
  }));

  it('stays listed, not selectable, with its reason in one muted line under its header', async () => {
    list.mockResolvedValue({ targets: [
      ...CATALOG.targets,
      view('agent:gemini-cli', 'GeminiCli', [], { tier_verdicts: refused('Gemini CLI is not signed in') } as Partial<ModelCatalogView>),
    ] });
    render(<FullList />);
    await open();
    const gemini = group('Gemini CLI');
    expect(within(gemini).getByTestId('agent-unavailable')).toBeInTheDocument();
    const reason = within(gemini).getByTestId('agent-switch-reason');
    expect(reason).toHaveTextContent(/^Gemini CLI is not signed in$/);
    expect(reason).toHaveClass('kr-agent-switch-reason');
    expect(reason.previousElementSibling).toHaveClass('kr-agent-switch-group-header');
    for (const row of within(gemini).getAllByRole('menuitem')) expect(row).toBeDisabled();
    expect(within(group('Claude Code')).queryByTestId('agent-switch-reason')).toBeNull();
  });

  it('writes the caller’s reason the same way', async () => {
    render(
      <AgentSwitchPicker currentAgent="ClaudeCode" currentTier="default" availableAgents={[]} availableTargets={TARGETS}
        tierLabels={TIER_LABELS} onTargetSelectionChange={vi.fn()} title="Agent" ariaLabel="Agent"
        availability={target => target.agent === 'Codex'
          ? { state: 'unavailable', label: 'Unavailable', title: 'Codex is not installed' } : null} />,
    );
    await open();
    expect(within(group('Codex')).getByTestId('agent-switch-reason')).toHaveTextContent('Codex is not installed');
  });
});
