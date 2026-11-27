// KT-1881 — ConfirmDialog, in place of window.confirm: an alertdialog that
// asks a question, names its action with the verb and the object, starts on
// Cancel, and answers false to Escape, the cross, the scrim and Cancel.
// useConfirm wraps it in a promise, so `if (!confirm(…)) return;` becomes
// `if (!(await confirm({…}))) return;`.
import { describe, expect, it, vi } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { ConfirmDialog } from './ConfirmDialog';
import { useConfirm } from './useConfirm';
import { describeViolations, runAxe } from '../../test/axe';

function Harness({ onConfirm = () => {}, onCancel = () => {}, tone }: {
  onConfirm?: () => void;
  onCancel?: () => void;
  tone?: 'danger' | 'primary';
}) {
  const [open, setOpen] = useState(false);
  return (
    <main>
      <button type="button" onClick={() => setOpen(true)}>Delete the selection</button>
      <ConfirmDialog
        open={open}
        title="Delete 3 projects?"
        message={'Their discussions are deleted with them.\n\nThis cannot be undone.'}
        confirmLabel="Delete 3 projects"
        tone={tone}
        onConfirm={() => { onConfirm(); setOpen(false); }}
        onCancel={() => { onCancel(); setOpen(false); }}
      />
    </main>
  );
}

describe('ConfirmDialog', () => {
  it('is an alertdialog named by its question and described by its message', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole('button', { name: 'Delete the selection' }));
    const dialog = screen.getByRole('alertdialog', { name: 'Delete 3 projects?' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(dialog).toHaveAccessibleDescription(/Their discussions are deleted with them\.\s+This cannot be undone\./);
  });

  it('names its action with the verb and the object, in the danger tone, and starts on Cancel', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole('button', { name: 'Delete the selection' }));
    const dialog = screen.getByRole('alertdialog');
    const action = within(dialog).getByRole('button', { name: 'Delete 3 projects' });
    expect(action).toHaveAttribute('data-variant', 'danger');
    const cancel = within(dialog).getByRole('button', { name: 'ui.confirm.cancel' });
    expect(cancel).toHaveFocus();
    // Cancel comes first, the action last.
    const footer = within(dialog).getAllByRole('button').filter(b => b.closest('footer'));
    expect(footer).toEqual([cancel, action]);
  });

  it('takes the primary tone for an action that loses nothing', async () => {
    const user = userEvent.setup();
    render(<Harness tone="primary" />);
    await user.click(screen.getByRole('button', { name: 'Delete the selection' }));
    expect(screen.getByRole('button', { name: 'Delete 3 projects' })).toHaveAttribute('data-variant', 'primary');
  });

  it('confirms only through its action, then gives the focus back', async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(<Harness onConfirm={onConfirm} onCancel={onCancel} />);
    const opener = screen.getByRole('button', { name: 'Delete the selection' });
    await user.click(opener);
    // Enter on the focused Cancel cancels: the default answer is no.
    await user.keyboard('{Enter}');
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
    expect(opener).toHaveFocus();

    await user.click(opener);
    await user.click(screen.getByRole('button', { name: 'Delete 3 projects' }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(opener).toHaveFocus();
  });

  it.each([
    ['Escape', async (user: ReturnType<typeof userEvent.setup>) => user.keyboard('{Escape}')],
    ['the cross', async (user: ReturnType<typeof userEvent.setup>) => user.click(screen.getByRole('button', { name: 'ui.overlay.close' }))],
    ['the scrim', async (user: ReturnType<typeof userEvent.setup>) => user.click(document.querySelector('.ui-overlay-scrim') as HTMLElement)],
  ])('cancels on %s', async (_how, close) => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(<Harness onConfirm={onConfirm} onCancel={onCancel} />);
    const opener = screen.getByRole('button', { name: 'Delete the selection' });
    await user.click(opener);
    await close(user);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
    expect(opener).toHaveFocus();
  });

  it('sits above a drawer or a dialog it was opened from', () => {
    render(<ConfirmDialog open title="Remove?" confirmLabel="Remove the shadow" onConfirm={() => {}} onCancel={() => {}} />);
    expect(document.querySelector('.ui-overlay')).toHaveAttribute('data-layer', 'nested');
  });

  it('passes axe (WCAG 2.1 A and AA) while open', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole('button', { name: 'Delete the selection' }));
    const results = await act(() => runAxe(document.body));
    expect(describeViolations(results)).toEqual([]);
  });
});

function AsyncHarness({ onAnswer }: { onAnswer: (answer: boolean) => void }) {
  const { confirm, dialog } = useConfirm();
  return (
    <main>
      <button
        type="button"
        onClick={async () => onAnswer(await confirm({
          title: 'Delete version v3?',
          message: 'Launches tied to it are no longer attributed.',
          confirmLabel: 'Delete v3',
        }))}
      >
        Delete v3
      </button>
      {dialog}
    </main>
  );
}

describe('useConfirm', () => {
  it('answers true once the action is chosen, false on Cancel or Escape', async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    render(<AsyncHarness onAnswer={onAnswer} />);
    const opener = screen.getByRole('button', { name: 'Delete v3' });

    await user.click(opener);
    const dialog = screen.getByRole('alertdialog', { name: 'Delete version v3?' });
    await user.click(within(dialog).getByRole('button', { name: 'Delete v3' }));
    expect(onAnswer).toHaveBeenLastCalledWith(true);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(opener).toHaveFocus();

    await user.click(opener);
    await user.click(screen.getByRole('button', { name: 'ui.confirm.cancel' }));
    expect(onAnswer).toHaveBeenLastCalledWith(false);

    await user.click(opener);
    await user.keyboard('{Escape}');
    expect(onAnswer).toHaveBeenLastCalledWith(false);
    expect(onAnswer).toHaveBeenCalledTimes(3);
    expect(opener).toHaveFocus();
  });

  it('answers false when the screen goes away before the answer', async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    const { unmount } = render(<AsyncHarness onAnswer={onAnswer} />);
    await user.click(screen.getByRole('button', { name: 'Delete v3' }));
    unmount();
    await act(async () => {});
    expect(onAnswer).toHaveBeenCalledWith(false);
  });
});
