// KT-1885 — one agent picker, four modes (field, compact chip, multi-choice,
// exact model) plus the palette the composer's @ menu drives. Wherever it is
// used it opens above any dialog, on its search field, and shows availability
// the same way.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());

import {
  AgentPickerPalette, AgentSwitchPicker, type AgentAvailability, type AgentSwitchTarget,
} from '../AgentSwitchPicker';
import { Dialog } from '../ui/Dialog';
import { I18nProvider } from '../../lib/I18nContext';
import { loadLocale, setUILocale } from '../../lib/i18n';
import { agentPaletteFieldAria, agentPaletteOptionId } from '../../lib/agentPickerPalette';

afterEach(() => {
  cleanup();
  setUILocale('fr');
});

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const TARGETS: AgentSwitchTarget[] = [{ agent: 'ClaudeCode' }, { agent: 'Codex' }, { agent: 'GeminiCli' }];
const unavailable = (target: AgentSwitchTarget): AgentAvailability | null => target.agent === 'GeminiCli'
  ? { state: 'unavailable', label: 'Not installed' }
  : null;
const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}(${args.join('|')})` : key);

/** Each mode of the picker, with the same three agents and Gemini CLI unavailable. */
const MODES = {
  tiers: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" currentTier="default" availableAgents={[]} availableTargets={TARGETS}
      availability={unavailable} onTargetSelectionChange={vi.fn()} variant="field" title="Agent" ariaLabel="Agent" />
  ),
  chip: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS}
      availability={unavailable} onChange={vi.fn()} compact title="Agent" ariaLabel="Agent" />
  ),
  multiple: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} availability={unavailable}
      multiple={{ selected: [{ agent: 'ClaudeCode' }], onToggle: vi.fn() }} title="Agent" ariaLabel="Agent" />
  ),
  exact: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} availability={unavailable}
      title="Agent" ariaLabel="Agent"
      exactModel={{ value: undefined, onChange: vi.fn(), main: { agent: 'ClaudeCode' }, testId: 'exact' }} />
  ),
} as const;

async function open() {
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: /^Agent( — |$)/ })); });
  return screen.getByRole('dialog', { name: 'Agent' });
}

describe('AgentSwitchPicker — above any dialog, on its search field (KT-1885)', () => {
  it.each(Object.keys(MODES) as (keyof typeof MODES)[])('%s: opens in <body> above a modal dialog, focused on its search', async mode => {
    const onClose = vi.fn();
    const Picker = MODES[mode];
    render(<Dialog open onClose={onClose} title="Settings"><Picker /></Dialog>);
    const modal = screen.getByRole('dialog', { name: 'Settings' });
    const popover = await open();
    // Rendered into <body>, outside the dialog: no overflow of the dialog clips it.
    expect(popover.parentElement).toBe(document.body);
    expect(modal.contains(popover)).toBe(false);
    expect(within(popover).getByRole('searchbox')).toHaveFocus();
    // Escape on the empty search closes the picker only, and focus goes back to its trigger.
    fireEvent.keyDown(within(popover).getByRole('searchbox'), { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'Agent' })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: /^Agent( — |$)/ })).toHaveFocus();
  });

  it('stacks its popover on the popover layer, above every overlay layer', () => {
    const css = readFileSync(join(SRC, 'components', 'AgentSwitchPicker.css'), 'utf8');
    const rule = css.slice(css.indexOf('.kr-agent-switch-popover {'));
    expect(rule.slice(0, rule.indexOf('}'))).toContain('z-index: var(--kr-z-popover)');
    const tokens = readFileSync(join(SRC, 'styles', 'tokens.css'), 'utf8');
    const layer = (name: string) => Number(tokens.match(new RegExp(`--kr-z-${name}:\\s*(\\d+)`))?.[1]);
    expect(layer('popover')).toBeGreaterThan(layer('modal-nested'));
    expect(layer('popover')).toBeGreaterThan(layer('modal'));
    expect(layer('popover')).toBeGreaterThan(layer('drawer'));
  });
});

describe('AgentSwitchPicker — availability reads the same in every mode (KT-1885)', () => {
  it.each(Object.keys(MODES) as (keyof typeof MODES)[])('%s: an unavailable agent wears the shared badge and cannot be chosen', async mode => {
    const Picker = MODES[mode];
    render(<Picker />);
    const popover = await open();
    const badge = within(popover).getByTestId('agent-unavailable');
    expect(badge).toHaveTextContent('Not installed');
    expect(badge).toHaveAttribute('data-state', 'unavailable');
    expect(badge).toHaveClass('kr-agent-switch-badge');
    const items = within(popover).getAllByRole(mode === 'multiple' ? 'menuitemcheckbox' : 'menuitem')
      .filter(item => (item.getAttribute('aria-label') ?? item.textContent ?? '').includes('Gemini CLI'));
    expect(items.length).toBeGreaterThan(0);
    for (const item of items) expect(item).toBeDisabled();
  });

  it('palette (the @ menu): the same badge, and the row cannot be chosen', () => {
    const onSelect = vi.fn();
    render(<AgentPickerPalette ariaLabel="Mention" t={t} rows={[
      { key: 'gemini', agent: 'GeminiCli', label: '@gemini', availability: unavailable({ agent: 'GeminiCli' }), onSelect },
    ]} />);
    const row = screen.getByRole('option');
    const badge = within(row).getByTestId('agent-unavailable');
    expect(badge).toHaveAttribute('data-state', 'unavailable');
    expect(badge).toHaveClass('kr-agent-switch-badge');
    expect(row).toHaveAttribute('aria-disabled', 'true');
    fireEvent.mouseDown(row);
    expect(onSelect).not.toHaveBeenCalled();
  });
});

describe('AgentSwitchPicker — the modes (KT-1885)', () => {
  it('field: a full-width trigger', () => {
    const { container } = render(MODES.tiers());
    expect(container.querySelector('.kr-agent-switch')).toHaveAttribute('data-variant', 'field');
  });

  it('compact chip: the dense trigger', () => {
    const { container } = render(MODES.chip());
    expect(container.querySelector('.kr-agent-switch')).toHaveAttribute('data-variant', 'compact');
  });

  it('multi-choice: each target toggles and the list stays open', async () => {
    const onToggle = vi.fn();
    render(
      <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS}
        multiple={{ selected: [{ agent: 'ClaudeCode' }], onToggle, summary: selected => `${selected.length} chosen` }}
        title="Agent" ariaLabel="Agent" />,
    );
    expect(screen.getByRole('button', { name: /^Agent( — |$)/ })).toHaveTextContent('1 chosen');
    const popover = await open();
    expect(within(popover).getByRole('menuitemcheckbox', { name: /Claude Code/ })).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(within(popover).getByRole('menuitemcheckbox', { name: /Codex/ }));
    expect(onToggle).toHaveBeenCalledWith({ agent: 'Codex' });
    expect(screen.getByRole('dialog', { name: 'Agent' })).toBeInTheDocument();
  });

  it('exact model: « = principal », a free model id typed in the search, the main model greyed', async () => {
    const onChange = vi.fn();
    render(
      <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} title="Agent" ariaLabel="Agent"
        exactModel={{ value: { agent: 'Codex' }, onChange, main: { agent: 'ClaudeCode' }, testId: 'exact' }} />,
    );
    const popover = await open();
    const main = within(within(popover).getByRole('group', { name: 'Claude Code' })).getAllByRole('menuitem')[0];
    expect(main).toBeDisabled();
    fireEvent.change(within(popover).getByRole('searchbox'), { target: { value: 'gpt-5.5-mini' } });
    // KT-1994 review — one typed id, for the chosen agent.
    const free = popover.querySelectorAll<HTMLElement>('[data-kind="free"]');
    expect(free).toHaveLength(1);
    fireEvent.click(free[0]);
    expect(onChange).toHaveBeenCalledWith({ agent: 'Codex', model: 'gpt-5.5-mini' });
  });
});

describe('AgentSwitchPicker — review fixes (KT-1885)', () => {
  it.each([
    ['fr', 0, 'Aucun agent'], ['fr', 1, '1 agent'], ['fr', 2, '2 agents'],
    ['en', 0, 'No agent'], ['en', 1, '1 agent'], ['en', 3, '3 agents'],
    ['es', 1, '1 agente'], ['es', 2, '2 agentes'], ['zh', 2, '2 个代理'],
  ] as const)('multi-choice (%s): %i chosen reads « %s », a real singular and plural', async (locale, count, text) => {
    await loadLocale(locale);
    setUILocale(locale);
    const selected = TARGETS.slice(0, count).map(target => ({ agent: target.agent }));
    render(
      <I18nProvider>
        <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS}
          multiple={{ selected, onToggle: vi.fn() }} title="Agent" ariaLabel="Agent" />
      </I18nProvider>,
    );
    expect(screen.getByRole('button', { name: /^Agent( — |$)/ })).toHaveTextContent(new RegExp(`^${text}$`));
  });

  // KT-1994 review, point 1a — the typed id is one entry at the end of the
  // list, for the step's own agent when none is chosen; no « Autres agents ».
  it('exact model: the typed id is one entry at the end of the list, for the step’s own agent', async () => {
    const onChange = vi.fn();
    render(
      <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} title="Agent" ariaLabel="Agent"
        exactModel={{ value: undefined, onChange, main: { agent: 'ClaudeCode' }, testId: 'exact' }} />,
    );
    const popover = await open();
    // « codex » matches one agent's name: the others leave the list.
    fireEvent.change(within(popover).getByRole('searchbox'), { target: { value: 'codex' } });
    expect(within(popover).getByRole('group', { name: 'Codex' })).toBeInTheDocument();
    expect(within(popover).queryByRole('group', { name: 'Gemini CLI' })).toBeNull();
    expect(within(popover).queryByRole('group', { name: 'Claude Code' })).toBeNull();
    const items = within(popover).getAllByRole('menuitem');
    const free = popover.querySelectorAll<HTMLElement>('[data-kind="free"]');
    expect(free).toHaveLength(1);
    expect(items[items.length - 1]).toBe(free[0]);
    fireEvent.click(free[0]);
    expect(onChange).toHaveBeenCalledWith({ agent: 'ClaudeCode', model: 'codex' });
  });

  it('an outside click closes it without sending the focus back to the trigger later', async () => {
    let finishSave!: () => void;
    const onChange = vi.fn(() => new Promise<void>(resolve => { finishSave = resolve; }));
    render(
      <>
        <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS}
          onChange={onChange} title="Agent" ariaLabel="Agent" />
        <input aria-label="elsewhere" />
      </>,
    );
    const popover = await open();
    // A choice starts saving; meanwhile the user clicks into another field.
    fireEvent.click(within(popover).getByRole('menuitem', { name: /Codex/ }));
    expect(onChange).toHaveBeenCalledWith('Codex');
    const elsewhere = screen.getByRole('textbox', { name: 'elsewhere' });
    fireEvent.mouseDown(elsewhere);
    elsewhere.focus();
    expect(screen.queryByRole('dialog', { name: 'Agent' })).toBeNull();
    // The save ends later: the focus stays where the user put it.
    await act(async () => { finishSave(); });
    expect(elsewhere).toHaveFocus();
  });

  it('palette: names its rows for the field, keeps the highlighted row in view, and leaves Tab to the field', () => {
    const scrollIntoView = vi.fn();
    const original = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrollIntoView;
    const rows = (highlighted: number) => ['claude', 'codex', 'gemini'].map((key, index) => ({
      key, agent: 'ClaudeCode' as const, label: `@${key}`, highlighted: index === highlighted, onSelect: vi.fn(),
      tiers: {
        targetLabel: `@${key}`, resolve: () => ({ model: '' }) as never, tierLabel: (tier: string) => tier,
        modelLabel: () => 'model', defaultLabel: 'default', onSelect: vi.fn(),
      },
    }));
    const { rerender } = render(<AgentPickerPalette id="mentions" ariaLabel="Mention" t={t} rows={rows(0)} />);
    const options = screen.getAllByRole('option');
    expect(screen.getByRole('listbox')).toHaveAttribute('id', 'mentions');
    expect(options.map(option => option.id)).toEqual([
      agentPaletteOptionId('mentions', 'claude'), agentPaletteOptionId('mentions', 'codex'), agentPaletteOptionId('mentions', 'gemini'),
    ]);
    expect(new Set(options.map(option => option.id)).size).toBe(3);
    scrollIntoView.mockClear();
    rerender(<AgentPickerPalette id="mentions" ariaLabel="Mention" t={t} rows={rows(2)} />);
    expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' });
    expect(scrollIntoView.mock.contexts.at(-1)).toBe(options[2]);
    for (const tier of screen.getAllByRole('button').filter(node => node.classList.contains('kr-agent-switch-tier-option'))) {
      expect(tier).toHaveAttribute('tabindex', '-1');
    }
    HTMLElement.prototype.scrollIntoView = original;
  });

  it('palette: the field that drives it carries aria-controls, aria-expanded and aria-activedescendant while open', () => {
    expect(agentPaletteFieldAria('mentions', false, 'codex')).toEqual({});
    expect(agentPaletteFieldAria('mentions', true, 'codex')).toEqual({
      'aria-controls': 'mentions',
      'aria-expanded': true,
      'aria-activedescendant': agentPaletteOptionId('mentions', 'codex'),
    });
  });
});
