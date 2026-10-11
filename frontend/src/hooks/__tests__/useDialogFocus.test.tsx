import { describe, it, expect, vi, afterEach } from 'vitest';
import { useRef, useState } from 'react';
import { render, screen, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useDialogFocus, type DialogFocusOptions } from '../useDialogFocus';

function Dialog({ onClose, options }: { onClose: () => void; options?: DialogFocusOptions }) {
  const ref = useDialogFocus<HTMLDivElement>({ onEscape: onClose, ...options });
  return (
    <div ref={ref} role="dialog" aria-label="dialog">
      <button type="button">first</button>
      <input aria-label="middle" />
      <button type="button" onClick={onClose}>last</button>
    </div>
  );
}

function Harness({ options, onEscapeSpy }: { options?: DialogFocusOptions; onEscapeSpy?: () => void }) {
  const [open, setOpen] = useState(false);
  const close = () => { onEscapeSpy?.(); setOpen(false); };
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>open</button>
      <button type="button">outside</button>
      {open && <Dialog onClose={close} options={options} />}
    </>
  );
}

afterEach(cleanup);

describe('useDialogFocus (KT-1072)', () => {
  it('focuses the first control on open', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByText('open'));
    expect(screen.getByText('first')).toHaveFocus();
  });

  it('wraps Tab and Shift+Tab inside the dialog', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByText('open'));
    await userEvent.tab();
    expect(screen.getByLabelText('middle')).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByText('last')).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByText('first')).toHaveFocus();
    await userEvent.tab({ shift: true });
    expect(screen.getByText('last')).toHaveFocus();
  });

  it('closes on Escape and returns focus to the trigger', async () => {
    const spy = vi.fn();
    render(<Harness onEscapeSpy={spy} />);
    await userEvent.click(screen.getByText('open'));
    await userEvent.keyboard('{Escape}');
    expect(spy).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByText('open')).toHaveFocus();
  });

  it('returns focus to the trigger when closed by a control', async () => {
    render(<Harness />);
    await userEvent.click(screen.getByText('open'));
    await userEvent.click(screen.getByText('last'));
    expect(screen.getByText('open')).toHaveFocus();
  });

  it('honours an explicit initial focus target', async () => {
    function WithInitial() {
      const input = useRef<HTMLInputElement>(null);
      const ref = useDialogFocus<HTMLDivElement>({ initialFocus: input });
      return (
        <div ref={ref} role="dialog">
          <button type="button">a</button>
          <input ref={input} aria-label="target" />
        </div>
      );
    }
    render(<WithInitial />);
    expect(screen.getByLabelText('target')).toHaveFocus();
  });

  it('only the innermost dialog reacts to Escape', async () => {
    const outer = vi.fn();
    const inner = vi.fn();
    function Nested() {
      const outerRef = useDialogFocus<HTMLDivElement>({ onEscape: outer });
      const innerRef = useDialogFocus<HTMLDivElement>({ onEscape: inner });
      return (
        <div ref={outerRef} role="dialog">
          <button type="button">outer</button>
          <div ref={innerRef} role="dialog"><button type="button">inner</button></div>
        </div>
      );
    }
    render(<Nested />);
    await userEvent.keyboard('{Escape}');
    expect(inner).toHaveBeenCalledTimes(1);
    expect(outer).not.toHaveBeenCalled();
  });

  it('lets Tab leave a non-modal panel (trap: false)', async () => {
    render(<Harness options={{ trap: false }} />);
    await userEvent.click(screen.getByText('open'));
    await userEvent.tab();
    await userEvent.tab();
    await userEvent.tab();
    expect(screen.getByRole('dialog').contains(document.activeElement)).toBe(false);
  });

  it('falls back to returnFocus when the trigger left the DOM', async () => {
    function Swapping() {
      const [open, setOpen] = useState(false);
      const back = useRef<HTMLButtonElement>(null);
      const ref = useDialogFocus<HTMLDivElement>({ active: open, onEscape: () => setOpen(false), returnFocus: back });
      if (!open) return <button type="button" ref={back} onClick={() => setOpen(true)}>open</button>;
      return <div ref={ref} role="dialog"><button type="button">inside</button></div>;
    }
    render(<Swapping />);
    await userEvent.click(screen.getByText('open'));
    expect(screen.getByText('inside')).toHaveFocus();
    await userEvent.keyboard('{Escape}');
    expect(screen.getByText('open')).toHaveFocus();
  });
});
