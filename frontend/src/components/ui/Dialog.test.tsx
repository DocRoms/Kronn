// KT-1881 — Dialog: the centred modal layer. The shared contract (role, name,
// focus, Escape, cross, scrim, axe) is in src/test/overlayContract.tsx.
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Dialog } from './Dialog';
import { describeOverlayContract } from '../../test/overlayContract';
import { BREAKPOINTS } from '../../styles/designTokens';
import { cssText, maxWidthBlock } from '../../test/cssBlocks';

const DIR = dirname(fileURLToPath(import.meta.url));
const css = (file: string) => cssText(join(DIR, file));

describeOverlayContract('Dialog', Dialog, 'ui-dialog');

describe('Dialog', () => {
  it('can be an alertdialog', () => {
    render(<Dialog open onClose={() => {}} title="Leave the page?" role="alertdialog">x</Dialog>);
    expect(screen.getByRole('alertdialog', { name: 'Leave the page?' })).toHaveAttribute('aria-modal', 'true');
  });

  it('sits on the modal layer, a nested one above it', () => {
    const { rerender } = render(<Dialog open onClose={() => {}} title="Base">x</Dialog>);
    expect(document.querySelector('.ui-overlay')).toHaveAttribute('data-layer', 'modal');
    rerender(<Dialog open onClose={() => {}} title="Base" layer="nested">x</Dialog>);
    expect(document.querySelector('.ui-overlay')).toHaveAttribute('data-layer', 'nested');
    const text = css('overlay.css');
    expect(text).toMatch(/\.ui-overlay\[data-layer="modal"\]\s*\{[^}]*z-index:\s*var\(--kr-z-modal\)/);
    expect(text).toMatch(/\.ui-overlay\[data-layer="nested"\]\s*\{[^}]*z-index:\s*var\(--kr-z-modal-nested\)/);
  });

  it('takes its three widths from the overlay width tokens', () => {
    const text = css('Dialog.css');
    for (const size of ['sm', 'md', 'lg']) {
      expect(text).toMatch(new RegExp(`\\.ui-dialog\\[data-size="${size}"\\]\\s*\\{[^}]*var\\(--kr-overlay-w-${size}\\)`));
    }
  });

  it('fills the screen under the first breakpoint', () => {
    const narrow = maxWidthBlock(css('Dialog.css'), BREAKPOINTS.sm - 1);
    expect(narrow).toMatch(/\.ui-dialog[^{]*\{[^}]*width:\s*100%/);
    expect(narrow).toMatch(/\.ui-dialog[^{]*\{[^}]*height:\s*100%/);
    expect(narrow).toMatch(/\.ui-dialog[^{]*\{[^}]*border-radius:\s*0/);
  });
});
