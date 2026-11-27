// KT-1881 — Drawer: the side panel, full height at the end edge. The shared
// contract (role, name, focus, Escape, cross, scrim, axe) is in
// src/test/overlayContract.tsx.
import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/react';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Drawer } from './Drawer';
import { describeOverlayContract } from '../../test/overlayContract';
import { BREAKPOINTS } from '../../styles/designTokens';
import { cssText, maxWidthBlock } from '../../test/cssBlocks';

const DIR = dirname(fileURLToPath(import.meta.url));
const css = (file: string) => cssText(join(DIR, file));

describeOverlayContract('Drawer', Drawer, 'ui-drawer');

describe('Drawer', () => {
  it('sits on the drawer layer, under a dialog', () => {
    render(<Drawer open onClose={() => {}} title="Notifications">x</Drawer>);
    expect(document.querySelector('.ui-overlay')).toHaveAttribute('data-layer', 'drawer');
    expect(css('overlay.css')).toMatch(/\.ui-overlay\[data-layer="drawer"\]\s*\{[^}]*z-index:\s*var\(--kr-z-drawer\)/);
  });

  it('runs the full height of the window at the end edge, in the three widths', () => {
    const text = css('Drawer.css');
    expect(text).toMatch(/\.ui-drawer\s*\{[^}]*height:\s*100%/);
    expect(text).toMatch(/\.ui-overlay\[data-kind="drawer"\]\s*\{[^}]*justify-content:\s*flex-end/);
    for (const size of ['sm', 'md', 'lg']) {
      expect(text).toMatch(new RegExp(`\\.ui-drawer\\[data-size="${size}"\\]\\s*\\{[^}]*var\\(--kr-overlay-w-${size}\\)`));
    }
  });

  it('fills the screen under the first breakpoint', () => {
    const narrow = maxWidthBlock(css('Drawer.css'), BREAKPOINTS.sm - 1);
    expect(narrow).toMatch(/\.ui-drawer[^{]*\{[^}]*width:\s*100%/);
    expect(narrow).toMatch(/\.ui-drawer[^{]*\{[^}]*border-radius:\s*0/);
  });
});
