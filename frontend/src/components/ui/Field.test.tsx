// KT-1882 — Field: the label above the control and bound to it, an optional
// marker, a help line and an error line under the control. The asterisk means
// "required" and nothing else; the error is announced and describes the control.
import { describe, expect, it } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Field } from './Field';
import { Select } from './Select';
import { describeViolations, runAxe } from '../../test/axe';
import {
  contrastRatio, cssRules, declarations, loadThemes, parseColor, resolveValue, SURFACE_TOKENS, themeColor,
} from '../../styles/themeAudit';

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(join(HERE, 'Field.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const THEMES = loadThemes(join(HERE, '..', '..'));
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];

describe('Field', () => {
  it('binds its label to a native control it wraps, the label above it', () => {
    render(<Field label="Project name"><input type="text" /></Field>);
    const input = screen.getByRole('textbox', { name: 'Project name' });
    const label = screen.getByText('Project name').closest('label')!;
    expect(label).toHaveAttribute('for', input.id);
    expect(input.id).not.toBe('');
    // The label comes first in the document: above the control.
    expect(label.compareDocumentPosition(input) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('keeps the id the control already has', () => {
    render(<Field label="Branch"><input id="branch-input" type="text" /></Field>);
    expect(screen.getByRole('textbox', { name: 'Branch' })).toHaveAttribute('id', 'branch-input');
  });

  it('binds its label through a render function, for a control it cannot clone', () => {
    render(
      <Field label="Prompt" help="What the agent receives.">
        {control => <div className="wrapper"><textarea {...control} /></div>}
      </Field>,
    );
    const textarea = screen.getByRole('textbox', { name: 'Prompt' });
    expect(textarea).toHaveAccessibleDescription('What the agent receives.');
  });

  it('binds its label to a Select it wraps', () => {
    render(
      <Field label="Agent" help="Runs the step.">
        <Select value="claude" onChange={() => {}} options={[{ value: 'claude', label: 'Claude Code' }, { value: 'codex', label: 'Codex' }]} />
      </Field>,
    );
    const select = screen.getByRole('combobox', { name: 'Agent' });
    expect(select).toHaveAccessibleDescription('Runs the step.');
  });

  it('describes the control with its help line, under the control', () => {
    render(<Field label="Cron" help="Five fields, in the server time zone."><input type="text" /></Field>);
    const input = screen.getByRole('textbox', { name: 'Cron' });
    const help = screen.getByText('Five fields, in the server time zone.');
    expect(input).toHaveAccessibleDescription('Five fields, in the server time zone.');
    expect(input.compareDocumentPosition(help) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(input).not.toHaveAttribute('aria-invalid');
  });

  it('marks the control invalid and reads the error first, then the help', () => {
    render(
      <Field label="Cron" help="Five fields." error="Six fields given, five expected.">
        <input type="text" aria-describedby="outside-note" />
      </Field>,
    );
    const input = screen.getByRole('textbox', { name: 'Cron' });
    expect(input).toHaveAttribute('aria-invalid', 'true');
    const [first, second, third] = input.getAttribute('aria-describedby')!.split(' ');
    expect(document.getElementById(first)).toHaveTextContent('Six fields given, five expected.');
    expect(document.getElementById(second)).toHaveTextContent('Five fields.');
    // The control's own description is kept, last.
    expect(third).toBe('outside-note');
  });

  it('announces an error when it appears: the line sits in a live region present from the start', async () => {
    function Harness() {
      const [error, setError] = useState<string | undefined>();
      return (
        <>
          <Field label="Name" error={error}><input type="text" /></Field>
          <button type="button" onClick={() => setError('A name is required.')}>Validate</button>
        </>
      );
    }
    const user = userEvent.setup();
    const { container } = render(<Harness />);
    const live = container.querySelector('[aria-live]')!;
    expect(live).toBeTruthy();
    expect(live).toHaveAttribute('aria-live', 'polite');
    expect(live).toBeEmptyDOMElement();
    expect(screen.getByRole('textbox')).not.toHaveAttribute('aria-invalid');

    await user.click(screen.getByRole('button', { name: 'Validate' }));
    expect(container.querySelector('[aria-live]')).toBe(live);
    expect(live).toHaveTextContent('A name is required.');
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('textbox')).toHaveAccessibleDescription('A name is required.');
  });

  it('shows the asterisk only on a required field, and tells the control it is required', () => {
    const { container, rerender } = render(<Field label="Name" required><input type="text" /></Field>);
    const marker = container.querySelector('.ui-field-required')!;
    expect(marker).toHaveTextContent('*');
    expect(marker).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveAttribute('aria-required', 'true');

    rerender(<Field label="Name"><input type="text" /></Field>);
    expect(container.textContent).not.toContain('*');
    expect(screen.getByRole('textbox')).not.toHaveAttribute('aria-required');

    rerender(<Field label="Name" optional><input type="text" /></Field>);
    expect(container.textContent).not.toContain('*');
    expect(container.querySelector('.ui-field-optional')).toHaveTextContent('ui.field.optional');
  });

  it('never prints an asterisk for anything but required: the label text cannot carry one', () => {
    // A caller writing "Options *" to say "advanced options are set" (the
    // WorkflowWizard habit) gets its asterisk dropped, not shown.
    render(<Field label="Options *"><input type="text" /></Field>);
    expect(screen.getByRole('textbox', { name: 'Options' })).toBeInTheDocument();
    expect(document.body.textContent).not.toContain('*');
  });

  it('refuses required and optional together at the type level', () => {
    // @ts-expect-error — a field is required or optional, not both.
    const both = <Field label="x" required optional><input /></Field>;
    expect(both).toBeTruthy();
  });

  it('disables the control it wraps', () => {
    const { container } = render(<Field label="Name" disabled><input type="text" /></Field>);
    expect(screen.getByRole('textbox', { name: 'Name' })).toBeDisabled();
    expect(container.firstElementChild).toHaveAttribute('data-disabled', 'true');
  });

  it('gives a native text control the field style, and leaves a component its own', () => {
    render(
      <>
        <Field label="A"><input type="text" className="mine" /></Field>
        <Field label="B"><Select value="" onChange={() => {}} options={[]} placeholder="Choose" /></Field>
      </>,
    );
    expect(screen.getByRole('textbox', { name: 'A' })).toHaveClass('ui-field-control', 'mine');
    expect(screen.getByRole('combobox', { name: 'B' })).not.toHaveClass('ui-field-control');
  });

  it('passes axe in every state', async () => {
    render(
      <main>
        <Field label="Empty"><input type="text" /></Field>
        <Field label="Filled"><input type="text" defaultValue="Kronn" /></Field>
        <Field label="Help" help="Shown under the field."><input type="text" /></Field>
        <Field label="Error" error="Something is wrong."><input type="text" /></Field>
        <Field label="Disabled" disabled><input type="text" /></Field>
        <Field label="Required" required><input type="text" /></Field>
        <Field label="Optional" optional><textarea /></Field>
        <Field label="Agent" required error="Choose an agent.">
          <Select value="" onChange={() => {}} placeholder="Choose" options={[{ value: 'a', label: 'A' }]} />
        </Field>
      </main>,
    );
    const results = await act(() => runAxe(document.body));
    expect(describeViolations(results)).toEqual([]);
    expect(results.passes.map(p => p.id)).toContain('label');
  });

  it('writes the text of a control in 16px on a coarse pointer (no zoom on focus)', () => {
    const coarse = CSS.slice(CSS.indexOf('@media (pointer: coarse)'));
    expect(coarse).toMatch(/\.ui-field-control[^{]*\{[^}]*font-size:\s*var\(--kr-type-xl\)/);
    expect(coarse).toMatch(/\.ui-field-control[^{]*\{[^}]*min-height:\s*var\(--kr-control-h-touch\)/);
    for (const theme of THEME_NAMES) {
      const tokens = THEMES.get(theme)!;
      const value = resolveValue(tokens.get('--kr-type-xl')!, tokens);
      expect(parseFloat(value) * (value.endsWith('rem') ? 16 : 1), theme).toBe(16);
    }
  });

  it('writes the label, the asterisk, the help and the error at 12px at least, and reads them at 4.5:1 in every theme', () => {
    const colourOf = (selector: string) => {
      const rule = cssRules(CSS).find(r => r.selector === selector)!;
      expect(rule, selector).toBeTruthy();
      const decl = new Map(declarations(rule.body));
      if (selector !== '.ui-field-required') {
        expect(decl.get('font-size'), `${selector} size`).toMatch(/var\(--kr-type-(sm|md|lg|xl)\)/);
      }
      return decl.get('color')!;
    };
    const misses: string[] = [];
    for (const selector of ['.ui-field-label', '.ui-field-required', '.ui-field-help', '.ui-field-error']) {
      const colour = colourOf(selector);
      for (const theme of THEME_NAMES) {
        const tokens = THEMES.get(theme)!;
        const fg = parseColor(resolveValue(colour, tokens));
        expect(fg, `${selector} ${theme} colour`).not.toBeNull();
        for (const surface of SURFACE_TOKENS) {
          const bg = themeColor(tokens, surface);
          if (!bg || bg.a < 1) continue;
          const ratio = contrastRatio(fg!, bg);
          if (ratio < 4.5) misses.push(`${selector} ${theme} on ${surface}: ${ratio.toFixed(2)}`);
        }
      }
    }
    expect(misses).toEqual([]);
  });
});
