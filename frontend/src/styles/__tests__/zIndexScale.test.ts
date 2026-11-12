// KT-1701 — one z-index scale. A hard-coded layer above 50 in a stylesheet or
// an inline style fails here, and the popover layer must outrank every
// non-system overlay: a popover is opened from inside the topmost layer.
import { describe, it, expect } from 'vitest';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { appSources, cssRules, declarations } from '../themeAudit';
import {
  LAYER_ORDER, STICKY_MAX, classZIndex, evalZIndex, hardCodedZIndex, loadBaseTokens,
} from '../zIndexAudit';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const TOKENS = loadBaseTokens(SRC);
const SOURCES = appSources(SRC);

const layer = (name: (typeof LAYER_ORDER)[number]) => evalZIndex(`var(${name})`, TOKENS);

describe('z-index scale', () => {
  it('defines every layer, strictly ascending', () => {
    const values = LAYER_ORDER.map(layer);
    expect(values.every(v => typeof v === 'number')).toBe(true);
    expect(values[0]).toBe(STICKY_MAX);
    values.slice(1).forEach((v, i) => expect(v!, LAYER_ORDER[i + 1]).toBeGreaterThan(values[i]!));
  });

  it('puts popovers above every modal, toasts above popovers and the tour above toasts', () => {
    expect(layer('--kr-z-popover')!).toBeGreaterThan(layer('--kr-z-modal-nested')!);
    expect(layer('--kr-z-toast')!).toBeGreaterThan(layer('--kr-z-popover')!);
    expect(layer('--kr-z-tour')!).toBeGreaterThan(layer('--kr-z-toast')!);
  });

  it('keeps no hard-coded z-index above 50 in frontend/src', () => {
    const offenders = hardCodedZIndex(SOURCES);
    expect(offenders, `Raw z-index above ${STICKY_MAX} — use a --kr-z-* layer from tokens.css:\n`
      + offenders.map(o => `  src/${o.file}:${o.line}  ${o.detail}`).join('\n')).toEqual([]);
  });

  it('flags a raw layer in CSS and in inline styles, and accepts tokens and small local values', () => {
    const fixture = [
      { file: 'a.css', text: '.a { z-index: 1000; }\n.b { z-index: 30; }\n.c { z-index: calc(var(--kr-z-modal) + 20); }' },
      { file: 'b.tsx', text: "const s = { zIndex: 9999, top: 0 }; const t = { zIndex: 'var(--kr-z-toast)' }; const u = { zIndex: 4 };" },
      { file: 'c.css', text: '.d { z-index: calc(var(--kr-z-sticky) + 60); }\n.e { z-index: calc(var(--kr-z-sticky) + 10); }' },
    ];
    expect(hardCodedZIndex(fixture).map(f => `${f.file}:${f.line}`)).toEqual(['a.css:1', 'b.tsx:1', 'c.css:1']);
  });

  it('resolves tokens and calc offsets', () => {
    expect(evalZIndex('calc(var(--kr-z-popover) + 1)', TOKENS)).toBe(layer('--kr-z-popover')! + 1);
    expect(evalZIndex('var(--kr-z-modal) !important', TOKENS)).toBe(layer('--kr-z-modal'));
    expect(evalZIndex('auto', TOKENS)).toBeNull();
  });

  it('keeps every other overlay below the popover layer', () => {
    const popover = layer('--kr-z-popover')!;
    const systemLayers = /--kr-z-(popover|toast|tour)/;
    const above: string[] = [];
    for (const source of SOURCES.filter(s => s.file.endsWith('.css'))) {
      for (const rule of cssRules(source.text)) {
        for (const [prop, value] of declarations(rule.body)) {
          if (prop !== 'z-index' || systemLayers.test(value)) continue;
          const z = evalZIndex(value, TOKENS);
          if (z !== null && z >= popover) above.push(`src/${source.file}:${rule.line} ${rule.selector} → ${z}`);
        }
      }
    }
    expect(above, 'A non-popover layer outranks the popover layer — popovers would open under it').toEqual([]);
  });

  it('reads a class z-index from the stylesheets', () => {
    expect(classZIndex('kr-agent-switch-popover', SRC)).toBe(layer('--kr-z-popover'));
    expect(classZIndex('dash-modal-overlay', SRC)).toBe(layer('--kr-z-modal'));
    expect(classZIndex('no-such-class-anywhere', SRC)).toBeNull();
  });
});
