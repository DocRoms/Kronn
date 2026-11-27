// KT-2014 — useAnchoredLayer: the anchored layer among the other layers. A
// tooltip inside it takes the first Escape, a layer opened from it (a Menu, a
// nested anchored layer, each a portal in body) does not count as outside,
// and it still answers Escape once its focused control is gone.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useAnchoredLayer } from './useAnchoredLayer';
import { Tooltip } from './Tooltip';
import { Menu } from './Menu';
import { IconButton } from './IconButton';

function Layer({ name, onClose, children }: { name: string; onClose?: () => void; children?: ReactNode }) {
  const anchorRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const close = () => { onClose?.(); setOpen(false); };
  const { ref: layerRef, style } = useAnchoredLayer<HTMLDivElement>({ anchorRef, open, placement: 'bottom', align: 'start', onClose: close });
  return (
    <>
      <button ref={anchorRef} type="button" onClick={() => setOpen(o => !o)}>{`open ${name}`}</button>
      {open && createPortal(
        <div ref={layerRef} role="dialog" aria-label={name} style={style}>{children}</div>,
        document.body,
      )}
    </>
  );
}

const layer = (name: string) => screen.queryByRole('dialog', { name });
const tip = () => document.querySelector('[role="tooltip"]');

afterEach(() => { vi.restoreAllMocks(); });

describe('useAnchoredLayer (KT-2014)', () => {
  it('a first Escape hides a tooltip open inside it, the second closes it and returns focus to the anchor', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Layer name="details" onClose={onClose}><Tooltip content="Last seen"><button type="button">Status</button></Tooltip></Layer>);
    await user.click(screen.getByRole('button', { name: 'open details' }));
    expect(screen.getByRole('button', { name: 'Status' })).toHaveFocus();
    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Status' }).parentElement!);
    expect(tip()).not.toBeNull();
    await user.keyboard('{Escape}');
    expect(tip()).toBeNull();
    expect(layer('details')).not.toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    expect(layer('details')).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'open details' })).toHaveFocus();
  });

  it('a press in a Menu opened from it lands, and does not close it; a press outside still does', async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(
      <>
        <Layer name="participants">
          <Menu label="Participant actions" items={[{ id: 'mute', label: 'Mute', onSelect }]} />
        </Layer>
        <button type="button">Outside</button>
      </>,
    );
    await user.click(screen.getByRole('button', { name: 'open participants' }));
    await user.click(screen.getByRole('button', { name: 'Participant actions' }));
    const item = screen.getByRole('menuitem', { name: 'Mute' });
    // The menu is a portal in body, outside the layer's own node.
    expect(layer('participants')!.contains(item)).toBe(false);
    await user.click(item);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(layer('participants')).not.toBeNull();
    await user.click(screen.getByRole('button', { name: 'Outside' }));
    expect(layer('participants')).toBeNull();
  });

  it('a press in a nested anchored layer does not close the parent', async () => {
    const user = userEvent.setup();
    const choose = vi.fn();
    render(
      <Layer name="parent">
        <Layer name="child"><button type="button" onClick={choose}>Pick</button></Layer>
      </Layer>,
    );
    await user.click(screen.getByRole('button', { name: 'open parent' }));
    await user.click(screen.getByRole('button', { name: 'open child' }));
    await user.click(screen.getByRole('button', { name: 'Pick' }));
    expect(choose).toHaveBeenCalledTimes(1);
    expect(layer('parent')).not.toBeNull();
    expect(layer('child')).not.toBeNull();
  });

  it('answers Escape when its focused control was removed by a refresh', async () => {
    const user = userEvent.setup();
    function Refreshing({ onClose }: { onClose: () => void }) {
      const [shown, setShown] = useState(true);
      return (
        <Layer name="list" onClose={onClose}>
          {shown && <button type="button" onClick={() => setShown(false)}>Row</button>}
          <span>empty</span>
        </Layer>
      );
    }
    const onClose = vi.fn();
    render(<Refreshing onClose={onClose} />);
    await user.click(screen.getByRole('button', { name: 'open list' }));
    const row = screen.getByRole('button', { name: 'Row' });
    expect(row).toHaveFocus();
    act(() => row.click());
    expect(document.activeElement).toBe(document.body);
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(layer('list')).toBeNull();
    expect(screen.getByRole('button', { name: 'open list' })).toHaveFocus();
  });

  it('closed with Escape, it gives focus back to an IconButton anchor whose tooltip does not take the next Escape', async () => {
    const user = userEvent.setup();
    function IconLayer({ onClose }: { onClose: () => void }) {
      const anchorRef = useRef<HTMLButtonElement>(null);
      const [open, setOpen] = useState(false);
      const close = () => { onClose(); setOpen(false); };
      const { ref: layerRef, style } = useAnchoredLayer<HTMLDivElement>({ anchorRef, open, placement: 'bottom', align: 'start', onClose: close });
      return (
        <>
          <IconButton ref={anchorRef} label="Participants" icon={<span />} onClick={() => setOpen(o => !o)} />
          {open && createPortal(
            <div ref={layerRef} role="dialog" aria-label="participants" style={style}><button type="button">Ada</button></div>,
            document.body,
          )}
        </>
      );
    }
    const onClose = vi.fn();
    const page = vi.fn();
    const onPageKey = (e: KeyboardEvent) => { if (e.key === 'Escape') page(); };
    document.addEventListener('keydown', onPageKey);
    try {
      render(<IconLayer onClose={onClose} />);
      const anchor = screen.getByRole('button', { name: 'Participants' });
      await user.tab();
      expect(anchor).toHaveFocus();
      await user.keyboard('{Enter}');
      expect(screen.getByRole('button', { name: 'Ada' })).toHaveFocus();
      await user.keyboard('{Escape}');
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(layer('participants')).toBeNull();
      expect(anchor).toHaveFocus();
      expect(page).not.toHaveBeenCalled();
      await user.keyboard('{Escape}');
      expect(page).toHaveBeenCalledTimes(1);
      expect(tip()).toBeNull();
    } finally {
      document.removeEventListener('keydown', onPageKey);
    }
  });
});
