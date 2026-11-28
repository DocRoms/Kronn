// KT-1883 — where the toast stack sits (owner decision of 2026-11-21, its
// breakpoint corrected after review). From --kr-bp-md (768 px, where the
// discussions list column appears) up: bottom left, inside the list column, a
// toast at most 320 px wide, so it covers neither the bell (top right) nor the
// composer's Send, Add a file and Quick Prompt (bottom, right of the list).
// Below it, where a single column leaves every corner over something: at the
// top, just under the app's top bar, full width less the 16 px margins. In
// both forms the stack is never taller than the window and scrolls.
// jsdom evaluates no media query, so the stylesheet is read here.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BREAKPOINTS } from '../../styles/designTokens';

const CSS = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'toastStack.css'), 'utf8')
  .replace(/\/\*[\s\S]*?\*\//g, '');

/** The narrow query: the md breakpoint minus one, as every stylesheet writes it. */
const NARROW = new RegExp(`@media\\s*\\(max-width:\\s*${BREAKPOINTS.md - 1}px\\)\\s*\\{`);

/** Wide rules (outside any @media) and narrow rules (inside the md query). */
function split(): { wide: string; narrow: string } {
  const m = NARROW.exec(CSS);
  if (!m) return { wide: CSS, narrow: '' };
  let depth = 1;
  let i = m.index + m[0].length;
  while (depth > 0 && i < CSS.length) {
    if (CSS[i] === '{') depth++;
    else if (CSS[i] === '}') depth--;
    i++;
  }
  const narrow = CSS.slice(m.index + m[0].length, i - 1);
  const rest = CSS.slice(0, m.index) + CSS.slice(i);
  // Other @media blocks (reduced motion) are not placement.
  const wide = rest.replace(/@media[^{]*\{(?:[^{}]*\{[^{}]*\})*[^{}]*\}/g, '');
  return { wide, narrow };
}

/** The declarations of `selector` in `block`, as a property → value map. */
function decls(block: string, selector: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [, sel, body] of block.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (!sel.split(',').map(s => s.trim()).includes(selector)) continue;
    for (const d of body.split(';')) {
      const at = d.indexOf(':');
      if (at > 0) out[d.slice(0, at).trim()] = d.slice(at + 1).trim();
    }
  }
  return out;
}

describe('toast stack placement', () => {
  const { wide, narrow } = split();

  it('has its narrow form below the md breakpoint, where the list column goes away', () => {
    expect(BREAKPOINTS.md).toBe(768);
    expect(narrow, 'no @media (max-width: 767px) block').not.toBe('');
    expect(CSS, 'the sm breakpoint no longer places the stack').not.toMatch(new RegExp(`max-width:\\s*${BREAKPOINTS.sm - 1}px`));
  });

  it('from 768 px up: fixed at the bottom left, 16 px in, growing upwards, newest at the bottom', () => {
    const stack = decls(wide, '.kr-toast-stack');
    expect(stack.position).toBe('fixed');
    expect(stack.left).toBe('var(--kr-space-4)');
    expect(stack.bottom).toBe('var(--kr-space-4)');
    expect(stack.top).toBeUndefined();
    expect(stack.right).toBeUndefined();
    expect(stack['flex-direction']).toBe('column');
    expect(stack['z-index']).toBe('var(--kr-z-toast)');
  });

  it('from 768 px up: a toast is at most 320 px wide, inside the list column', () => {
    const toast = decls(wide, '.kr-toast-stack .kr-toast');
    expect(toast['max-width']).toBe('320px');
  });

  it('from 768 px up: never taller than the window less its two margins, and scrolls', () => {
    const stack = decls(wide, '.kr-toast-stack');
    expect(stack['max-height']).toBe('calc(100dvh - 2 * var(--kr-space-4))');
    expect(stack['overflow-y']).toBe('auto');
    expect(stack['overflow-x']).toBe('hidden');
    // A capped column would otherwise squeeze its toasts instead of scrolling.
    expect(decls(wide, '.kr-toast-stack .kr-toast')['flex-shrink']).toBe('0');
  });

  it('lets clicks through the stack, but not through its toasts', () => {
    expect(decls(wide, '.kr-toast-stack')['pointer-events']).toBe('none');
    expect(decls(wide, '.kr-toast-stack .kr-toast')['pointer-events']).toBe('auto');
  });

  it('below 768 px: at the top, just under the app bar, full width less the 16 px margins', () => {
    const stack = decls(narrow, '.kr-toast-stack');
    expect(stack.top).toBe('calc(var(--app-bar-h, 0px) + var(--kr-space-2))');
    expect(stack.bottom).toBe('auto');
    expect(stack.left).toBe('var(--kr-space-4)');
    expect(stack.right).toBe('var(--kr-space-4)');
    expect(stack['align-items']).toBe('stretch');
    const toast = decls(narrow, '.kr-toast-stack .kr-toast');
    expect(toast['max-width']).toBe('none');
    expect(toast['min-width']).toBe('0');
  });

  it('below 768 px: never taller than the window less the app bar and the margins', () => {
    const stack = decls(narrow, '.kr-toast-stack');
    expect(stack['max-height']).toBe('calc(100dvh - var(--app-bar-h, 0px) - var(--kr-space-2) - var(--kr-space-4))');
  });
});
