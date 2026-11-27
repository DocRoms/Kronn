// KT-1880 — Menu: the WAI-ARIA menu button, and a destructive action that
// can only be the last entry.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Menu, type MenuItem } from './Menu';

const setup = (over: Partial<Record<'rename' | 'duplicate' | 'archive' | 'remove', () => void>> = {}, disabledDuplicate = false) => {
  const items: MenuItem[] = [
    { id: 'rename', label: 'Rename', onSelect: over.rename ?? vi.fn() },
    { id: 'duplicate', label: 'Duplicate', onSelect: over.duplicate ?? vi.fn(), disabled: disabledDuplicate },
    { id: 'archive', label: 'Archive', onSelect: over.archive ?? vi.fn() },
  ];
  const destructive: MenuItem = { id: 'remove', label: 'Delete', onSelect: over.remove ?? vi.fn() };
  render(
    <>
      <Menu label="Actions on this task" items={items} destructiveItem={destructive} />
      <button type="button">Outside</button>
    </>,
  );
  return { user: userEvent.setup(), trigger: screen.getByRole('button', { name: 'Actions on this task' }) };
};

const menu = () => screen.queryByRole('menu');

/** jsdom lays nothing out: give the trigger a box and the menu a size, in the
 *  1024 x 768 window jsdom reports. */
function mockGeometry(trigger: Omit<DOMRect, 'x' | 'y' | 'width' | 'height' | 'toJSON'>, menuSize: { width: number; height: number }) {
  const isMenu = (el: Element) => el.getAttribute('role') === 'menu';
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const r = this.getAttribute('aria-haspopup') === 'menu' ? trigger : { top: 0, bottom: 0, left: 0, right: 0 };
    return { ...r, x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top, toJSON: () => r } as DOMRect;
  });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (this: HTMLElement) {
    return isMenu(this) ? menuSize.width : 0;
  });
  vi.spyOn(Element.prototype, 'scrollHeight', 'get').mockImplementation(function (this: Element) {
    return isMenu(this) ? menuSize.height : 0;
  });
}

afterEach(() => {
  vi.restoreAllMocks();
  delete (Element.prototype as Partial<Element>).scrollIntoView;
});
const names = () => screen.getAllByRole('menuitem').map(i => i.textContent);

describe('Menu', () => {
  it('is a closed menu button named by its label', () => {
    const { trigger } = setup();
    expect(trigger).toHaveAttribute('aria-haspopup', 'menu');
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    expect(menu()).toBeNull();
  });

  it('opens on click on the first item, labelled by its trigger', async () => {
    const { user, trigger } = setup();
    await user.click(trigger);
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
    expect(menu()).toHaveAccessibleName('Actions on this task');
    expect(trigger).toHaveAttribute('aria-controls', menu()!.id);
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
  });

  it('puts the destructive action last, after a separator, marked as such', async () => {
    const { user, trigger } = setup();
    await user.click(trigger);
    expect(names()).toEqual(['Rename', 'Duplicate', 'Archive', 'Delete']);
    const last = screen.getByRole('menuitem', { name: 'Delete' });
    expect(last).toHaveAttribute('data-destructive', 'true');
    expect(last.previousElementSibling).toHaveAttribute('role', 'separator');
    expect(screen.getAllByRole('menuitem').filter(i => i.hasAttribute('data-destructive'))).toHaveLength(1);
  });

  it('opens with the keyboard: Enter on the first item, Arrow Up on the last', async () => {
    const { user, trigger } = setup();
    act(() => trigger.focus());
    await user.keyboard('{Enter}');
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(menu()).toBeNull();
    expect(trigger).toHaveFocus();
    await user.keyboard('{ArrowUp}');
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toHaveFocus();
    await user.keyboard('{Escape}');
    await user.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
  });

  it('focuses an item only once the menu is visible (a browser refuses focus on a hidden element)', async () => {
    const seen: string[] = [];
    const focus = HTMLElement.prototype.focus;
    vi.spyOn(HTMLElement.prototype, 'focus').mockImplementation(function (this: HTMLElement, options?: FocusOptions) {
      const layer = this.closest<HTMLElement>('[role="menu"]');
      if (layer) {
        const computed = getComputedStyle(layer);
        seen.push(`${this.textContent} visibility=${layer.style.visibility || computed.visibility} display=${layer.style.display || computed.display}`);
      }
      focus.call(this, options);
    });
    const { user, trigger } = setup();
    act(() => trigger.focus());
    await user.keyboard('{Enter}');
    await user.keyboard('{ArrowDown}');
    expect(seen.length).toBeGreaterThan(0);
    for (const entry of seen) {
      expect(entry).not.toMatch(/visibility=hidden|display=none/);
    }
    expect(seen.at(-1)).toMatch(/^Duplicate /);
  });

  it('moves with the arrows, wraps, skips a disabled item, and goes Home and End', async () => {
    const { user, trigger } = setup({}, true);
    await user.click(trigger);
    await user.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Archive' })).toHaveFocus();
    await user.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toHaveFocus();
    await user.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
    await user.keyboard('{ArrowUp}');
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toHaveFocus();
    await user.keyboard('{Home}');
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
    await user.keyboard('{End}');
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toHaveFocus();
  });

  it('jumps to the next item starting with a typed letter', async () => {
    const { user, trigger } = setup();
    await user.click(trigger);
    await user.keyboard('a');
    expect(screen.getByRole('menuitem', { name: 'Archive' })).toHaveFocus();
    await user.keyboard('d');
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toHaveFocus();
    await user.keyboard('d');
    expect(screen.getByRole('menuitem', { name: 'Duplicate' })).toHaveFocus();
  });

  it('runs the chosen item once, closes and gives focus back to the trigger', async () => {
    const archive = vi.fn();
    const remove = vi.fn();
    const { user, trigger } = setup({ archive, remove });
    await user.click(trigger);
    await user.keyboard('{ArrowDown}{ArrowDown}{Enter}');
    expect(archive).toHaveBeenCalledTimes(1);
    expect(menu()).toBeNull();
    expect(trigger).toHaveFocus();

    await user.click(trigger);
    await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
    expect(remove).toHaveBeenCalledTimes(1);
  });

  it('closes on Tab, back on the trigger', async () => {
    const { user, trigger } = setup();
    await user.click(trigger);
    await user.keyboard('{Tab}');
    expect(menu()).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it('closes on a press outside without taking focus back', async () => {
    const { user, trigger } = setup();
    await user.click(trigger);
    await user.click(screen.getByRole('button', { name: 'Outside' }));
    expect(menu()).toBeNull();
    expect(screen.getByRole('button', { name: 'Outside' })).toHaveFocus();
  });

  it('stops Escape there, so a dialog around it stays open', async () => {
    const onKeyDown = vi.fn();
    render(
      <div onKeyDown={e => onKeyDown(e.key)}>
        <Menu label="Row actions" items={[{ id: 'a', label: 'Open', onSelect: vi.fn() }]} />
      </div>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Row actions' }));
    fireEvent.keyDown(screen.getByRole('menuitem', { name: 'Open' }), { key: 'Escape' });
    expect(menu()).toBeNull();
    expect(onKeyDown).not.toHaveBeenCalledWith('Escape');
  });

  it('stays inside the window: flips above a trigger at the bottom, clamped to the side', async () => {
    // The trigger sits in the bottom-right corner of the 1024 x 768 window.
    mockGeometry({ top: 730, bottom: 762, left: 990, right: 1022 }, { width: 160, height: 155 });
    const { user, trigger } = setup();
    await user.click(trigger);
    expect(menu()!.style.top).toBe(`${730 - 4 - 155}px`);
    // Lined up on the trigger's end (1022 - 160) would cross the 8 px margin.
    expect(menu()!.style.left).toBe(`${1024 - 8 - 160}px`);
    expect(menu()!.style.maxHeight).toBe('');
    expect(menu()!.style.visibility).toBe('');
  });

  it('scrolls inside a max-height when taller than the window, the focused item in view', async () => {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    // Room below: 768 - 8 - (332 + 4) = 424; above: 300 - 4 - 8 = 288.
    mockGeometry({ top: 300, bottom: 332, left: 400, right: 432 }, { width: 160, height: 900 });
    const { user, trigger } = setup();
    await user.click(trigger);
    expect(menu()!.style.top).toBe('336px');
    expect(menu()!.style.maxHeight).toBe('424px');
    scrollIntoView.mockClear();
    await user.keyboard('{End}');
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toHaveFocus();
    expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' });
    expect(scrollIntoView.mock.contexts.at(-1)).toBe(screen.getByRole('menuitem', { name: 'Delete' }));
  });

  it('can show its label on a text trigger, and has no separator without other items', async () => {
    render(<Menu label="More" showLabel items={[]} destructiveItem={{ id: 'x', label: 'Delete', onSelect: vi.fn() }} />);
    const trigger = screen.getByRole('button', { name: 'More' });
    expect(trigger).toHaveTextContent('More');
    await userEvent.click(trigger);
    expect(names()).toEqual(['Delete']);
    expect(screen.queryByRole('separator')).toBeNull();
  });
});
