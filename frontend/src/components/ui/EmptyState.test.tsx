// KT-1883 — EmptyState: three cases, each with its way out. A first use
// offers the action that creates; a search with no result offers to clear;
// nothing selected offers nothing (the list next to it is the way out).
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { EmptyState } from './EmptyState';
import { EMPTY_STATE_KINDS } from './feedbackKinds';
import { describeViolations, runAxe } from '../../test/axe';

describe('EmptyState', () => {
  it('has the three cases', () => {
    expect(EMPTY_STATE_KINDS).toEqual(['firstUse', 'noResult', 'nothingSelected']);
  });

  it('offers the action that creates the first item, as the primary button', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const { container } = render(
      <EmptyState kind="firstUse" title="No workflow yet" description="A workflow runs agents on a schedule." action={{ label: 'Create a workflow', onClick }} />,
    );
    expect(container.firstElementChild).toHaveAttribute('data-kind', 'firstUse');
    expect(screen.getByText('No workflow yet')).toBeInTheDocument();
    expect(screen.getByText('A workflow runs agents on a schedule.')).toBeInTheDocument();
    const button = screen.getByRole('button', { name: 'Create a workflow' });
    expect(button).toHaveAttribute('data-variant', 'primary');
    await user.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('offers to clear the filters when nothing matches, with its own label or the default one', async () => {
    const user = userEvent.setup();
    const onClear = vi.fn();
    const { rerender } = render(<EmptyState kind="noResult" title="No task matches" onClear={onClear} />);
    await user.click(screen.getByRole('button', { name: 'ui.emptyState.clear' }));
    expect(onClear).toHaveBeenCalledTimes(1);
    rerender(<EmptyState kind="noResult" title="No task matches" onClear={onClear} clearLabel="Clear the search" />);
    expect(screen.getByRole('button', { name: 'Clear the search' })).toHaveAttribute('data-variant', 'neutral');
  });

  it('offers no action when nothing is selected', () => {
    render(<EmptyState kind="nothingSelected" title="Pick a discussion on the left" />);
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('requires the way out of each case at the type level', () => {
    // @ts-expect-error — a first use needs its action.
    void <EmptyState kind="firstUse" title="x" />;
    // @ts-expect-error — no result needs its clear.
    void <EmptyState kind="noResult" title="x" />;
    // @ts-expect-error — nothing selected takes no action.
    void <EmptyState kind="nothingSelected" title="x" action={{ label: 'y', onClick: () => {} }} />;
  });

  it('hides its icon from assistive technology and takes another one', () => {
    const { container } = render(<EmptyState kind="nothingSelected" title="Pick one" icon={<svg data-testid="own" />} />);
    expect(container.querySelector('.ui-empty-state-icon')).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getByTestId('own')).toBeInTheDocument();
  });

  it('passes axe in its three cases', async () => {
    const { container } = render(<main>
      <EmptyState kind="firstUse" title="No workflow yet" action={{ label: 'Create a workflow', onClick: () => {} }} />
      <EmptyState kind="noResult" title="No task matches" onClear={() => {}} />
      <EmptyState kind="nothingSelected" title="Pick a discussion" />
    </main>);
    const results = await act(() => runAxe(container));
    expect(describeViolations(results)).toEqual([]);
  });
});
