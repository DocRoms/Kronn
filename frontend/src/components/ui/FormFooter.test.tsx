// KT-1882 — FormFooter: what still blocks the action, then Cancel, then the
// action on the right; pinned to the bottom of the scrolling form.
import { describe, expect, it, vi } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FormFooter } from './FormFooter';
import { describeViolations, runAxe } from '../../test/axe';

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(join(HERE, 'FormFooter.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

describe('FormFooter', () => {
  it('puts Cancel first, then the action, last in the row', () => {
    render(<FormFooter submitLabel="Create the workflow" onSubmit={() => {}} onCancel={() => {}} />);
    const buttons = screen.getAllByRole('button');
    expect(buttons.map(b => b.textContent)).toEqual(['ui.formFooter.cancel', 'Create the workflow']);
    expect(buttons[1]).toHaveAttribute('data-variant', 'primary');
    expect(buttons[0]).toHaveAttribute('data-variant', 'neutral');
  });

  it('calls onCancel and onSubmit', async () => {
    const onCancel = vi.fn();
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<FormFooter submitLabel="Save" cancelLabel="Discard" onSubmit={onSubmit} onCancel={onCancel} />);
    await user.click(screen.getByRole('button', { name: 'Discard' }));
    await user.click(screen.getByRole('button', { name: 'Save' }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it('has no Cancel when nothing cancels', () => {
    render(<FormFooter submitLabel="Save" onSubmit={() => {}} />);
    expect(screen.getAllByRole('button')).toHaveLength(1);
  });

  it('submits the form around it when it has no onSubmit', async () => {
    const onFormSubmit = vi.fn((e: { preventDefault: () => void }) => e.preventDefault());
    const user = userEvent.setup();
    render(<form onSubmit={onFormSubmit}><FormFooter submitLabel="Save" /></form>);
    const action = screen.getByRole('button', { name: 'Save' });
    expect(action).toHaveAttribute('type', 'submit');
    await user.click(action);
    expect(onFormSubmit).toHaveBeenCalledTimes(1);
  });

  it('lists what blocks the action, and the action says why it is not available', async () => {
    const onSubmit = vi.fn();
    const onFormSubmit = vi.fn((e: { preventDefault: () => void }) => e.preventDefault());
    const user = userEvent.setup();
    render(
      <form onSubmit={onFormSubmit}>
        <FormFooter submitLabel="Create the workflow" onSubmit={onSubmit} blockers={['Name the workflow.', 'Add a first step.']} />
      </form>,
    );
    const list = screen.getByRole('list');
    expect(screen.getByText('ui.formFooter.blockers')).toHaveClass('ui-visually-hidden');
    expect(within(list).getAllByRole('listitem').map(li => li.textContent)).toEqual(['Name the workflow.', 'Add a first step.']);
    const action = screen.getByRole('button', { name: 'Create the workflow' });
    // Still focusable, so a keyboard or screen-reader user reaches it and hears why.
    expect(action).not.toBeDisabled();
    expect(action).toHaveAttribute('aria-disabled', 'true');
    const description = action.getAttribute('aria-describedby')!.split(' ').map(id => document.getElementById(id)!.textContent).join(' ');
    expect(description).toContain('ui.formFooter.blockers');
    expect(description).toContain('Name the workflow.');
    expect(description).toContain('Add a first step.');
    expect(action).toHaveAccessibleDescription(/Name the workflow\.\s*Add a first step\./);
    await user.click(action);
    action.focus();
    await user.keyboard('{Enter}');
    expect(onSubmit).not.toHaveBeenCalled();
    expect(onFormSubmit).not.toHaveBeenCalled();
  });

  it('announces the blockers as they change, from a live region present from the start', () => {
    const { container, rerender } = render(<FormFooter submitLabel="Save" onSubmit={() => {}} />);
    const live = container.querySelector('[aria-live="polite"]')!;
    expect(live).toBeTruthy();
    expect(within(live as HTMLElement).queryByRole('listitem')).toBeNull();
    expect(screen.getByRole('button', { name: 'Save' })).not.toHaveAttribute('aria-disabled');

    rerender(<FormFooter submitLabel="Save" onSubmit={() => {}} blockers={['Fix the cron expression.']} />);
    expect(container.querySelector('[aria-live="polite"]')).toBe(live);
    expect(live).toHaveTextContent('Fix the cron expression.');
  });

  it('holds the action while busy, and says so', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<FormFooter submitLabel="Save" onSubmit={onSubmit} busy />);
    const action = screen.getByRole('button', { name: 'Save' });
    expect(action).toHaveAttribute('aria-disabled', 'true');
    expect(action).toHaveAttribute('aria-busy', 'true');
    await user.click(action);
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it('takes a danger action', () => {
    render(<FormFooter submitLabel="Delete the project" tone="danger" onSubmit={() => {}} />);
    expect(screen.getByRole('button', { name: 'Delete the project' })).toHaveAttribute('data-variant', 'danger');
  });

  it('is pinned by default, and can sit in the flow', () => {
    const { container, rerender } = render(<FormFooter submitLabel="Save" onSubmit={() => {}} />);
    expect(container.firstElementChild).toHaveAttribute('data-pinned', 'true');
    rerender(<FormFooter submitLabel="Save" onSubmit={() => {}} pinned={false} />);
    expect(container.firstElementChild).toHaveAttribute('data-pinned', 'false');
  });

  it('pins itself to the bottom of what scrolls, the actions at the end edge', () => {
    expect(CSS).toMatch(/\.ui-form-footer\[data-pinned="true"\]\s*\{[^}]*position:\s*sticky[^}]*\}/);
    expect(CSS).toMatch(/\.ui-form-footer\[data-pinned="true"\]\s*\{[^}]*bottom:\s*0[^}]*\}/);
    expect(CSS).toMatch(/\.ui-form-footer-actions\s*\{[^}]*margin-inline-start:\s*auto[^}]*\}/);
  });

  it('passes axe, ready and blocked', async () => {
    render(
      <main>
        <FormFooter submitLabel="Save" onSubmit={() => {}} onCancel={() => {}} />
        <FormFooter submitLabel="Create" onSubmit={() => {}} onCancel={() => {}} blockers={['Name it.']} />
      </main>,
    );
    const results = await act(() => runAxe(document.body));
    expect(describeViolations(results)).toEqual([]);
  });
});
