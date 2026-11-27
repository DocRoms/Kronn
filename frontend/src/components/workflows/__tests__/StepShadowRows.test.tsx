// KT-1696 — the shadows of an Agent step as sub-rows: agent/model picker with
// its transport badge, the Active and Merge toggles, ✕ and + Shadow, the summary
// of the merged result with its `?` popover.
// KT-1707 — the block opens with a highlighted Principal row; every row has the same
// `role · agent · model [badge]` structure, and the shadow's free label is a tooltip.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { CatalogModelEntry, ShadowMerge, StepShadow, WorkflowStep } from '../../../types/generated';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../../lib/api', () => buildApiMock({ modelCatalogApi: { list: list as never } }));

import { StepShadowRows } from '../StepShadowRows';
import { hasShadowBlock } from '../shadowRowModel';
import type { ShadowEdit } from '../../../lib/shadowMerge';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}(${args.join('|')})` : key);

const shadow = (id: string, extra: Partial<StepShadow> = {}): StepShadow => ({ id, label: '', enabled: true, ...extra });
const step = (extra: Partial<WorkflowStep> = {}): WorkflowStep => ({
  name: 'advise', agent: 'Ollama', step_type: { type: 'Agent' }, agent_settings: { model: 'qwen3.8:27b-mlx' }, skill_ids: [], ...extra,
}) as unknown as WorkflowStep;
const merge = (extra: Partial<ShadowMerge> = {}): ShadowMerge => ({ enabled: true, ...extra });

const argon = shadow('argon', { label: 'Argon', agent: { agent: 'LiteLlm', model: 'gemini-4-argon', connection_id: 'lite-1' } });
const twin = shadow('twin', { label: 'Qwen identique' });
const choices = [
  { agent: 'Ollama' as const },
  { agent: 'LiteLlm' as const, connectionId: 'lite-1', label: 'LiteLLM' },
  { agent: 'ClaudeCode' as const },
];

function entry(model: string, target: string, agent: CatalogModelEntry['agent_type']): CatalogModelEntry {
  return { id: model, runtime_target_id: target, agent_type: agent, model_id: model, display_name: model, tier_assignment: null,
    provenance: 'live', availability: 'available', capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
    first_seen_at: '2026-09-09T00:00:00Z', last_checked_at: '2026-09-09T00:00:00Z', created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z' };
}

beforeEach(() => {
  list.mockReset().mockResolvedValue({ targets: [
    { runtime_target_id: 'agent:ollama', agent_type: 'Ollama', stale: false, live_refresh_ok: true,
      models: [entry('qwen3.8:27b-mlx', 'agent:ollama', 'Ollama'), entry('llama4:8b', 'agent:ollama', 'Ollama')] },
    { runtime_target_id: 'http:lite-1', agent_type: 'LiteLlm', stale: false, live_refresh_ok: true,
      models: [entry('gemini-4-argon', 'http:lite-1', 'LiteLlm')] },
  ] });
});
afterEach(cleanup);

/** KT-1994 — an agent's other models are folded under one control. */
async function unfold(menu: HTMLElement, agent: string) {
  fireEvent.click(await within(menu).findByRole('menuitem', { name: `${agent} · agentPicker.otherModels` }));
}

function renderRows(target: WorkflowStep, extra: Partial<Parameters<typeof StepShadowRows>[0]> = {}) {
  const edits: ShadowEdit[] = [];
  render(<StepShadowRows step={target} nextStepName="publish" choices={choices} onEdit={edit => edits.push(edit)} t={t} {...extra} />);
  return edits;
}

const withShadows = (extra: Partial<WorkflowStep> = {}) =>
  step({ shadows: [argon, twin], shadow_merge: merge({ shadows: ['argon'], timeout_secs: 600 }), ...extra });

describe('StepShadowRows — the sub-rows', () => {
  it('shows a Shadow row per shadow: role, agent · model, transport badge — and no free label', () => {
    renderRows(withShadows());
    const rowArgon = screen.getByTestId('wf-shadow-rows-argon');
    expect(within(rowArgon).getByText('wf.shadow.rows.roleShadow')).toBeInTheDocument();
    expect(within(rowArgon).getByTestId('wf-shadow-rows-argon-agent')).toHaveTextContent('LiteLLM · gemini-4-argon');
    expect(within(rowArgon).getByText('wf.shadow.rows.transport.http')).toBeInTheDocument();
    expect(rowArgon).not.toHaveTextContent('Argon');
    // The free label stays reachable as the row's tooltip.
    expect(rowArgon).toHaveAttribute('title', 'Argon');
    const rowTwin = screen.getByTestId('wf-shadow-rows-twin');
    expect(rowTwin).not.toHaveTextContent('Qwen identique');
    expect(rowTwin).toHaveAttribute('title', 'Qwen identique');
    // Without an agent of its own the shadow shows the principal's, resolved, with a muted « = principal ».
    const twinAgent = within(rowTwin).getByTestId('wf-shadow-rows-twin-agent');
    expect(twinAgent).toHaveTextContent('Ollama · qwen3.8:27b-mlx');
    expect(twinAgent).toHaveTextContent('wf.shadow.rows.sameAgent');
    expect(within(rowArgon).getByTestId('wf-shadow-rows-argon-agent')).not.toHaveTextContent('wf.shadow.rows.sameAgent');
    // The identical shadow runs on the step's own agent: Ollama is a local runtime.
    expect(within(rowTwin).getByText('wf.shadow.rows.transport.local')).toBeInTheDocument();
  });

  it('labels the two one-click toggles, and states their state', () => {
    renderRows(withShadows());
    const active = screen.getByTestId('wf-shadow-rows-argon-active');
    const merged = screen.getByTestId('wf-shadow-rows-argon-merge');
    expect(active).toHaveAttribute('role', 'switch');
    expect(active).toHaveTextContent('wf.shadow.rows.active');
    expect(merged).toHaveTextContent('wf.shadow.rows.merge');
    expect(active).toHaveAttribute('aria-checked', 'true');
    expect(merged).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('wf-shadow-rows-twin-merge')).toHaveAttribute('aria-checked', 'false');
  });

  it('explains both toggles in their tooltips, with the shadow and the next step by name', () => {
    renderRows(withShadows());
    expect(screen.getByTestId('wf-shadow-rows-twin-active')).toHaveAttribute('title', 'wf.shadow.rows.activeTip');
    expect(screen.getByTestId('wf-shadow-rows-twin-merge')).toHaveAttribute('title', 'wf.shadow.rows.mergeTip(Ollama · qwen3.8:27b-mlx|publish)');
  });

  it('Merge on a shadow that is not merged writes an explicit list', () => {
    const edits = renderRows(withShadows());
    fireEvent.click(screen.getByTestId('wf-shadow-rows-twin-merge'));
    expect(edits).toHaveLength(1);
    expect(edits[0].merge).toEqual({ enabled: true, shadows: ['argon', 'twin'], timeout_secs: 600 });
  });

  it('Merge on a paused shadow also activates it', () => {
    const paused = shadow('twin', { label: 'Qwen identique', enabled: false });
    const edits = renderRows(step({ shadows: [argon, paused], shadow_merge: merge({ shadows: ['argon'] }) }));
    expect(screen.getByTestId('wf-shadow-rows-twin')).toHaveAttribute('data-paused', 'true');
    fireEvent.click(screen.getByTestId('wf-shadow-rows-twin-merge'));
    expect(edits[0].shadows.find(s => s.id === 'twin')?.enabled).toBe(true);
    expect(edits[0].merge).toEqual({ enabled: true, shadows: ['argon', 'twin'] });
  });

  it('turning off the last merged shadow switches the merge off and keeps the condense agent and timeout', () => {
    const agent = { agent: 'ClaudeCode' as const };
    const edits = renderRows(withShadows({ shadow_merge: merge({ shadows: ['argon'], timeout_secs: 600, agent }) }));
    fireEvent.click(screen.getByTestId('wf-shadow-rows-argon-merge'));
    expect(edits[0].merge).toEqual({ enabled: false, shadows: [], timeout_secs: 600, agent });
  });

  it('Active pauses a shadow that is not merged; a merged shadow always runs', () => {
    const edits = renderRows(withShadows());
    fireEvent.click(screen.getByTestId('wf-shadow-rows-twin-active'));
    expect(edits[0].shadows.find(s => s.id === 'twin')?.enabled).toBe(false);
    const locked = screen.getByTestId('wf-shadow-rows-argon-active');
    expect(locked).toBeDisabled();
    expect(locked).toHaveAttribute('title', 'wf.shadow.rows.activeLocked');
  });

  it('removes a shadow with ✕', () => {
    const edits = renderRows(withShadows());
    fireEvent.click(screen.getByTestId('wf-shadow-rows-twin-remove'));
    expect(edits[0].shadows.map(s => s.id)).toEqual(['argon']);
    expect(edits[0].merge).toEqual({ enabled: true, shadows: ['argon'], timeout_secs: 600 });
  });

  it('is read-only without a handler: nothing to click, nothing to remove', () => {
    render(<StepShadowRows step={withShadows()} choices={choices} t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-twin-merge')).toBeDisabled();
    expect(screen.getByTestId('wf-shadow-rows-twin-active')).toBeDisabled();
    expect(screen.getByTestId('wf-shadow-rows-twin-agent')).toBeDisabled();
    expect(screen.queryByTestId('wf-shadow-rows-twin-remove')).toBeNull();
    expect(screen.queryByTestId('wf-shadow-rows-add')).toBeNull();
  });

  it('renders nothing for a step that cannot carry shadows, or has none', () => {
    const { container } = render(<StepShadowRows step={step({ external_effect: true, shadows: [twin] })} t={t} />);
    expect(container).toBeEmptyDOMElement();
    cleanup();
    const none = render(<StepShadowRows step={step()} onEdit={vi.fn()} t={t} />);
    expect(none.container).toBeEmptyDOMElement();
    none.unmount();
    render(<StepShadowRows step={step()} onEdit={vi.fn()} addWhenEmpty t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-add')).toBeInTheDocument();
  });
});

const rowsCss = readFileSync(resolve(process.cwd(), 'src/components/workflows/StepShadowRows.css'), 'utf8');
const ruleOf = (selector: string) => rowsCss.match(new RegExp(`^${selector.replace('.', '\\.')}\\s*\\{([^}]*)\\}`, 'm'))?.[1] ?? '';

describe('StepShadowRows — the Principal row', () => {
  it('opens the block: first row, highlighted, same structure as a shadow row', () => {
    renderRows(withShadows());
    const rows = screen.getByTestId('wf-shadow-rows');
    const principal = screen.getByTestId('wf-shadow-rows-principal');
    expect(rows.firstElementChild).toBe(principal);
    expect(principal).toHaveAttribute('data-role', 'principal');
    expect(principal).toHaveClass('wf-shadow-subrow', 'wf-shadow-principal');
    expect(principal.children[0]).toHaveTextContent('wf.shadow.rows.principal');
    expect(within(principal).getByTestId('wf-shadow-rows-principal-agent')).toHaveTextContent('Ollama · qwen3.8:27b-mlx');
    expect(within(principal).getByText('wf.shadow.rows.transport.local')).toBeInTheDocument();
    // A shadow row has the same first columns, then comes after it.
    const argon = screen.getByTestId('wf-shadow-rows-argon');
    expect(argon).toHaveClass('wf-shadow-subrow');
    expect(principal.compareDocumentPosition(argon) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(argon.children[0]).toHaveTextContent('wf.shadow.rows.roleShadow');
    expect(argon).toHaveAttribute('data-role', 'shadow');
  });

  it('is always active: a chip, no Active / Merge toggle, no ✕', () => {
    renderRows(withShadows());
    const principal = screen.getByTestId('wf-shadow-rows-principal');
    expect(within(principal).getByTestId('wf-shadow-rows-principal-active')).toHaveTextContent('wf.shadow.rows.alwaysActive');
    expect(within(principal).queryByRole('switch')).toBeNull();
    expect(within(principal).queryByRole('button')).toBeNull();
  });

  it('says « base of the merge » only while a merge is on', () => {
    const { rerender } = render(<StepShadowRows step={withShadows()} nextStepName="publish" choices={choices} onEdit={vi.fn()} t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-principal-base')).toHaveTextContent('wf.shadow.rows.mergeBase');
    rerender(<StepShadowRows step={withShadows({ shadow_merge: merge({ enabled: false, shadows: [] }) })} nextStepName="publish" choices={choices} onEdit={vi.fn()} t={t} />);
    expect(screen.queryByTestId('wf-shadow-rows-principal-base')).toBeNull();
    expect(screen.getByTestId('wf-shadow-rows-principal')).toBeInTheDocument();
  });

  it('hosts the step\'s own picker when given one, in place of the read-only label', () => {
    renderRows(withShadows(), { mainPicker: <button type="button" data-testid="own-picker">picker</button> });
    expect(within(screen.getByTestId('wf-shadow-rows-principal')).getByTestId('own-picker')).toBeInTheDocument();
  });

  it('appears with the first shadow and goes with the last one', () => {
    const { rerender } = render(<StepShadowRows step={step()} onEdit={vi.fn()} addWhenEmpty t={t} />);
    expect(screen.queryByTestId('wf-shadow-rows-principal')).toBeNull();
    expect(hasShadowBlock(step())).toBe(false);
    rerender(<StepShadowRows step={step({ shadows: [twin] })} onEdit={vi.fn()} addWhenEmpty t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-principal')).toBeInTheDocument();
    expect(hasShadowBlock(step({ shadows: [twin] }))).toBe(true);
    rerender(<StepShadowRows step={step({ shadows: [] })} onEdit={vi.fn()} addWhenEmpty t={t} />);
    expect(screen.queryByTestId('wf-shadow-rows-principal')).toBeNull();
    expect(screen.getByTestId('wf-shadow-rows-add')).toBeInTheDocument();
  });

  it('is teal, the shadows violet and indented, in the same grid', () => {
    expect(ruleOf('.wf-shadow-principal')).toMatch(/--kr-cyan-rgb/);
    expect(ruleOf('.wf-shadow-subrow')).toMatch(/grid-template-columns/);
    expect(ruleOf('.wf-shadow-subrow')).toMatch(/--kr-purple-rgb/);
    expect(rowsCss).toMatch(/\.wf-shadow-subrow\[data-role='shadow'\]\s*\{\s*margin-left:/);
    expect(rowsCss).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
  });
});

describe('StepShadowRows — the agent/model menu', () => {
  const open = async (id: string) => {
    await act(async () => { fireEvent.click(screen.getByTestId(id)); });
    return screen.getByTestId(`${id}-menu`);
  };

  it('opens on one click, grouped by agent with transport, and includes « = principal »', async () => {
    renderRows(withShadows());
    const menu = await open('wf-shadow-rows-twin-agent');
    expect(within(menu).getByTestId('wf-shadow-rows-twin-agent-same')).toBeInTheDocument();
    const groups = within(menu).getAllByRole('group').map(group => group.getAttribute('aria-label'));
    expect(groups).toEqual(['Ollama', 'LiteLLM', 'Claude Code']);
    // KT-1994 — the catalogue's models no mode points to are folded under « Autres modèles ».
    expect(within(menu).queryByRole('menuitem', { name: 'Ollama · llama4:8b' })).toBeNull();
    await unfold(menu, 'Ollama');
    expect(within(within(menu).getByRole('group', { name: 'LiteLLM' })).getByText('agentPicker.kind.api')).toBeInTheDocument();
    expect(within(within(menu).getByRole('group', { name: 'Ollama' })).getByText('agentPicker.kind.local')).toBeInTheDocument();
    expect(within(within(menu).getByRole('group', { name: 'Claude Code' })).getByText('agentPicker.kind.cli')).toBeInTheDocument();
    await waitFor(() => expect(within(menu).getByRole('menuitem', { name: 'Ollama · llama4:8b' })).toBeInTheDocument());
  });

  it('greys out the main run\'s model', async () => {
    renderRows(withShadows());
    const menu = await open('wf-shadow-rows-twin-agent');
    await unfold(menu, 'Ollama');
    const main = await within(menu).findByRole('menuitem', { name: 'Ollama · qwen3.8:27b-mlx' });
    expect(main).toBeDisabled();
    expect(main).toHaveAttribute('data-main', 'true');
    expect(within(menu).getByRole('menuitem', { name: 'Ollama · llama4:8b' })).toBeEnabled();
  });

  it('picks an agent and model for the shadow', async () => {
    const edits = renderRows(withShadows());
    const menu = await open('wf-shadow-rows-twin-agent');
    await unfold(menu, 'Ollama');
    fireEvent.click(await within(menu).findByRole('menuitem', { name: 'Ollama · llama4:8b' }));
    expect(edits[0].shadows.find(s => s.id === 'twin')?.agent).toEqual({ agent: 'Ollama', model: 'llama4:8b' });
    expect(screen.queryByTestId('wf-shadow-rows-twin-agent-menu')).toBeNull();
  });

  it('picks a connection: the shadow keeps its connection id', async () => {
    const edits = renderRows(withShadows());
    const menu = await open('wf-shadow-rows-twin-agent');
    await unfold(menu, 'LiteLLM');
    fireEvent.click(await within(menu).findByRole('menuitem', { name: 'LiteLLM · gemini-4-argon' }));
    expect(edits[0].shadows.find(s => s.id === 'twin')?.agent).toEqual({ agent: 'LiteLlm', model: 'gemini-4-argon', connection_id: 'lite-1' });
  });

  it('« = principal » makes the shadow identical', async () => {
    const edits = renderRows(withShadows());
    const menu = await open('wf-shadow-rows-argon-agent');
    fireEvent.click(within(menu).getByTestId('wf-shadow-rows-argon-agent-same'));
    expect(edits[0].shadows.find(s => s.id === 'argon')?.agent).toBeUndefined();
  });

  it('+ Shadow opens the same menu and creates a line with the chosen agent and model', async () => {
    const edits = renderRows(withShadows());
    const menu = await open('wf-shadow-rows-add');
    await unfold(menu, 'Ollama');
    fireEvent.click(await within(menu).findByRole('menuitem', { name: 'Ollama · llama4:8b' }));
    expect(edits[0].shadows.map(s => s.id)).toEqual(['argon', 'twin', 'shadow-3']);
    expect(edits[0].shadows[2]).toEqual({ id: 'shadow-3', label: '', enabled: true, agent: { agent: 'Ollama', model: 'llama4:8b' } });
    // The new line is compared only until its Merge is turned on.
    expect(edits[0].merge).toEqual({ enabled: true, shadows: ['argon'], timeout_secs: 600 });
  });

  // KT-1994 — a shadow's model that no mode of its agent names reads « Personnalisé », from its saved value.
  it('labels a saved model outside the modes « Personnalisé » and lists it, checked, under its agent while folded', async () => {
    renderRows(withShadows());
    const trigger = screen.getByTestId('wf-shadow-rows-argon-agent');
    expect(trigger.querySelector('.kr-agent-switch-custom')).toHaveTextContent('agentPicker.custom');
    const menu = await open('wf-shadow-rows-argon-agent');
    const lite = within(menu).getByRole('group', { name: 'LiteLLM' });
    expect(within(lite).getByRole('menuitem', { name: 'LiteLLM · agentPicker.custom · gemini-4-argon' }))
      .toHaveAttribute('data-current', 'true');
    expect(within(lite).getByRole('menuitem', { name: 'LiteLLM · agentPicker.otherModels' })).toHaveAttribute('aria-expanded', 'false');
  });

  it('closes on Escape', async () => {
    renderRows(withShadows());
    await open('wf-shadow-rows-twin-agent');
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByTestId('wf-shadow-rows-twin-agent-menu')).toBeNull();
  });
});

describe('StepShadowRows — the merged result', () => {
  it('summarises main + the merged shadows → the next step, with the condense agent', () => {
    renderRows(withShadows());
    const summary = screen.getByTestId('wf-shadow-rows-merged');
    expect(summary).toHaveTextContent('wf.shadow.rows.summary(wf.shadow.rows.shadowNamed(LiteLLM · gemini-4-argon)|publish)');
    expect(summary).toHaveTextContent('wf.shadow.rows.mergedBy');
    expect(within(summary).getByTestId('wf-shadow-rows-condense')).toHaveTextContent('wf.shadow.rows.sameAgent');
    expect(within(summary).getByTestId('wf-shadow-rows-condense')).toHaveTextContent('Ollama · qwen3.8:27b-mlx');
  });

  it('keeps « merged by » and its picker in one group that never splits', () => {
    renderRows(withShadows());
    const group = screen.getByTestId('wf-shadow-rows-merged-agent');
    expect(group.parentElement).toBe(screen.getByTestId('wf-shadow-rows-merged'));
    expect(within(group).getByText('wf.shadow.rows.mergedBy')).toBeInTheDocument();
    expect(within(group).getByTestId('wf-shadow-rows-condense')).toBeInTheDocument();
    const rule = ruleOf('.wf-shadow-merged-agent');
    expect(rule).toMatch(/flex-wrap:\s*nowrap/);
    expect(rule).toMatch(/white-space:\s*nowrap/);
    // The summary row wraps whole groups, so the unit moves as one when space is missing.
    expect(ruleOf('.wf-shadow-merged')).toMatch(/flex-wrap:\s*wrap/);
  });

  it('names every merged shadow, and says « the step result » on the last step', () => {
    renderRows(withShadows({ shadow_merge: merge({ shadows: ['argon', 'twin'] }) }), { nextStepName: undefined });
    expect(screen.getByTestId('wf-shadow-rows-merged')).toHaveTextContent('wf.shadow.rows.summaryLast(wf.shadow.rows.shadowNamed(LiteLLM · gemini-4-argon) + wf.shadow.rows.shadowNamed(Ollama · qwen3.8:27b-mlx))');
  });

  it('is not a setting for the shadows, and disappears when nothing is merged', () => {
    const { rerender } = render(<StepShadowRows step={withShadows()} nextStepName="publish" choices={choices} onEdit={vi.fn()} t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-merged')).toBeInTheDocument();
    rerender(<StepShadowRows step={withShadows({ shadow_merge: merge({ enabled: false, shadows: [], timeout_secs: 600 }) })} nextStepName="publish" choices={choices} onEdit={vi.fn()} t={t} />);
    expect(screen.queryByTestId('wf-shadow-rows-merged')).toBeNull();
    rerender(<StepShadowRows step={withShadows({ shadow_merge: undefined })} nextStepName="publish" choices={choices} onEdit={vi.fn()} t={t} />);
    expect(screen.queryByTestId('wf-shadow-rows-merged')).toBeNull();
  });

  it('changes the agent that merges', async () => {
    const edits = renderRows(withShadows());
    await act(async () => { fireEvent.click(screen.getByTestId('wf-shadow-rows-condense')); });
    const menu = screen.getByTestId('wf-shadow-rows-condense-menu');
    await unfold(menu, 'Ollama');
    fireEvent.click(await within(menu).findByRole('menuitem', { name: 'Ollama · llama4:8b' }));
    expect(edits[0].merge).toEqual({ enabled: true, shadows: ['argon'], timeout_secs: 600, agent: { agent: 'Ollama', model: 'llama4:8b' } });
  });

  describe('the ? popover', () => {
    it('opens on hover and closes when the pointer leaves', () => {
      renderRows(withShadows());
      const help = screen.getByTestId('wf-shadow-rows-help');
      expect(screen.queryByTestId('wf-shadow-rows-help-pop')).toBeNull();
      fireEvent.mouseEnter(help.parentElement as HTMLElement);
      const pop = screen.getByTestId('wf-shadow-rows-help-pop');
      expect(pop).toHaveTextContent('wf.shadow.rows.helpOne');
      expect(pop).toHaveTextContent('wf.shadow.rows.helpFlow(wf.shadow.rows.shadowNamed(LiteLLM · gemini-4-argon)|publish)');
      expect(pop).toHaveTextContent('wf.shadow.rows.helpDedupe');
      expect(pop).toHaveTextContent('wf.shadow.rows.helpCompared');
      // timeout_secs 600 shows as minutes.
      expect(pop).toHaveTextContent('wf.shadow.rows.helpWait(10)');
      fireEvent.mouseLeave(help.parentElement as HTMLElement);
      expect(screen.queryByTestId('wf-shadow-rows-help-pop')).toBeNull();
    });

    it('opens on tap and stays until tapped again, outside, or Escape (touch has no hover)', () => {
      renderRows(withShadows());
      const help = screen.getByTestId('wf-shadow-rows-help');
      fireEvent.click(help);
      expect(screen.getByTestId('wf-shadow-rows-help-pop')).toBeInTheDocument();
      expect(help).toHaveAttribute('aria-expanded', 'true');
      fireEvent.click(help);
      expect(screen.queryByTestId('wf-shadow-rows-help-pop')).toBeNull();
      fireEvent.click(help);
      fireEvent.pointerDown(document.body);
      expect(screen.queryByTestId('wf-shadow-rows-help-pop')).toBeNull();
      fireEvent.click(help);
      fireEvent.keyDown(document, { key: 'Escape' });
      expect(screen.queryByTestId('wf-shadow-rows-help-pop')).toBeNull();
    });

    it('uses the default wait when the merge sets none, rounded up to whole minutes', () => {
      const { unmount } = render(<StepShadowRows step={withShadows({ shadow_merge: merge({ shadows: ['argon'] }) })} t={t} />);
      fireEvent.click(screen.getByTestId('wf-shadow-rows-help'));
      expect(screen.getByTestId('wf-shadow-rows-help-pop')).toHaveTextContent('wf.shadow.rows.helpWait(10)');
      unmount();
      render(<StepShadowRows step={withShadows({ shadow_merge: merge({ shadows: ['argon'], timeout_secs: 90 }) })} t={t} />);
      fireEvent.click(screen.getByTestId('wf-shadow-rows-help'));
      expect(screen.getByTestId('wf-shadow-rows-help-pop')).toHaveTextContent('wf.shadow.rows.helpWait(2)');
    });
  });
});
