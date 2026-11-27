// KT-1886 — ProgressBar: a named progressbar, determinate (value, max, a
// reading in valueText) or indeterminate (no value: animated, still under
// prefers-reduced-motion); five tones, two heights; a stacked form, an image
// whose name lists every segment and its share, with an optional legend.
// Written before the component.
import { describe, expect, it } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ProgressBar } from './ProgressBar';
import { PROGRESS_TONES } from './dataKinds';
import { cssText } from '../../test/cssBlocks';
import { describeViolations, runAxe } from '../../test/axe';

const CSS = cssText(join(dirname(fileURLToPath(import.meta.url)), 'ProgressBar.css'));
/** The share as the component writes it (the test context's locale is fr). */
const pct = (n: number) => new Intl.NumberFormat('fr', { style: 'percent', maximumFractionDigits: 0 }).format(n);

describe('ProgressBar — determinate', () => {
  it('is a progressbar named by its visible label, with its value, its bounds and its reading', () => {
    render(<ProgressBar label="Steps done" value={3} max={8} valueText="3/8" />);
    const bar = screen.getByRole('progressbar', { name: 'Steps done' });
    expect(bar).toHaveAttribute('aria-valuenow', '3');
    expect(bar).toHaveAttribute('aria-valuemin', '0');
    expect(bar).toHaveAttribute('aria-valuemax', '8');
    expect(bar).toHaveAttribute('aria-valuetext', '3/8');
    // The reading is written next to the label too.
    expect(screen.getByText('3/8')).toHaveClass('ui-progress-reading');
    expect(screen.getByText('Steps done')).not.toHaveClass('ui-visually-hidden');
    // The fill's width is the share.
    const fill = bar.querySelector('.ui-progress-fill') as HTMLElement;
    expect(fill.style.getPropertyValue('--ui-progress-value')).toBe('37.5%');
  });

  it('defaults max to 100 and clamps the value inside its bounds', () => {
    const { rerender } = render(<ProgressBar label="Upload" value={45} />);
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuemax', '100');
    expect(screen.getByRole('progressbar')).not.toHaveAttribute('aria-valuetext');
    rerender(<ProgressBar label="Upload" value={140} />);
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100');
    rerender(<ProgressBar label="Upload" value={-3} />);
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '0');
  });

  it('keeps a hidden label for assistive technology', () => {
    render(<ProgressBar label="Disk used" hideLabel value={70} valueText="70 %" />);
    expect(screen.getByRole('progressbar', { name: 'Disk used' })).toBeInTheDocument();
    expect(screen.getByText('Disk used')).toHaveClass('ui-visually-hidden');
  });

  it('takes five tones and two heights, from the status and accent tokens', () => {
    expect(PROGRESS_TONES).toEqual(['accent', 'success', 'warning', 'danger', 'info']);
    const { container, rerender } = render(<ProgressBar label="x" value={1} />);
    const root = () => container.querySelector('.ui-progress')!;
    expect(root()).toHaveAttribute('data-tone', 'accent');
    expect(root()).toHaveAttribute('data-size', 'md');
    rerender(<ProgressBar label="x" value={1} tone="danger" size="sm" />);
    expect(root()).toHaveAttribute('data-tone', 'danger');
    expect(root()).toHaveAttribute('data-size', 'sm');
    for (const tone of ['success', 'warning', 'danger', 'info']) {
      expect(CSS, tone).toMatch(new RegExp(`\\[data-tone="${tone}"\\][^{]*\\{[^}]*var\\(--kr-status-${tone}\\)`));
    }
    expect(CSS).toMatch(/\[data-size="sm"\][^{]*\{[^}]*height:/);
  });
});

describe('ProgressBar — indeterminate', () => {
  it('has no value when none is given, and animates, still under reduced motion', () => {
    const { container } = render(<ProgressBar label="Cloning the repository" />);
    const bar = screen.getByRole('progressbar', { name: 'Cloning the repository' });
    expect(bar).not.toHaveAttribute('aria-valuenow');
    expect(bar).not.toHaveAttribute('aria-valuetext');
    expect(container.querySelector('.ui-progress')).toHaveAttribute('data-indeterminate', 'true');
    expect(CSS).toMatch(/\[data-indeterminate="true"\] \.ui-progress-fill\s*\{[^}]*animation:/);
    const reduced = CSS.slice(CSS.indexOf('@media (prefers-reduced-motion: reduce)'));
    expect(reduced).toMatch(/\[data-indeterminate="true"\] \.ui-progress-fill\s*\{[^}]*animation:\s*none/);
  });
});

describe('ProgressBar — stacked', () => {
  const SEGMENTS = [
    { id: 'done', label: 'Done', value: 6, tone: 'success' as const },
    { id: 'failed', label: 'Failed', value: 1, tone: 'danger' as const },
    { id: 'running', label: 'Running', value: 3, tone: 'info' as const },
    { id: 'skipped', label: 'Skipped', value: 0, tone: 'warning' as const },
  ];

  it('is an image whose name lists every segment, its count and its share', () => {
    render(<ProgressBar label="Tasks" segments={SEGMENTS} />);
    const img = screen.getByRole('img');
    expect(screen.queryByRole('progressbar')).toBeNull();
    expect(img).toHaveAccessibleName(
      `Tasks: Done 6 (${pct(0.6)}), Failed 1 (${pct(0.1)}), Running 3 (${pct(0.3)}), Skipped 0 (${pct(0)})`,
    );
  });

  it('draws one segment per non-empty part, in its tone, sized by its share, a small one kept visible', () => {
    const { container } = render(<ProgressBar label="Tasks" segments={SEGMENTS} />);
    const parts = [...container.querySelectorAll<HTMLElement>('.ui-progress-segment')];
    expect(parts.map(p => p.getAttribute('data-tone'))).toEqual(['success', 'danger', 'info']);
    expect(parts.map(p => p.style.getPropertyValue('--ui-progress-value'))).toEqual(['60%', '10%', '30%']);
    expect(CSS).toMatch(/\.ui-progress-segment\s*\{[^}]*min-width:\s*var\(--kr-space-1\)/);
  });

  it('measures the shares against max when given, leaving the rest of the track empty', () => {
    render(<ProgressBar label="Quota" max={20} segments={SEGMENTS.slice(0, 1)} />);
    expect(screen.getByRole('img')).toHaveAccessibleName(`Quota: Done 6 (${pct(0.3)})`);
  });

  it('writes a share under 1 % as "<1 %", not 0', () => {
    render(<ProgressBar label="Tasks" segments={[{ id: 'a', label: 'A', value: 1, tone: 'info' }, { id: 'b', label: 'B', value: 999, tone: 'success' }]} />);
    expect(screen.getByRole('img')).toHaveAccessibleName(`Tasks: A 1 (<${pct(0.01)}), B 999 (${pct(0.999)})`);
  });

  it('has an optional legend with the counts, its swatches hidden from assistive technology', () => {
    const { container, rerender } = render(<ProgressBar label="Tasks" segments={SEGMENTS} />);
    expect(container.querySelector('.ui-progress-legend')).toBeNull();
    rerender(<ProgressBar label="Tasks" segments={SEGMENTS} legend />);
    const legend = container.querySelector('.ui-progress-legend')! as HTMLElement;
    expect(legend.tagName).toBe('UL');
    const items = within(legend).getAllByRole('listitem');
    expect(items.map(li => li.textContent)).toEqual(['Done6', 'Failed1', 'Running3', 'Skipped0']);
    for (const swatch of legend.querySelectorAll('.ui-progress-swatch')) expect(swatch).toHaveAttribute('aria-hidden', 'true');
  });

  it('with a legend, reads the breakdown once: the legend is a list named by the label, the image is named by the label alone', () => {
    render(<ProgressBar label="Tasks" segments={SEGMENTS} legend />);
    expect(screen.getByRole('img')).toHaveAccessibleName('Tasks');
    const list = screen.getByRole('list', { name: 'Tasks' });
    expect(within(list).getAllByRole('listitem').map(li => li.textContent)).toEqual(['Done6', 'Failed1', 'Running3', 'Skipped0']);
  });

  it('without a legend, keeps the full breakdown in the image name and renders no list', () => {
    render(<ProgressBar label="Tasks" segments={SEGMENTS} />);
    expect(screen.getByRole('img')).toHaveAccessibleName(
      `Tasks: Done 6 (${pct(0.6)}), Failed 1 (${pct(0.1)}), Running 3 (${pct(0.3)}), Skipped 0 (${pct(0)})`,
    );
    expect(screen.queryByRole('list')).toBeNull();
  });

  it('passes axe in every form', async () => {
    render(
      <main>
        <ProgressBar label="Steps" value={3} max={8} valueText="3/8" />
        <ProgressBar label="Hidden" hideLabel value={30} />
        <ProgressBar label="Cloning" />
        <ProgressBar label="Tasks" segments={SEGMENTS} legend />
      </main>,
    );
    const results = await act(() => runAxe(document.querySelector('main')!));
    expect(describeViolations(results)).toEqual([]);
  });
});
