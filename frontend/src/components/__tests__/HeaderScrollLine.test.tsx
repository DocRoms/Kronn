// KT-1936 review round 2 — the room header's agent line (owner decision
// 2026-11-20, option A): ONE line that scrolls sideways, the main agent first,
// the right edge faded while something lies past it, and the participants'
// « +N » counter pinned at the line's right end, outside what scrolls. jsdom
// has no layout: the boxes are stubbed — a 200px line holding a 120px trigger
// and three 80px chips (360px of content).
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { HeaderScrollLine } from '../HeaderScrollLine';
import { WHEEL_LINE_PX } from '../../lib/stripOverflow';
import { ParticipantsStrip } from '../ParticipantsStrip';

const LINE = 200;
const TRIGGER = 120;
const CHIP = 80;
const restore: Array<() => void> = [];
const scrolled = new WeakMap<Element, number>();

function stub<K extends keyof HTMLElement>(prop: K, descriptor: PropertyDescriptor) {
  const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, prop)
    ?? Object.getOwnPropertyDescriptor(Element.prototype, prop);
  Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, ...descriptor });
  restore.push(() => {
    if (original) Object.defineProperty(HTMLElement.prototype, prop, original);
    else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop as string];
  });
}

const isScroller = (el: Element) => el.classList.contains('kr-header-line-scroller');
const scrollerOf = (el: Element) => el.closest('.kr-header-line')?.querySelector('.kr-header-line-scroller');
const box = (left: number, width: number) => ({
  left, right: left + width, top: 0, bottom: 24, width, height: 24, x: left, y: 0, toJSON: () => ({}),
}) as DOMRect;

beforeEach(() => {
  stub('scrollLeft', {
    get(this: HTMLElement) { return scrolled.get(this) ?? 0; },
    set(this: HTMLElement, value: number) { scrolled.set(this, value); },
  });
  stub('clientWidth', { get(this: HTMLElement) { return isScroller(this) ? LINE : CHIP; } });
  stub('scrollWidth', {
    get(this: HTMLElement) { return isScroller(this) ? TRIGGER + this.querySelectorAll('.kr-participants-strip-item').length * CHIP : CHIP; },
  });
  stub('getBoundingClientRect', {
    value(this: HTMLElement) {
      if (isScroller(this)) return box(0, LINE);
      if (this.classList.contains('kr-participants-strip-item')) {
        const index = Array.from(this.parentElement!.children).indexOf(this);
        const offset = scrolled.get(scrollerOf(this)!) ?? 0;
        return box(TRIGGER + index * CHIP - offset, CHIP);
      }
      return box(0, 0);
    },
  });
});
afterEach(() => { while (restore.length) restore.pop()!(); });

const entries = (n: number) => Array.from({ length: n }, (_, i) => ({
  key: i,
  chip: <button type="button" className="disc-participant-chip">{`agent-${i}`}</button>,
  row: <span>{`row-${i}`}</span>,
  onSelect: () => {},
}));

function renderLine(chips = 3) {
  return render(
    <HeaderScrollLine>
      <span className="disc-native-agent-control"><button type="button">@opencode</button></span>
      <ParticipantsStrip entries={entries(chips)} moreLabel={n => `all ${n}`} allLabel="All participants" />
      <span className="trust-badge">trust</span>
    </HeaderScrollLine>,
  );
}

const line = () => screen.getByTestId('header-scroll-line');
const scroller = () => screen.getByTestId('header-scroll-line-scroller');

describe('HeaderScrollLine', () => {
  it('starts with the main agent, and pins the « +N » counter outside what scrolls', () => {
    renderLine();
    const track = scroller().firstElementChild!;
    expect(track.firstElementChild).toHaveClass('disc-native-agent-control');
    expect(track.lastElementChild).toHaveClass('trust-badge');
    const more = screen.getByTestId('participants-more');
    expect(more.parentElement).toHaveClass('kr-header-line-end');
    expect(scroller().contains(more)).toBe(false);
    expect(line().lastElementChild).toBe(more.parentElement);
  });

  it('counts the chips not fully in the line and fades its right edge while it scrolls further', () => {
    renderLine();
    // 200px from 0: the trigger (0–120) and chip 0 (120–200) are in view;
    // chips 1 and 2 lie past the edge.
    expect(screen.getByTestId('participants-more')).toHaveTextContent('+2');
    expect(screen.getByRole('button', { name: 'all 3' })).toBeInTheDocument();
    expect(line()).toHaveAttribute('data-fade-end', 'true');
    // The strip does not fade by itself on the line: the line does.
    expect(document.querySelector('.kr-participants-strip')).not.toHaveAttribute('data-fade-end');
  });

  it('follows the scroll: at the end the fade is off and the counter counts what went out on the left', () => {
    renderLine();
    act(() => {
      scroller().scrollLeft = TRIGGER + 3 * CHIP - LINE;
      fireEvent.scroll(scroller());
    });
    // Chip 0 (−40–40) went out on the left; chips 1 and 2 are in view.
    expect(screen.getByTestId('participants-more')).toHaveTextContent('+1');
    expect(line()).not.toHaveAttribute('data-fade-end');
  });

  it('shows no counter and no fade when everything fits', () => {
    renderLine(1);
    expect(screen.queryByTestId('participants-more')).toBeNull();
    expect(line()).not.toHaveAttribute('data-fade-end');
  });

  it('turns a plain mouse wheel into a sideways scroll, within the line\'s room', () => {
    renderLine();
    act(() => { fireEvent.wheel(scroller(), { deltaY: 100, deltaX: 0 }); });
    expect(scroller().scrollLeft).toBe(100);
    act(() => { fireEvent.wheel(scroller(), { deltaY: 500, deltaX: 0 }); });
    expect(scroller().scrollLeft).toBe(TRIGGER + 3 * CHIP - LINE);
  });

  // KT-2015 — a wheel that reports lines (deltaMode 1) or pages (2) moved the
  // line by 1 to 3 px a notch: its delta was read as pixels.
  it('counts a wheel in line or page mode in pixels, and lets the page scroll when the line cannot move', () => {
    renderLine();
    const room = TRIGGER + 3 * CHIP - LINE;
    act(() => { fireEvent.wheel(scroller(), { deltaY: 1, deltaX: 0, deltaMode: 1 }); });
    expect(scroller().scrollLeft).toBe(WHEEL_LINE_PX);
    act(() => { fireEvent.wheel(scroller(), { deltaY: -1, deltaX: 0, deltaMode: 1 }); });
    expect(scroller().scrollLeft).toBe(0);
    act(() => { fireEvent.wheel(scroller(), { deltaY: 1, deltaX: 0, deltaMode: 2 }); });
    expect(scroller().scrollLeft).toBe(room);
    // At the end the line cannot move: the wheel is not consumed, the page scrolls.
    let consumed = true;
    act(() => { consumed = !fireEvent.wheel(scroller(), { deltaY: 3, deltaX: 0, deltaMode: 1 }); });
    expect(consumed).toBe(false);
    expect(scroller().scrollLeft).toBe(room);
    // Moving, it is consumed.
    act(() => { consumed = !fireEvent.wheel(scroller(), { deltaY: -1, deltaX: 0, deltaMode: 1 }); });
    expect(consumed).toBe(true);
    expect(scroller().scrollLeft).toBe(room - WHEEL_LINE_PX);
  });

  it('opens the full list from the pinned counter, as a layer in document.body', async () => {
    renderLine();
    await act(async () => { fireEvent.click(screen.getByTestId('participants-more')); });
    const list = screen.getByRole('dialog', { name: 'All participants' });
    expect(list.parentElement).toBe(document.body);
    expect(screen.getByRole('button', { name: 'row-0' })).toHaveFocus();
  });
});
