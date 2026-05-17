// 0.8.5 — picker UI tests. Verify the collapsed → expanded flow, the
// filter chips, the search input, and that clicking "Use this template"
// calls onApply with the original entry (so the wizard's apply* router
// gets the verbatim payload).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { WorkflowQuickStartPicker } from '../WorkflowQuickStartPicker';
import type { UnifiedQuickStart } from '../../../lib/workflow-quick-start';
import type { WorkflowStep } from '../../../types/generated';

function tFr(key: string, ...args: (string | number)[]): string {
  // Minimal stub that mirrors the i18n function shape. Returns the key
  // so tests can assert on it without depending on the real string
  // table — the picker only cares that text is rendered.
  if (args.length === 0) return key;
  return `${key}:${args.join(',')}`;
}

function makeStep(name: string): WorkflowStep {
  return {
    name,
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
  } as WorkflowStep;
}

function entry(overrides: Partial<UnifiedQuickStart> = {}): UnifiedQuickStart {
  return {
    id: 'preset:auto-dev',
    source: 'preset',
    title: 'Auto-dev (Agent + Exec)',
    description: 'Lance un agent puis exécute les commandes shell.',
    complexity: 'simple',
    stepsCount: 2,
    badges: ['Agent', 'Exec'],
    applicable: true,
    payload: { kind: 'preset', preset: { id: 'auto-dev', steps: [makeStep('main')] } as never },
    ...overrides,
  };
}

describe('WorkflowQuickStartPicker — visibility', () => {
  it('renders nothing when isEdit is true', () => {
    const { container } = render(
      <WorkflowQuickStartPicker
        entries={[entry()]}
        isEdit={true}
        onApply={vi.fn()}
        t={tFr}
      />,
    );
    expect(container.firstChild).toBeNull();
  });

  it('renders nothing when the catalogue is empty', () => {
    const { container } = render(
      <WorkflowQuickStartPicker
        entries={[]}
        isEdit={false}
        onApply={vi.fn()}
        t={tFr}
      />,
    );
    expect(container.firstChild).toBeNull();
  });

  it('renders the collapsed toggle by default', () => {
    render(
      <WorkflowQuickStartPicker
        entries={[entry(), entry({ id: 'p2', title: 'Other' })]}
        isEdit={false}
        onApply={vi.fn()}
        t={tFr}
      />,
    );
    const toggle = screen.getByRole('button', { name: /wiz\.quickstart\.toggle/i });
    expect(toggle).toBeInTheDocument();
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle.textContent).toContain('2'); // count substituted into toggle label
  });
});

describe('WorkflowQuickStartPicker — expanded state', () => {
  function setup() {
    const onApply = vi.fn();
    render(
      <WorkflowQuickStartPicker
        entries={[
          entry({ id: 'preset:auto-dev', source: 'preset', title: 'Auto-dev', complexity: 'simple' }),
          entry({
            id: 'starter:chartbeat',
            source: 'starter',
            title: 'Chartbeat Top 5',
            complexity: 'intermediate',
            badges: ['mcp-chartbeat'],
            description: 'Demo plugin Chartbeat.',
            applicable: false,
            notApplicableReason: 'requires plugin: mcp-chartbeat',
          }),
          entry({
            id: 'suggestion:autopilot',
            source: 'project-suggestion',
            title: 'AutoPilot ticket',
            complexity: 'advanced',
            audience: 'dev',
            reason: 'Le projet a un tracker GitHub.',
          }),
        ]}
        isEdit={false}
        onApply={onApply}
        t={tFr}
      />,
    );
    // Expand the panel.
    fireEvent.click(screen.getByRole('button', { name: /wiz\.quickstart\.toggle/i }));
    return { onApply };
  }

  it('shows all three rows after expanding', () => {
    setup();
    expect(screen.getByText('Auto-dev')).toBeInTheDocument();
    expect(screen.getByText('Chartbeat Top 5')).toBeInTheDocument();
    expect(screen.getByText('AutoPilot ticket')).toBeInTheDocument();
  });

  it('renders the search input + filter chips', () => {
    setup();
    expect(screen.getByPlaceholderText(/wiz\.quickstart\.searchPlaceholder/)).toBeInTheDocument();
    // 3 complexity chips + 3 source chips
    expect(screen.getAllByText(/wiz\.quickstart\.complexity\.(simple|intermediate|advanced)/))
      .toHaveLength(6); // 3 chips + 3 badges (one per row)
    expect(screen.getAllByText(/wiz\.quickstart\.source\.(starter|preset|project-suggestion)/))
      .toHaveLength(6); // 3 chips + 3 badges
  });

  it('disables the Apply button on a non-applicable entry + shows reason', () => {
    setup();
    expect(screen.getByText(/requires plugin: mcp-chartbeat/)).toBeInTheDocument();
    const buttons = screen.getAllByRole('button', { name: /wiz\.quickstart\.apply/ });
    // 3 entries, 3 apply buttons. The one tied to Chartbeat is disabled.
    const disabled = buttons.filter(b => (b as HTMLButtonElement).disabled);
    expect(disabled).toHaveLength(1);
  });

  it('calls onApply with the verbatim entry when "Use this template" is clicked', () => {
    const { onApply } = setup();
    const buttons = screen.getAllByRole('button', { name: /wiz\.quickstart\.apply/ });
    // First entry = Auto-dev (preset, simple, applicable)
    fireEvent.click(buttons[0]);
    expect(onApply).toHaveBeenCalledTimes(1);
    expect(onApply.mock.calls[0][0].id).toBe('preset:auto-dev');
  });

  it('filters by free-text search', () => {
    setup();
    const input = screen.getByPlaceholderText(/wiz\.quickstart\.searchPlaceholder/);
    fireEvent.change(input, { target: { value: 'chartbeat' } });
    expect(screen.queryByText('Auto-dev')).not.toBeInTheDocument();
    expect(screen.getByText('Chartbeat Top 5')).toBeInTheDocument();
    expect(screen.queryByText('AutoPilot ticket')).not.toBeInTheDocument();
  });

  it('shows the empty state when filters match nothing', () => {
    setup();
    const input = screen.getByPlaceholderText(/wiz\.quickstart\.searchPlaceholder/);
    fireEvent.change(input, { target: { value: 'zzzzzz' } });
    expect(screen.getByText(/wiz\.quickstart\.empty/)).toBeInTheDocument();
  });

  it('filters by complexity chip', () => {
    setup();
    // Find the chips by their text content (Translator returns the i18n key).
    const advancedChip = screen.getAllByText('wiz.quickstart.complexity.advanced')
      .map(el => el.closest('button'))
      .find(b => b !== null) as HTMLButtonElement;
    fireEvent.click(advancedChip);
    expect(screen.queryByText('Auto-dev')).not.toBeInTheDocument();
    expect(screen.queryByText('Chartbeat Top 5')).not.toBeInTheDocument();
    expect(screen.getByText('AutoPilot ticket')).toBeInTheDocument();
  });

  it('renders the reason blurb for suggestions that have one', () => {
    setup();
    expect(screen.getByText('Le projet a un tracker GitHub.')).toBeInTheDocument();
  });
});

// 0.8.5 dogfooding follow-up — gate the picker until the wizard's
// prerequisites are met (currently: a non-empty workflow name). Pre-fix
// the user could click a template before naming the workflow and the
// validator bounced them back to step 0 — confusing.
describe('WorkflowQuickStartPicker — disabled gate', () => {
  it('renders a disabled toggle with the supplied tooltip when `disabled` is true', () => {
    render(
      <WorkflowQuickStartPicker
        entries={[entry()]}
        isEdit={false}
        disabled={true}
        disabledReason="Saisissez un nom de workflow avant de sélectionner un modèle."
        onApply={vi.fn()}
        t={tFr}
      />,
    );
    const toggle = screen.getByRole('button', { name: /wiz\.quickstart\.toggle/i });
    expect(toggle).toBeDisabled();
    expect(toggle).toHaveAttribute('title', 'Saisissez un nom de workflow avant de sélectionner un modèle.');
    expect(toggle).toHaveAttribute('aria-disabled', 'true');
  });

  it('does not call onApply when the disabled toggle is clicked', () => {
    const onApply = vi.fn();
    render(
      <WorkflowQuickStartPicker
        entries={[entry()]}
        isEdit={false}
        disabled={true}
        disabledReason="…"
        onApply={onApply}
        t={tFr}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wiz\.quickstart\.toggle/i }));
    expect(onApply).not.toHaveBeenCalled();
    // Panel stays collapsed — no search input rendered.
    expect(screen.queryByPlaceholderText(/searchPlaceholder/)).not.toBeInTheDocument();
  });

  it('omits the tooltip when not disabled', () => {
    render(
      <WorkflowQuickStartPicker
        entries={[entry()]}
        isEdit={false}
        disabled={false}
        disabledReason="…"
        onApply={vi.fn()}
        t={tFr}
      />,
    );
    const toggle = screen.getByRole('button', { name: /wiz\.quickstart\.toggle/i });
    expect(toggle).not.toBeDisabled();
    expect(toggle).not.toHaveAttribute('title');
  });

  it('treats omitted `disabled` prop as enabled (backwards-compat)', () => {
    render(
      <WorkflowQuickStartPicker
        entries={[entry()]}
        isEdit={false}
        onApply={vi.fn()}
        t={tFr}
      />,
    );
    expect(screen.getByRole('button', { name: /wiz\.quickstart\.toggle/i })).not.toBeDisabled();
  });
});
