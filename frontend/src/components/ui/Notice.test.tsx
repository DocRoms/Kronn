// KT-1883 — Notice: four tones from the --kr-status-* colours, the text in a
// text colour that holds 4.5:1 on its tint in every theme, an alert only for
// the danger tone, one action, a dismiss cross with a name.
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Notice } from './Notice';
import { NOTICE_TONES } from './feedbackKinds';
import { describeViolations, runAxe } from '../../test/axe';
import {
  composite, contrastRatio, cssRules, declarations, loadThemes, parseColor, resolveValue, SURFACE_TOKENS, themeColor,
} from '../../styles/themeAudit';

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(join(HERE, 'Notice.css'), 'utf8');
const THEMES = loadThemes(join(HERE, '..', '..'));
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];

/** The declarations `selector` gets from Notice.css for `tone`: the base rule,
 *  then the tone rule (the info tone is the base). */
function noticeDeclarations(tone: string, selector = '.ui-notice'): Map<string, string> {
  const out = new Map<string, string>();
  for (const sel of ['.ui-notice', `.ui-notice[data-tone="${tone}"]`, ...(selector === '.ui-notice' ? [] : [selector])]) {
    for (const rule of cssRules(CSS).filter(r => r.selector === sel)) {
      for (const [prop, value] of declarations(rule.body)) out.set(prop, value);
    }
  }
  return out;
}

describe('Notice', () => {
  it('has four tones, each from its --kr-status-* colour', () => {
    expect(NOTICE_TONES).toEqual(['info', 'warning', 'danger', 'success']);
    for (const tone of NOTICE_TONES) expect(CSS, tone).toContain(`var(--kr-status-${tone})`);
  });

  it('writes its text in a text colour, never the tone colour', () => {
    for (const selector of ['.ui-notice', '.ui-notice-title', '.ui-notice-message']) {
      const color = cssRules(CSS).filter(r => r.selector === selector)
        .flatMap(r => declarations(r.body)).find(([prop]) => prop === 'color')?.[1];
      expect(color, selector).toBe('var(--kr-text-primary)');
    }
  });

  it('reads its text at 4.5:1 on its own tint, for every tone, theme and surface', () => {
    const misses: string[] = [];
    for (const theme of THEME_NAMES) {
      const themeTokens = THEMES.get(theme)!;
      for (const tone of NOTICE_TONES) {
        const notice = noticeDeclarations(tone);
        const tokens = new Map([...themeTokens, ...notice]);
        const text = parseColor(resolveValue(notice.get('color')!, tokens));
        const fill = parseColor(resolveValue(notice.get('background-color')!, tokens));
        expect(text, `${theme} ${tone} text`).not.toBeNull();
        expect(fill, `${theme} ${tone} fill`).not.toBeNull();
        for (const surface of SURFACE_TOKENS) {
          const ratio = contrastRatio(text!, composite(fill!, themeColor(themeTokens, surface)!));
          if (ratio < 4.5) misses.push(`${theme} ${tone} on ${surface}: ${ratio.toFixed(2)}`);
        }
      }
    }
    expect(misses).toEqual([]);
  });

  it.each(NOTICE_TONES)('shows the %s tone with its message in words and an icon hidden from assistive technology', tone => {
    const { container } = render(<Notice tone={tone} title="Heads up">Body {tone}</Notice>);
    const notice = container.firstElementChild!;
    expect(notice).toHaveAttribute('data-tone', tone);
    expect(notice).toHaveTextContent('Heads up');
    expect(notice).toHaveTextContent(`Body ${tone}`);
    expect(notice.querySelector('.ui-notice-icon')).toHaveAttribute('aria-hidden', 'true');
  });

  it('is an alert for the danger tone only, a polite status otherwise', () => {
    render(<>
      <Notice tone="danger">Could not save</Notice>
      <Notice tone="info">Read only</Notice>
      <Notice tone="warning">Almost full</Notice>
      <Notice tone="success">Published</Notice>
    </>);
    expect(screen.getByRole('alert')).toHaveTextContent('Could not save');
    expect(screen.getAllByRole('status').map(s => s.getAttribute('data-tone'))).toEqual(['info', 'warning', 'success']);
  });

  it('runs its one action and has no cross unless dismissible', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(<Notice tone="danger" action={{ label: 'Retry', onClick }}>Could not load the tasks</Notice>);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('is dismissed by a cross named by its label', async () => {
    const user = userEvent.setup();
    const onDismiss = vi.fn();
    render(<Notice tone="info" onDismiss={onDismiss}>Tip</Notice>);
    await user.click(screen.getByRole('button', { name: 'ui.notice.dismiss' }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it('passes axe in each tone, with an action and a cross', async () => {
    const { container } = render(<main>
      {NOTICE_TONES.map(tone => (
        <Notice key={tone} tone={tone} title={`Title ${tone}`} action={{ label: `Act ${tone}`, onClick: () => {} }} onDismiss={() => {}}>
          Message {tone}
        </Notice>
      ))}
    </main>);
    const results = await act(() => runAxe(container));
    expect(describeViolations(results)).toEqual([]);
  });
});
