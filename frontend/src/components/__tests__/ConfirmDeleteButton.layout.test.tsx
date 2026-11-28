/**
 * KT-1996 — the armed delete control on a workflow card and in the skill sheet.
 *
 * Armed, the button showed "Confirmer" (48 px) in a 26 px square and the word
 * ran 10 px over "Lancer"; the impact sentence beside it was `nowrap`, so a
 * long one was cut by the card or pushed it. happy-dom has no layout engine
 * (scrollWidth and clientWidth are 0), so this test renders the real component
 * with the longest locale label inside the real surfaces, loads their real
 * stylesheets, and reads the cascade happy-dom resolves: specificity, `var()`,
 * `:has()` and `@media` at the viewport width the test sets.
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ConfirmDeleteButton } from '../ConfirmDeleteButton';
import en from '../../lib/i18n/locales/en';
import es from '../../lib/i18n/locales/es';
import fr from '../../lib/i18n/locales/fr';
import zh from '../../lib/i18n/locales/zh';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
// The component's own sheet goes FIRST: its armed rule must win on
// specificity, whatever order the bundle puts the callers' sheets in.
const SHEETS = [
  'styles/tokens.css',
  'components/ConfirmDeleteButton.css',
  'pages/WorkflowsPage.css',
  'components/SkillSheet.css',
];

const LOCALES = { en, es, fr, zh } as Record<string, Record<string, string>>;
const labels = Object.values(LOCALES).map(dict => dict['automation.deleteConfirmAction']);
const LONGEST = [...labels].sort((a, b) => b.length - a.length)[0];
const LONG_IMPACT = fr['wf.deleteImpact'].replace('{0}', '1 248');

type HappyWindow = { happyDOM?: { setViewport(v: { width: number; height: number }): void } };
const setViewport = (width: number) => {
  (window as unknown as HappyWindow).happyDOM?.setViewport({ width, height: 900 });
};

beforeAll(() => {
  const style = document.createElement('style');
  style.textContent = SHEETS.map(path => readFileSync(join(SRC, path), 'utf8')).join('\n');
  document.head.appendChild(style);
});

afterEach(() => {
  cleanup();
  setViewport(1024);
});

const px = (value: string) => Number.parseFloat(value);

function renderOnCard(impact?: string, confirmLabel = LONGEST) {
  // The card footer as WorkflowsPage renders it: run summary, then the actions
  // with "Lancer" followed by the delete control.
  render(
    <div className="wf-card">
      <div className="wf-card-footer">
        <div className="wf-card-run-summary"><span className="wf-card-run-date">21/11/2026 18:26</span></div>
        <div className="wf-card-actions">
          <button type="button" className="wf-card-run-btn">Lancer</button>
          <ConfirmDeleteButton
            className="wf-card-delete-btn"
            label="Supprimer"
            confirmLabel={confirmLabel}
            itemName="Veille"
            testId="del"
            impact={impact === undefined ? undefined : () => impact}
            onConfirm={vi.fn().mockResolvedValue(undefined)}
          />
        </div>
      </div>
    </div>,
  );
  return screen.getByTestId('del');
}

function renderInSkillSheet(impact?: string, confirmLabel = LONGEST) {
  render(
    <footer className="skill-sheet-actions">
      <div className="skill-sheet-buttons">
        <button type="button" className="skill-sheet-action">Ouvrir les réglages</button>
        <ConfirmDeleteButton
          className="wf-icon-btn qp-card-tool-btn"
          label="Supprimer"
          confirmLabel={confirmLabel}
          itemName="rust-expert"
          testId="del"
          impact={impact === undefined ? undefined : () => impact}
          onConfirm={vi.fn().mockResolvedValue(undefined)}
        />
      </div>
    </footer>,
  );
  return screen.getByTestId('del');
}

const SURFACES = [
  { name: 'workflow card', render: renderOnCard, restWidth: '26px' },
  { name: 'skill sheet', render: renderInSkillSheet, restWidth: '28px' },
] as const;

describe('KT-1996 — the armed delete control takes the width of its word', () => {
  it('reads the label of each of the four locales', () => {
    expect(labels).toHaveLength(4);
    expect(labels.every(Boolean)).toBe(true);
    // "Confirmer" (fr) and "Confirmar" (es) tie for the longest.
    expect(LONGEST.length).toBe(Math.max(...labels.map(l => l.length)));
  });

  for (const surface of SURFACES) {
    describe(surface.name, () => {
      it('stays the icon square at rest', () => {
        const button = surface.render();
        expect(getComputedStyle(button).width).toBe(surface.restWidth);
      });

      it.each(Object.entries(LOCALES))('grows to the %s label once armed, at least 24 px high, and never shrinks under it', (_locale, dict) => {
        const label = dict['automation.deleteConfirmAction'];
        const button = surface.render(undefined, label);
        fireEvent.click(button);
        expect(button).toHaveAttribute('data-armed', 'true');
        expect(button).toHaveAccessibleName(`${label} · ${surface.name === 'skill sheet' ? 'rust-expert' : 'Veille'}`);
        const word = button.querySelector('.kr-confirm-delete-word');
        expect(word).toHaveTextContent(label);
        expect(getComputedStyle(word!).whiteSpace).toBe('nowrap');

        const cs = getComputedStyle(button);
        // The caller's fixed square is overridden: the box is the word's width
        // plus padding, so nothing of the label runs outside it.
        expect(cs.width).toBe('auto');
        expect(cs.minWidth).toBe('max-content');
        expect(cs.overflow === '' || cs.overflow === 'visible').toBe(true);
        expect(px(cs.paddingLeft)).toBeGreaterThan(0);
        expect(px(cs.paddingRight)).toBeGreaterThan(0);
        // A full row squeezes its neighbour or wraps; it does not squeeze the word.
        expect(cs.flexShrink).toBe('0');
        expect(px(cs.minHeight)).toBeGreaterThanOrEqual(24);
        expect(px(cs.height) || px(cs.minHeight)).toBeGreaterThanOrEqual(24);
      });

      it('sits beside the previous action in a flex row with a gap, never over it', () => {
        const button = surface.render();
        fireEvent.click(button);
        const control = button.parentElement!;
        const row = control.parentElement!;
        expect(getComputedStyle(row).display).toBe('flex');
        expect(px(getComputedStyle(row).columnGap || getComputedStyle(row).gap)).toBeGreaterThan(0);
        for (const el of [button, control]) {
          const cs = getComputedStyle(el);
          expect(['', 'static', 'relative']).toContain(cs.position);
          expect(px(cs.marginLeft) || 0).toBeGreaterThanOrEqual(0);
        }
      });
    });
  }
});

describe('KT-1996 — the impact sentence wraps inside the card', () => {
  for (const width of [1440, 400]) {
    for (const surface of SURFACES) {
      it(`${surface.name} at ${width} px`, () => {
        setViewport(width);
        const button = surface.render(LONG_IMPACT);
        fireEvent.click(button);
        const impact = screen.getByTestId('del-impact');
        expect(impact).toHaveTextContent(LONG_IMPACT);

        const cs = getComputedStyle(impact);
        expect(cs.whiteSpace === '' || cs.whiteSpace === 'normal').toBe(true);
        expect(cs.minWidth).toBe('0');
        // The control itself may not outgrow its row, and wraps its two parts.
        const control = getComputedStyle(impact.parentElement!);
        expect(control.flexWrap).toBe('wrap');
        expect(control.maxWidth).toBe('100%');
        expect(control.minWidth).toBe('0');

        // Every flex row between the control and the card wraps and may shrink:
        // the sentence gets the width of the card, not the width of its text.
        const row = impact.parentElement!.parentElement!;
        expect(getComputedStyle(row).flexWrap).toBe('wrap');
        expect(getComputedStyle(row).minWidth).toBe('0');
        if (surface.name === 'workflow card') {
          // The footer may wrap, so the actions go under the run summary only
          // when they do not fit beside it.
          expect(getComputedStyle(row.parentElement!).flexWrap).toBe('wrap');
        }
      });
    }
  }

  for (const width of [1440, 400]) {
    it(`workflow card at ${width} px: the armed actions keep their natural width, no forced row of their own`, () => {
      // Review of KT-1996: `flex: 1 1 100%` sent the actions under the summary
      // at every width, so at 1440 px the card grew 127 -> 150 px although the
      // sentence fitted on the footer row. They keep their natural basis
      // (on the summary's row while they fit) and may shrink once alone.
      setViewport(width);
      const button = renderOnCard(LONG_IMPACT);
      fireEvent.click(button);
      const actions = getComputedStyle(document.querySelector('.wf-card-actions')!);
      expect(actions.flexBasis).not.toBe('100%');
      expect(['', 'auto']).toContain(actions.flexBasis);
      expect(actions.flexGrow).toBe('0');
      expect(actions.flexShrink).toBe('1');
    });
  }

  it('workflow card: "Lancer" and the armed button share a row, the sentence wraps as its own item', () => {
    // At 400 px the control's own box went to a row of its own under "Lancer",
    // then broke into "Confirmer" and the sentence: three rows. Its box is
    // dissolved into the actions, so the button stays beside "Lancer" (69 +
    // 66 px of 354) and only the sentence, too wide for that row, wraps below.
    setViewport(400);
    const button = renderOnCard(LONG_IMPACT);
    fireEvent.click(button);
    const control = button.parentElement!;
    expect(control).toHaveClass('kr-confirm-delete');
    expect(getComputedStyle(control).display).toBe('contents');
    const impact = screen.getByTestId('del-impact');
    expect(impact.parentElement).toBe(control);
    // Both are flex items of the wrapping actions row, after "Lancer".
    const actions = control.parentElement!;
    expect(actions).toHaveClass('wf-card-actions');
    expect(actions.firstElementChild).toHaveClass('wf-card-run-btn');
    expect(getComputedStyle(actions).flexWrap).toBe('wrap');
    expect(getComputedStyle(button).flexShrink).toBe('0');
    expect(getComputedStyle(impact).flexGrow).toBe('1');
    expect(getComputedStyle(impact).minWidth).toBe('0');
  });

  it('skill sheet keeps the control as one box (no dissolve outside the workflow card)', () => {
    const button = renderInSkillSheet(LONG_IMPACT);
    fireEvent.click(button);
    expect(getComputedStyle(button.parentElement!).display).toBe('inline-flex');
  });

  it('leaves the card footer as it was while nothing is armed', () => {
    renderOnCard(LONG_IMPACT);
    const actions = document.querySelector('.wf-card-actions')!;
    expect(getComputedStyle(actions).flexShrink).toBe('0');
    expect(getComputedStyle(actions.parentElement!).flexWrap).not.toBe('wrap');
  });

  it('evaluates @media at the width the test sets (so 400 px is really read)', () => {
    // SkillSheet.css raises its actions to 44 px under 640 px.
    setViewport(1440);
    render(<button type="button" className="skill-sheet-action">x</button>);
    expect(getComputedStyle(screen.getByText('x')).minHeight).not.toBe('44px');
    cleanup();
    setViewport(400);
    render(<button type="button" className="skill-sheet-action">x</button>);
    expect(getComputedStyle(screen.getByText('x')).minHeight).toBe('44px');
  });
});
