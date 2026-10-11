import { describe, it, expect, vi, afterEach } from 'vitest';
import { useRef, useState } from 'react';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
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

  describe('Escape ownership (KT-1072 review)', () => {
    function Single({ onEscape, modal, inner }: { onEscape: () => void; modal: boolean; inner?: (e: React.KeyboardEvent) => void }) {
      const ref = useDialogFocus<HTMLDivElement>({ onEscape });
      return (
        <>
          <input aria-label="page composer" />
          <div ref={ref} role="dialog" aria-modal={modal ? 'true' : undefined}>
            <input aria-label="inside" onKeyDown={inner} />
          </div>
        </>
      );
    }

    it('ignores Escape during an IME composition', () => {
      const onEscape = vi.fn();
      render(<Single onEscape={onEscape} modal />);
      fireEvent.keyDown(screen.getByLabelText('inside'), { key: 'Escape', isComposing: true });
      fireEvent.keyDown(screen.getByLabelText('inside'), { key: 'Escape', keyCode: 229 });
      expect(onEscape).not.toHaveBeenCalled();
      fireEvent.keyDown(screen.getByLabelText('inside'), { key: 'Escape' });
      expect(onEscape).toHaveBeenCalledTimes(1);
    });

    it('a non-modal dialog ignores Escape pressed outside it', () => {
      const onEscape = vi.fn();
      render(<Single onEscape={onEscape} modal={false} />);
      fireEvent.keyDown(screen.getByLabelText('page composer'), { key: 'Escape' });
      fireEvent.keyDown(document.body, { key: 'Escape' });
      expect(onEscape).not.toHaveBeenCalled();
      fireEvent.keyDown(screen.getByLabelText('inside'), { key: 'Escape' });
      expect(onEscape).toHaveBeenCalledTimes(1);
    });

    it('a modal dialog takes Escape from body but not from another focused element', () => {
      const onEscape = vi.fn();
      render(<Single onEscape={onEscape} modal />);
      fireEvent.keyDown(screen.getByLabelText('page composer'), { key: 'Escape' });
      expect(onEscape).not.toHaveBeenCalled();
      fireEvent.keyDown(document.body, { key: 'Escape' });
      expect(onEscape).toHaveBeenCalledTimes(1);
    });

    it('closes one layer per Escape: page listeners do not see it', async () => {
      const onEscape = vi.fn();
      const pageWindow = vi.fn();
      const pageDocument = vi.fn();
      window.addEventListener('keydown', pageWindow);
      document.addEventListener('keydown', pageDocument);
      try {
        render(<Single onEscape={onEscape} modal />);
        await userEvent.keyboard('{Escape}');
        expect(onEscape).toHaveBeenCalledTimes(1);
        expect(pageWindow).not.toHaveBeenCalled();
        expect(pageDocument).not.toHaveBeenCalled();
      } finally {
        window.removeEventListener('keydown', pageWindow);
        document.removeEventListener('keydown', pageDocument);
      }
    });

    it('an inner control that consumes Escape keeps the dialog open', async () => {
      const onEscape = vi.fn();
      render(<Single onEscape={onEscape} modal inner={e => { if (e.key === 'Escape') e.preventDefault(); }} />);
      await userEvent.keyboard('{Escape}');
      expect(onEscape).not.toHaveBeenCalled();
    });
  });

  it('closing a dialog out of order leaves focus in the dialog still open', async () => {
    function A({ open }: { open: boolean }) {
      const ref = useDialogFocus<HTMLDivElement>({ active: open });
      return open ? <div ref={ref} role="dialog" aria-label="A"><button type="button">in A</button></div> : null;
    }
    function B() {
      const ref = useDialogFocus<HTMLDivElement>();
      return <div ref={ref} role="dialog" aria-label="B"><button type="button">in B</button><button type="button">b2</button></div>;
    }
    function Layers() {
      const [a, setA] = useState(false);
      const [b, setB] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setA(true)}>open A</button>
          <button type="button" onClick={() => setB(true)}>open B</button>
          <button type="button" onClick={() => setA(false)}>close A</button>
          <A open={a} />
          {b && <B />}
        </>
      );
    }
    render(<Layers />);
    await userEvent.click(screen.getByText('open A'));
    expect(screen.getByText('in A')).toHaveFocus();
    fireEvent.click(screen.getByText('open B'));
    expect(screen.getByText('in B')).toHaveFocus();
    fireEvent.click(screen.getByText('close A'));
    expect(screen.queryByRole('dialog', { name: 'A' })).toBeNull();
    expect(screen.getByText('in B')).toHaveFocus();
    await userEvent.tab();
    await userEvent.tab();
    expect(screen.getByText('in B')).toHaveFocus();
  });
});
