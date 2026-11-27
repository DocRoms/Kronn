// KT-1880 — where a Tooltip or a Menu sits next to its anchor, and how it
// stays inside the window.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { ANCHOR_GAP, VIEWPORT_MARGIN, anchoredStyle, useAnchoredStyle } from './anchor';

const viewport = { width: 1000, height: 900 };
const box = { top: 100, bottom: 130, left: 200, right: 240 };
const small = { width: 80, height: 30 };

describe('anchoredStyle', () => {
  it('puts a top layer above the anchor, centred', () => {
    expect(anchoredStyle(box, small, viewport, 'top', 'center')).toEqual({
      position: 'fixed', top: 100 - ANCHOR_GAP - 30, left: 220 - 40,
    });
  });

  it('puts a bottom layer below, lined up with the start or the end edge', () => {
    expect(anchoredStyle(box, small, viewport, 'bottom', 'start')).toEqual({
      position: 'fixed', top: 130 + ANCHOR_GAP, left: 200,
    });
    expect(anchoredStyle(box, small, viewport, 'bottom', 'end')).toEqual({
      position: 'fixed', top: 130 + ANCHOR_GAP, left: 240 - 80,
    });
  });

  it('flips a top layer below an anchor at the top of the window', () => {
    const high = { top: 8, bottom: 38, left: 400, right: 440 };
    expect(anchoredStyle(high, small, viewport, 'top', 'center')).toEqual({
      position: 'fixed', top: 38 + ANCHOR_GAP, left: 380,
    });
  });

  it('flips a bottom layer above an anchor at the bottom of the window', () => {
    // The case measured in review: trigger bottom at 894 px in a 900 px window.
    const low = { top: 862, bottom: 894, left: 400, right: 432 };
    const menu = { width: 160, height: 155 };
    expect(anchoredStyle(low, menu, viewport, 'bottom', 'end')).toEqual({
      position: 'fixed', top: 862 - ANCHOR_GAP - 155, left: 432 - 160,
    });
  });

  it('keeps the asked side when the other side has no more room', () => {
    // 20 px of room below, 10 px above: stays below, shrunk to the room.
    const tight = { width: 1000, height: 104 };
    const anchor = { top: 22, bottom: 72, left: 0, right: 40 };
    const style = anchoredStyle(anchor, small, tight, 'bottom', 'start');
    expect(style.top).toBe(72 + ANCHOR_GAP);
    expect(style.maxHeight).toBe(104 - VIEWPORT_MARGIN - 72 - ANCHOR_GAP);
  });

  it('clamps a layer at the right edge back inside the window', () => {
    const right = { top: 400, bottom: 432, left: 960, right: 992 };
    expect(anchoredStyle(right, small, viewport, 'top', 'center').left).toBe(1000 - VIEWPORT_MARGIN - 80);
    expect(anchoredStyle(right, small, viewport, 'bottom', 'start').left).toBe(1000 - VIEWPORT_MARGIN - 80);
  });

  it('clamps a layer at the left edge back inside the window', () => {
    const left = { top: 400, bottom: 432, left: 2, right: 34 };
    expect(anchoredStyle(left, small, viewport, 'top', 'center').left).toBe(VIEWPORT_MARGIN);
    const menu = { width: 160, height: 100 };
    expect(anchoredStyle(left, menu, viewport, 'bottom', 'end').left).toBe(VIEWPORT_MARGIN);
  });

  it('pins a layer wider than the window to the left margin', () => {
    const narrow = { width: 300, height: 900 };
    const wide = { width: 400, height: 30 };
    expect(anchoredStyle({ top: 400, bottom: 432, left: 250, right: 282 }, wide, narrow, 'bottom', 'end').left)
      .toBe(VIEWPORT_MARGIN);
  });

  it('gives a menu taller than both sides the room of the better side as max-height', () => {
    const short = { width: 1000, height: 400 };
    const anchor = { top: 100, bottom: 132, left: 400, right: 432 };
    const tall = { width: 160, height: 600 };
    // Below: 400 - 8 - 136 = 256; above: 100 - 4 - 8 = 88. Below is better.
    expect(anchoredStyle(anchor, tall, short, 'bottom', 'end')).toEqual({
      position: 'fixed', top: 132 + ANCHOR_GAP, left: 432 - 160, maxHeight: 256,
    });
    // Asked on top, it flips below, the side with more room, and still scrolls.
    expect(anchoredStyle(anchor, tall, short, 'top', 'end')).toEqual({
      position: 'fixed', top: 132 + ANCHOR_GAP, left: 432 - 160, maxHeight: 256,
    });
    // An anchor low in the window: above is better, the layer ends at the anchor.
    const low = { top: 300, bottom: 332, left: 400, right: 432 };
    expect(anchoredStyle(low, tall, short, 'bottom', 'end')).toEqual({
      position: 'fixed', top: VIEWPORT_MARGIN, left: 432 - 160, maxHeight: 300 - ANCHOR_GAP - VIEWPORT_MARGIN,
    });
  });

  it('measures its gap and margin in the spacing tokens', () => {
    const tokens = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'styles', 'tokens.css'), 'utf8');
    expect(tokens).toContain(`--kr-space-1: ${ANCHOR_GAP}px;`);
    expect(tokens).toContain(`--kr-space-2: ${VIEWPORT_MARGIN}px;`);
  });
});

// KT-2014 — when the placement is redone: not on the layer's own scroll, not
// into a new style object when nothing moved, and on a size change.
describe('useAnchoredStyle', () => {
  const observers: Array<{ callback: ResizeObserverCallback; observed: Element[] }> = [];
  class FakeResizeObserver {
    observed: Element[] = [];
    constructor(public callback: ResizeObserverCallback) { observers.push(this); }
    observe(node: Element) { this.observed.push(node); }
    unobserve() {}
    disconnect() { this.observed = []; }
  }

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    observers.length = 0;
    document.body.innerHTML = '';
  });

  /** An anchor at 100..130 px and a 160 x `height` layer holding a list. */
  function mount(height: { value: number }) {
    const anchor = document.createElement('button');
    const layer = document.createElement('div');
    const list = document.createElement('ul');
    layer.append(list);
    document.body.append(anchor, layer);
    const rect = vi.spyOn(anchor, 'getBoundingClientRect').mockImplementation(() => {
      const r = { top: 100, bottom: 130, left: 200, right: 240 };
      return { ...r, x: r.left, y: r.top, width: 40, height: 30, toJSON: () => r } as DOMRect;
    });
    Object.defineProperty(layer, 'offsetWidth', { configurable: true, get: () => 160 });
    Object.defineProperty(layer, 'scrollHeight', { configurable: true, get: () => height.value });
    let renders = 0;
    const anchorRef = { current: anchor };
    const layerRef = { current: layer };
    const hook = renderHook(() => {
      renders += 1;
      return useAnchoredStyle(anchorRef, layerRef, true, 'top', 'start');
    });
    return { anchor, layer, list, rect, hook, renders: () => renders };
  }

  it('is not recomputed on a scroll of the layer\'s own list, and is on a scroll of the page', () => {
    const { list, rect, hook } = mount({ value: 50 });
    expect(hook.result.current.placed).toBe(true);
    rect.mockClear();
    act(() => { list.dispatchEvent(new Event('scroll')); });
    expect(rect).not.toHaveBeenCalled();
    act(() => { document.dispatchEvent(new Event('scroll')); });
    expect(rect).toHaveBeenCalledTimes(1);
  });

  it('keeps its style object, and renders nothing, when a scroll moved nothing', () => {
    const { hook, renders } = mount({ value: 50 });
    const before = hook.result.current.style;
    const count = renders();
    act(() => {
      document.dispatchEvent(new Event('scroll'));
      window.dispatchEvent(new Event('resize'));
    });
    expect(hook.result.current.style).toBe(before);
    expect(renders()).toBe(count);
  });

  it('follows a size change of the layer, without a scroll or a resize', () => {
    vi.stubGlobal('ResizeObserver', FakeResizeObserver);
    const height = { value: 50 };
    const { layer, anchor, hook } = mount(height);
    expect(hook.result.current.style.top).toBe(100 - ANCHOR_GAP - 50);
    const observer = observers.at(-1)!;
    expect(observer.observed).toEqual([layer, anchor]);
    height.value = 80;
    act(() => { observer.callback([], observer as unknown as ResizeObserver); });
    expect(hook.result.current.style.top).toBe(100 - ANCHOR_GAP - 80);
    hook.unmount();
    expect(observer.observed).toEqual([]);
  });
});
