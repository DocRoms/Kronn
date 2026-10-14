import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { isPlainLeftClick, newTabClickProps } from '../newTabNavigation';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

function Row({ path, onOpen, onStar }: { path: string | null; onOpen: () => void; onStar?: () => void }) {
  return (
    <div role="presentation">
      <button type="button" data-testid="row" {...newTabClickProps(path)} onClick={onOpen}>
        <span data-testid="row-label">Row</span>
      </button>
      <div data-testid="card" {...newTabClickProps(path)} onClick={onOpen}>
        <span data-testid="card-label">Card</span>
        <button type="button" data-testid="star" onClick={onStar}>★</button>
      </div>
    </div>
  );
}

describe('newTabClickProps', () => {
  it('leaves a plain click to the element, and opens a modified or middle click in a new tab', () => {
    const open = vi.fn();
    vi.spyOn(window, 'open').mockImplementation(open);
    const onOpen = vi.fn();
    render(<Row path="/discussions/disc-1" onOpen={onOpen} />);

    fireEvent.click(screen.getByTestId('row-label'));
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(open).not.toHaveBeenCalled();

    for (const init of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }]) {
      fireEvent.click(screen.getByTestId('row-label'), init);
    }
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledTimes(3);
    expect(open).toHaveBeenLastCalledWith(`${window.location.origin}/discussions/disc-1`, '_blank', 'noopener,noreferrer');

    // A middle press does not start the browser's autoscroll, its click opens the tab.
    const press = new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 1 });
    screen.getByTestId('row').dispatchEvent(press);
    expect(press.defaultPrevented).toBe(true);
    fireEvent(screen.getByTestId('row'), new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 1 }));
    expect(open).toHaveBeenCalledTimes(4);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('leaves a click on a control nested in the element to that control', () => {
    const open = vi.fn();
    vi.spyOn(window, 'open').mockImplementation(open);
    const onStar = vi.fn();
    render(<Row path="/projects/p-1" onOpen={vi.fn()} onStar={onStar} />);

    fireEvent.click(screen.getByTestId('star'), { ctrlKey: true });
    fireEvent(screen.getByTestId('star'), new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 1 }));

    expect(onStar).toHaveBeenCalledTimes(1);
    expect(open).not.toHaveBeenCalled();
    // The card itself still opens.
    fireEvent.click(screen.getByTestId('card-label'), { ctrlKey: true });
    expect(open).toHaveBeenCalledWith(`${window.location.origin}/projects/p-1`, '_blank', 'noopener,noreferrer');
  });

  it('does nothing without an address', () => {
    const open = vi.fn();
    vi.spyOn(window, 'open').mockImplementation(open);
    const onOpen = vi.fn();
    render(<Row path={null} onOpen={onOpen} />);
    fireEvent.click(screen.getByTestId('row-label'), { ctrlKey: true });
    expect(open).not.toHaveBeenCalled();
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('tells the plain left click apart', () => {
    const click = { button: 0, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false };
    expect(isPlainLeftClick(click)).toBe(true);
    expect(isPlainLeftClick({ ...click, ctrlKey: true })).toBe(false);
    expect(isPlainLeftClick({ ...click, button: 1 })).toBe(false);
  });
});
