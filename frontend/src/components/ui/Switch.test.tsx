// KT-1882 — Switch: one on/off control, a checkbox with the switch role,
// always named by its visible label.
import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Switch } from './Switch';
import { describeViolations, runAxe } from '../../test/axe';
import { contrastRatio, loadThemes, parseColor, resolveValue, SURFACE_TOKENS, themeColor } from '../../styles/themeAudit';

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(join(HERE, 'Switch.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const THEMES = loadThemes(join(HERE, '..', '..'));
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];

function Harness({ onValue, disabled }: { onValue?: (v: boolean) => void; disabled?: boolean }) {
  const [on, setOn] = useState(false);
  return (
    <Switch
      label="Notify me"
      description="A desktop notification when a run ends."
      checked={on}
      disabled={disabled}
      onChange={v => { setOn(v); onValue?.(v); }}
    />
  );
}

/** The value a declaration of `selector` sets for `prop`, in Switch.css. */
function declared(selector: string, prop: string): string {
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const body = new RegExp(`(?:^|\\}|,)\\s*${esc}\\s*(?:,[^{]*)?\\{([^}]*)\\}`).exec(CSS)?.[1] ?? '';
  return new RegExp(`(?:^|;)\\s*${prop}:\\s*([^;]+)`).exec(body)?.[1].trim() ?? '';
}

describe('Switch', () => {
  it('is a switch named by its visible label and described by its description', () => {
    render(<Harness />);
    const sw = screen.getByRole('switch', { name: 'Notify me' });
    expect(sw).toHaveAttribute('type', 'checkbox');
    expect(sw).not.toBeChecked();
    expect(sw).toHaveAccessibleDescription('A desktop notification when a run ends.');
  });

  it('turns on and off with a click on the switch or on its label, and with Space', async () => {
    const onValue = vi.fn();
    const user = userEvent.setup();
    render(<Harness onValue={onValue} />);
    const sw = screen.getByRole('switch', { name: 'Notify me' });
    await user.click(sw);
    expect(sw).toBeChecked();
    await user.click(screen.getByText('Notify me'));
    expect(sw).not.toBeChecked();
    sw.focus();
    await user.keyboard(' ');
    expect(sw).toBeChecked();
    expect(onValue.mock.calls.map(c => c[0])).toEqual([true, false, true]);
  });

  it('does not change while disabled', async () => {
    const onValue = vi.fn();
    const user = userEvent.setup();
    render(<Harness onValue={onValue} disabled />);
    const sw = screen.getByRole('switch', { name: 'Notify me' });
    expect(sw).toBeDisabled();
    await user.click(screen.getByText('Notify me'));
    expect(onValue).not.toHaveBeenCalled();
    expect(sw.closest('.ui-switch')).toHaveAttribute('data-disabled', 'true');
  });

  it('follows the value it is given (controlled)', () => {
    const onChange = vi.fn();
    const { rerender } = render(<Switch label="Auto-merge" checked onChange={onChange} />);
    expect(screen.getByRole('switch')).toBeChecked();
    fireEvent.click(screen.getByRole('switch'));
    expect(onChange).toHaveBeenCalledWith(false);
    rerender(<Switch label="Auto-merge" checked={false} onChange={onChange} />);
    expect(screen.getByRole('switch')).not.toBeChecked();
  });

  it('can hide its label and stay named by it', () => {
    render(<Switch label="Enable the hook" hideLabel checked={false} onChange={() => {}} />);
    expect(screen.getByRole('switch', { name: 'Enable the hook' })).toBeInTheDocument();
    expect(screen.getByText('Enable the hook')).toHaveClass('ui-visually-hidden');
  });

  it('takes its own id and a ref', () => {
    let node: HTMLInputElement | null = null;
    render(<Switch id="notify" label="Notify" checked={false} onChange={() => {}} ref={el => { node = el; }} />);
    expect(screen.getByRole('switch')).toHaveAttribute('id', 'notify');
    expect(node).toBe(screen.getByRole('switch'));
  });

  it('passes axe on and off, enabled and disabled', async () => {
    render(
      <main>
        <Switch label="Off" checked={false} onChange={() => {}} />
        <Switch label="On" checked onChange={() => {}} />
        <Switch label="With description" description="More words." checked onChange={() => {}} />
        <Switch label="Disabled" disabled checked={false} onChange={() => {}} />
      </main>,
    );
    const results = await act(() => runAxe(document.body));
    expect(describeViolations(results)).toEqual([]);
  });

  it('draws its state at 3:1 against every surface, in every theme (WCAG 1.4.11)', () => {
    const misses: string[] = [];
    const off = declared('.ui-switch-thumb', 'background-color');
    const offBorder = declared('.ui-switch-track', 'border-color');
    const onFill = declared('.ui-switch-input:checked + .ui-switch-track', 'background-color');
    const onThumb = declared('.ui-switch-input:checked + .ui-switch-track .ui-switch-thumb', 'background-color');
    for (const value of [off, offBorder, onFill, onThumb]) expect(value).toMatch(/^var\(--kr-[a-z-]+\)$/);
    for (const theme of THEME_NAMES) {
      const tokens = THEMES.get(theme)!;
      const colour = (v: string) => parseColor(resolveValue(v, tokens))!;
      for (const surface of SURFACE_TOKENS) {
        const bg = themeColor(tokens, surface);
        if (!bg || bg.a < 1) continue;
        for (const [what, value] of [['off thumb', off], ['off track', offBorder], ['on track', onFill]] as const) {
          const ratio = contrastRatio(colour(value), bg);
          if (ratio < 3) misses.push(`${theme} ${what} on ${surface}: ${ratio.toFixed(2)}`);
        }
      }
      const thumbOnFill = contrastRatio(colour(onThumb), colour(onFill));
      if (thumbOnFill < 3) misses.push(`${theme} on thumb on its track: ${thumbOnFill.toFixed(2)}`);
    }
    expect(misses).toEqual([]);
  });
});
