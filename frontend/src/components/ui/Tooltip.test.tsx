// KT-1880 — Tooltip: reachable by keyboard, dismissible with Escape,
// hoverable, and wired as the description of what it explains (WCAG 1.4.13).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Tooltip, TOOLTIP_CLOSE_DELAY_MS } from './Tooltip';
import { Dialog } from './Dialog';
import { Drawer } from './Drawer';
import { IconButton } from './IconButton';
import { placeFocus } from '../../hooks/useDialogFocus';

const tip = () => document.querySelector('[role="tooltip"]');

afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });

describe('Tooltip', () => {
  it('is closed at first and leaves the anchor without a description', () => {
    render(<Tooltip content="Runs every hour"><button type="button">Schedule</button></Tooltip>);
    expect(tip()).toBeNull();
    expect(screen.getByRole('button')).not.toHaveAttribute('aria-describedby');
  });

  it('opens on keyboard focus and describes the anchor', async () => {
    const user = userEvent.setup();
    render(<Tooltip content="Runs every hour"><button type="button">Schedule</button></Tooltip>);
    await user.tab();
    const button = screen.getByRole('button', { name: 'Schedule' });
    expect(button).toHaveFocus();
    expect(tip()).toHaveTextContent('Runs every hour');
    expect(button).toHaveAccessibleDescription('Runs every hour');
    await user.tab();
    expect(tip()).toBeNull();
  });

  it('closes on Escape without moving focus, and opens again on the next focus', async () => {
    const user = userEvent.setup();
    render(<><Tooltip content="Runs every hour"><button type="button">Schedule</button></Tooltip><button type="button">Next</button></>);
    await user.tab();
    expect(tip()).not.toBeNull();
    await user.keyboard('{Escape}');
    expect(tip()).toBeNull();
    expect(screen.getByRole('button', { name: 'Schedule' })).toHaveFocus();
    await user.tab();
    await user.tab({ shift: true });
    expect(tip()).not.toBeNull();
  });

  it('opens on hover and stays while the pointer moves onto it', () => {
    vi.useFakeTimers();
    render(<Tooltip content="Runs every hour"><button type="button">Schedule</button></Tooltip>);
    const anchor = screen.getByRole('button').parentElement!;
    fireEvent.mouseEnter(anchor);
    expect(tip()).not.toBeNull();

    fireEvent.mouseLeave(anchor);
    fireEvent.mouseEnter(tip()!);
    act(() => { vi.advanceTimersByTime(TOOLTIP_CLOSE_DELAY_MS * 2); });
    expect(tip()).not.toBeNull();

    fireEvent.mouseLeave(tip()!);
    act(() => { vi.advanceTimersByTime(TOOLTIP_CLOSE_DELAY_MS); });
    expect(tip()).toBeNull();
  });

  it('closes on Escape when opened by hover too', () => {
    render(<Tooltip content="Runs every hour"><button type="button">Schedule</button></Tooltip>);
    fireEvent.mouseEnter(screen.getByRole('button').parentElement!);
    expect(tip()).not.toBeNull();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(tip()).toBeNull();
  });

  it('keeps a description the anchor already had', async () => {
    render(
      <>
        <p id="own">Own hint</p>
        <Tooltip content="Runs every hour"><button type="button" aria-describedby="own">Schedule</button></Tooltip>
      </>,
    );
    const button = screen.getByRole('button');
    expect(button).toHaveAttribute('aria-describedby', 'own');
    await userEvent.setup().tab();
    expect(button.getAttribute('aria-describedby')).toMatch(/^own \S+$/);
    expect(button).toHaveAccessibleDescription('Own hint Runs every hour');
  });

  it('stays inside the window on an anchor at its right edge', async () => {
    // jsdom lays nothing out: the anchor sits in the top-right corner of the
    // 1024 x 768 window, the tooltip is 120 x 28.
    const box = { top: 4, bottom: 28, left: 1000, right: 1024 };
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const r = this.classList.contains('ui-tooltip-anchor') ? box : { top: 0, bottom: 0, left: 0, right: 0 };
      return { ...r, x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top, toJSON: () => r } as DOMRect;
    });
    const isTip = (el: Element) => el.getAttribute('role') === 'tooltip';
    vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return isTip(this) ? 120 : 0;
    });
    vi.spyOn(Element.prototype, 'scrollHeight', 'get').mockImplementation(function (this: Element) {
      return isTip(this) ? 28 : 0;
    });
    render(<Tooltip content="Settings"><button type="button">S</button></Tooltip>);
    await userEvent.setup().tab();
    const el = tip() as HTMLElement;
    // No room above the anchor: below it, and pulled left of the right edge.
    expect(el.style.top).toBe(`${28 + 4}px`);
    expect(el.style.left).toBe(`${1024 - 8 - 120}px`);
  });

  it('is rendered in <body>, so a clipped container does not hide it', async () => {
    const { container } = render(
      <div style={{ overflow: 'hidden' }}>
        <Tooltip content="Runs every hour"><button type="button">Schedule</button></Tooltip>
      </div>,
    );
    await userEvent.setup().tab();
    expect(container.contains(tip())).toBe(false);
    expect(tip()!.parentElement).toBe(document.body);
    expect((tip() as HTMLElement).style.position).toBe('fixed');
  });

  // KT-2014 — the dialog listens on body and stops Escape there: an open
  // tooltip inside it takes Escape first, one layer per Escape (WCAG 1.4.13).
  for (const [name, Overlay] of [['Dialog', Dialog], ['Drawer', Drawer]] as const) {
    it(`inside a ${name}, the first Escape hides the tooltip and leaves the ${name} open, the second closes it`, async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      render(
        <Overlay open onClose={onClose} title="Schedule">
          <Tooltip content="Runs every hour"><button type="button">Help</button></Tooltip>
        </Overlay>,
      );
      fireEvent.mouseEnter(screen.getByRole('button', { name: 'Help' }).parentElement!);
      expect(tip()).not.toBeNull();
      await user.keyboard('{Escape}');
      expect(tip()).toBeNull();
      expect(onClose).not.toHaveBeenCalled();
      expect(screen.getByRole('dialog')).toBeInTheDocument();
      await user.keyboard('{Escape}');
      expect(onClose).toHaveBeenCalledTimes(1);
    });
  }

  it('leaves Escape to the page once it is hidden, and ignores it during an IME composition', () => {
    const page = vi.fn();
    document.addEventListener('keydown', page);
    try {
      render(<Tooltip content="Runs every hour"><button type="button">Schedule</button></Tooltip>);
      fireEvent.mouseEnter(screen.getByRole('button').parentElement!);
      fireEvent.keyDown(document, { key: 'Escape', isComposing: true });
      expect(tip()).not.toBeNull();
      page.mockClear();
      fireEvent.keyDown(document, { key: 'Escape' });
      expect(tip()).toBeNull();
      expect(page).not.toHaveBeenCalled();
      fireEvent.keyDown(document, { key: 'Escape' });
      expect(page).toHaveBeenCalledTimes(1);
    } finally {
      document.removeEventListener('keydown', page);
    }
  });

  // KT-2014 — a tooltip owns an Escape only when the user put it there (the
  // pointer, or a focus the user moved). One a layer showed by placing the
  // focus hides on the Escape and lets it through to the layer.
  const cross = () => document.querySelector<HTMLButtonElement>('.ui-overlay-close')!;
  for (const [name, Overlay] of [['Dialog', Dialog], ['Drawer', Drawer]] as const) {
    it(`a ${name} whose initial focus is its cross closes on one Escape`, async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      render(<Overlay open onClose={onClose} title="Schedule"><p>Nothing to act on</p></Overlay>);
      expect(cross()).toHaveFocus();
      await user.keyboard('{Escape}');
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(tip()).toBeNull();
    });

    it(`inside a ${name}, the tooltip of the cross the user tabbed onto takes the first Escape`, async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      render(<Overlay open onClose={onClose} title="Schedule"><button type="button">Help</button></Overlay>);
      expect(screen.getByRole('button', { name: 'Help' })).toHaveFocus();
      await user.tab();
      expect(cross()).toHaveFocus();
      expect(tip()).not.toBeNull();
      await user.keyboard('{Escape}');
      expect(tip()).toBeNull();
      expect(onClose).not.toHaveBeenCalled();
      expect(cross()).toHaveFocus();
      await user.keyboard('{Escape}');
      expect(onClose).toHaveBeenCalledTimes(1);
    });
  }

  it('hides every open tooltip on the same Escape, one opened by focus and one by hover', async () => {
    const user = userEvent.setup();
    render(<><IconButton label="Edit" icon={<span />} /><IconButton label="Copy" icon={<span />} /></>);
    await user.tab();
    expect(screen.getByRole('button', { name: 'Edit' })).toHaveFocus();
    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Copy' }).parentElement!);
    expect(document.querySelectorAll('[role="tooltip"]')).toHaveLength(2);
    await user.keyboard('{Escape}');
    expect(document.querySelectorAll('[role="tooltip"]')).toHaveLength(0);
  });

  it('hides when its anchor is activated: Enter, Space or a press', async () => {
    const user = userEvent.setup();
    render(<><button type="button">Before</button><IconButton label="Edit" icon={<span />} /></>);
    const button = screen.getByRole('button', { name: 'Edit' });
    for (const key of ['{Enter}', ' ']) {
      act(() => screen.getByRole('button', { name: 'Before' }).focus());
      await user.tab();
      expect(button).toHaveFocus();
      expect(tip()).not.toBeNull();
      await user.keyboard(key);
      expect(tip()).toBeNull();
      expect(button).toHaveFocus();
    }
    fireEvent.mouseEnter(button.parentElement!);
    expect(tip()).not.toBeNull();
    fireEvent.pointerDown(button);
    expect(tip()).toBeNull();
  });

  it('shown by a focus a layer placed, it hides on Escape and lets it through', () => {
    const page = vi.fn();
    document.addEventListener('keydown', page);
    try {
      render(<IconButton label="Edit" icon={<span />} />);
      act(() => placeFocus(screen.getByRole('button', { name: 'Edit' })));
      expect(tip()).not.toBeNull();
      fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
      expect(tip()).toBeNull();
      expect(page).toHaveBeenCalledTimes(1);
    } finally {
      document.removeEventListener('keydown', page);
    }
  });
});
