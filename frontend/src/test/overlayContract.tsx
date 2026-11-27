// KT-1881 — what Dialog and Drawer both guarantee, run against each of them:
// role and name, modal, initial focus, Tab kept inside, Escape, the cross and
// the scrim close, focus back to the opener, one layer per Escape, an axe run
// on the open layer.
import { useRef, useState, type ComponentType, type ReactNode, type RefObject } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expectFocusKeptIn } from './dialogFocus';
import { describeViolations, runAxe } from './axe';

export interface OverlayLikeProps {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  size?: 'sm' | 'md' | 'lg';
  footer?: ReactNode;
  initialFocus?: RefObject<HTMLElement | null>;
  children?: ReactNode;
}

export function describeOverlayContract(name: string, Overlay: ComponentType<OverlayLikeProps>, panelClass: string) {
  function Harness({ Overlay, onClose, nested = false, withInitial = false }: {
    Overlay: ComponentType<OverlayLikeProps>;
    onClose?: () => void;
    nested?: boolean;
    withInitial?: boolean;
  }) {
    const [open, setOpen] = useState(false);
    const [inner, setInner] = useState(false);
    const save = useRef<HTMLButtonElement>(null);
    const close = () => { onClose?.(); setOpen(false); };
    return (
      <main>
        <button type="button" onClick={() => setOpen(true)}>Open it</button>
        <button type="button">Outside</button>
        <Overlay
          open={open}
          onClose={close}
          title="Rename the project"
          description="The name shows in the list and in the URL."
          initialFocus={withInitial ? save : undefined}
          footer={<button type="button" ref={save}>Save the name</button>}
        >
          <label>Name <input type="text" /></label>
          {nested && <button type="button" onClick={() => setInner(true)}>Open the inner one</button>}
          {nested && (
            <Overlay open={inner} onClose={() => setInner(false)} title="Inner layer">
              <button type="button">Inner action</button>
            </Overlay>
          )}
        </Overlay>
      </main>
    );
  }

  describe(`${name} — the shared overlay contract`, () => {
    it('renders nothing while closed', () => {
      render(<Overlay open={false} onClose={() => {}} title="Hidden">x</Overlay>);
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(document.querySelector(`.${panelClass}`)).toBeNull();
    });

    it('is a modal dialog named by its title and described by its description', async () => {
      const user = userEvent.setup();
      render(<Harness Overlay={Overlay} />);
      await user.click(screen.getByRole('button', { name: 'Open it' }));
      const dialog = screen.getByRole('dialog', { name: 'Rename the project' });
      expect(dialog).toHaveAttribute('aria-modal', 'true');
      expect(dialog).toHaveAccessibleDescription('The name shows in the list and in the URL.');
      expect(dialog).toHaveClass(panelClass);
      // In <body>, so an ancestor with overflow: hidden or a transform does not clip it.
      expect(dialog.closest('main')).toBeNull();
      expect(within(dialog).getByRole('heading', { level: 2, name: 'Rename the project' })).toBeInTheDocument();
    });

    it('puts the focus on the first control of its content, or on the one the caller names', async () => {
      const user = userEvent.setup();
      const { unmount } = render(<Harness Overlay={Overlay} />);
      await user.click(screen.getByRole('button', { name: 'Open it' }));
      expect(screen.getByRole('textbox', { name: 'Name' })).toHaveFocus();
      unmount();

      render(<Harness Overlay={Overlay} withInitial />);
      await user.click(screen.getByRole('button', { name: 'Open it' }));
      expect(screen.getByRole('button', { name: 'Save the name' })).toHaveFocus();
    });

    it('keeps Tab and Shift+Tab inside', async () => {
      const user = userEvent.setup();
      render(<Harness Overlay={Overlay} />);
      await user.click(screen.getByRole('button', { name: 'Open it' }));
      await expectFocusKeptIn(screen.getByRole('dialog'));
    });

    it('closes on Escape and gives the focus back to the opener', async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      render(<Harness Overlay={Overlay} onClose={onClose} />);
      const opener = screen.getByRole('button', { name: 'Open it' });
      await user.click(opener);
      await user.keyboard('{Escape}');
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(opener).toHaveFocus();
    });

    it('has one cross, named, at the top, that closes and gives the focus back', async () => {
      const user = userEvent.setup();
      render(<Harness Overlay={Overlay} />);
      const opener = screen.getByRole('button', { name: 'Open it' });
      await user.click(opener);
      const dialog = screen.getByRole('dialog');
      const crosses = within(dialog).getAllByRole('button', { name: 'ui.overlay.close' });
      expect(crosses).toHaveLength(1);
      expect(crosses[0].closest('header')).toBe(dialog.querySelector('header'));
      expect(crosses[0]).toHaveClass('ui-overlay-close');
      await user.click(crosses[0]);
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(opener).toHaveFocus();
    });

    it('closes on a click on the scrim, not on a click inside', async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      render(<Harness Overlay={Overlay} onClose={onClose} />);
      await user.click(screen.getByRole('button', { name: 'Open it' }));
      await user.click(screen.getByRole('textbox', { name: 'Name' }));
      expect(onClose).not.toHaveBeenCalled();
      const scrim = document.querySelector('.ui-overlay-scrim') as HTMLElement;
      expect(scrim).toHaveAttribute('aria-hidden', 'true');
      await user.click(scrim);
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it('closes one layer per Escape, the innermost first', async () => {
      const user = userEvent.setup();
      render(<Harness Overlay={Overlay} nested />);
      await user.click(screen.getByRole('button', { name: 'Open it' }));
      const innerOpener = screen.getByRole('button', { name: 'Open the inner one' });
      await user.click(innerOpener);
      expect(screen.getAllByRole('dialog')).toHaveLength(2);
      expect(screen.getByRole('button', { name: 'Inner action' })).toHaveFocus();
      await user.keyboard('{Escape}');
      expect(screen.getAllByRole('dialog')).toHaveLength(1);
      expect(innerOpener).toHaveFocus();
      await user.keyboard('{Escape}');
      expect(screen.queryByRole('dialog')).toBeNull();
    });

    it('takes the three sizes, md by default', () => {
      const { rerender } = render(<Overlay open onClose={() => {}} title="Sized">x</Overlay>);
      expect(screen.getByRole('dialog')).toHaveAttribute('data-size', 'md');
      for (const size of ['sm', 'md', 'lg'] as const) {
        rerender(<Overlay open onClose={() => {}} title="Sized" size={size}>x</Overlay>);
        expect(screen.getByRole('dialog')).toHaveAttribute('data-size', size);
      }
    });

    it('passes axe (WCAG 2.1 A and AA) while open', async () => {
      const user = userEvent.setup();
      render(<Harness Overlay={Overlay} />);
      await user.click(screen.getByRole('button', { name: 'Open it' }));
      const results = await act(() => runAxe(document.body));
      expect(describeViolations(results)).toEqual([]);
      const passed = results.passes.map(p => p.id);
      expect(passed).toContain('aria-dialog-name');
      expect(passed).toContain('button-name');
    });
  });
}
