// KT-1886 — StatTile and StatGrid: one figure with its label, unit, icon,
// hint and delta; "—" with its reason when unknown; a placeholder of the same
// height while loading; a button (with aria-pressed when selected) when it
// can be clicked; two sizes. StatGrid wraps its tiles by its container width.
// Written before the component.
import { describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Activity } from 'lucide-react';
import { StatGrid, StatTile } from './StatTile';
import { cssText } from '../../test/cssBlocks';
import { describeViolations, runAxe } from '../../test/axe';

const CSS = cssText(join(dirname(fileURLToPath(import.meta.url)), 'StatTile.css'));

describe('StatTile', () => {
  it('shows its label, its value and its unit, the label at --kr-type-sm', () => {
    const { container } = render(<StatTile label="Tokens" value="12.4" unit="k" hint="Last 7 days" />);
    const tile = container.querySelector('.ui-stat-tile')!;
    expect(tile.tagName).toBe('DIV');
    expect(tile.querySelector('.ui-stat-label')).toHaveTextContent('Tokens');
    expect(tile.querySelector('.ui-stat-value')).toHaveTextContent('12.4');
    expect(tile.querySelector('.ui-stat-unit')).toHaveTextContent('k');
    expect(tile.querySelector('.ui-stat-hint')).toHaveTextContent('Last 7 days');
    expect(CSS).toMatch(/\.ui-stat-label\s*\{[^}]*font-size:\s*var\(--kr-type-sm\)/);
  });

  it('has two sizes: the value at --kr-type-xl (md, default) or --kr-type-2xl (lg)', () => {
    const { container, rerender } = render(<StatTile label="Runs" value={42} />);
    expect(container.querySelector('.ui-stat-tile')).toHaveAttribute('data-size', 'md');
    rerender(<StatTile label="Runs" value={42} size="lg" />);
    expect(container.querySelector('.ui-stat-tile')).toHaveAttribute('data-size', 'lg');
    expect(CSS).toMatch(/\.ui-stat-value\s*\{[^}]*font-size:\s*var\(--kr-type-xl\)/);
    expect(CSS).toMatch(/\[data-size="lg"\] \.ui-stat-value\s*\{[^}]*font-size:\s*var\(--kr-type-2xl\)/);
  });

  it('hides its icon from assistive technology', () => {
    const { container } = render(<StatTile label="Runs" value={42} icon={<Activity data-testid="icon" />} />);
    expect(screen.getByTestId('icon').closest('.ui-stat-icon')).toHaveAttribute('aria-hidden', 'true');
    expect(container.querySelector('.ui-stat-tile')).toBeInTheDocument();
  });

  it('shows "—" for an unknown value, with the reason in a title and for assistive technology', () => {
    const { container } = render(<StatTile label="Cost" value={null} unknownReason="This agent reports no cost" unit="$" />);
    const value = container.querySelector('.ui-stat-value')!;
    expect(value).toHaveAttribute('data-unknown', 'true');
    expect(value).toHaveAttribute('title', 'This agent reports no cost');
    expect(value.querySelector('[aria-hidden="true"]')).toHaveTextContent('—');
    expect(value.querySelector('.ui-visually-hidden')).toHaveTextContent('This agent reports no cost');
    // No unit after a dash.
    expect(container.querySelector('.ui-stat-unit')).toBeNull();
  });

  it('says "Unknown" when no reason is given', () => {
    const { container } = render(<StatTile label="Cost" value={null} />);
    expect(container.querySelector('.ui-stat-value .ui-visually-hidden')).toHaveTextContent('ui.stat.unknown');
  });

  it('keeps its height while loading: a placeholder in place of the value, busy', () => {
    const { container } = render(<StatTile label="Tokens" value="12" loading />);
    const tile = container.querySelector('.ui-stat-tile')!;
    expect(tile).toHaveAttribute('aria-busy', 'true');
    const placeholder = tile.querySelector('.ui-stat-placeholder')!;
    expect(placeholder).toHaveAttribute('aria-hidden', 'true');
    expect(tile.querySelector('.ui-stat-value')).toBeNull();
    expect(tile.querySelector('.ui-visually-hidden')).toHaveTextContent('ui.loading.label');
    // The same line height as the value, at each size.
    expect(CSS).toMatch(/\.ui-stat-placeholder\s*\{[^}]*height:\s*calc\(var\(--kr-type-xl\)\s*\*\s*var\(--ui-stat-line\)\)/);
    expect(CSS).toMatch(/\[data-size="lg"\] \.ui-stat-placeholder\s*\{[^}]*height:\s*calc\(var\(--kr-type-2xl\)\s*\*\s*var\(--ui-stat-line\)\)/);
    expect(CSS).toMatch(/\.ui-stat-value\s*\{[^}]*line-height:\s*var\(--ui-stat-line\)/);
    expect(CSS).toMatch(/@media \(prefers-reduced-motion: reduce\)\s*\{[^}]*\.ui-stat-placeholder[^}]*\{[^}]*animation:\s*none/);
  });

  it('writes the delta direction in words, never by colour alone', () => {
    const { container, rerender } = render(
      <StatTile label="Failures" value={3} delta={{ text: '+2 vs last week', direction: 'up', tone: 'danger' }} />,
    );
    const delta = container.querySelector('.ui-stat-delta')!;
    expect(delta).toHaveAttribute('data-direction', 'up');
    expect(delta).toHaveAttribute('data-tone', 'danger');
    expect(delta.querySelector('.ui-visually-hidden')).toHaveTextContent('ui.stat.up');
    expect(delta).toHaveTextContent('+2 vs last week');
    expect(delta.querySelector('.ui-stat-delta-icon')).toHaveAttribute('aria-hidden', 'true');
    rerender(<StatTile label="Failures" value={3} delta={{ text: '−1', direction: 'down', tone: 'success' }} />);
    expect(container.querySelector('.ui-stat-delta .ui-visually-hidden')).toHaveTextContent('ui.stat.down');
    rerender(<StatTile label="Failures" value={3} delta={{ text: '0', direction: 'flat' }} />);
    expect(container.querySelector('.ui-stat-delta')).toHaveAttribute('data-tone', 'neutral');
    expect(container.querySelector('.ui-stat-delta .ui-visually-hidden')).toHaveTextContent('ui.stat.flat');
    // The tone colours the arrow, never the text.
    expect(CSS).toMatch(/\.ui-stat-delta\s*\{[^}]*color:\s*var\(--kr-text-secondary\)/);
  });

  it('becomes a button when it can be clicked, with aria-pressed when selected', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const { rerender } = render(<StatTile label="Failed" value={3} onClick={onClick} />);
    const button = screen.getByRole('button');
    expect(button).toHaveAttribute('type', 'button');
    expect(button).toHaveTextContent('Failed');
    expect(button).not.toHaveAttribute('aria-pressed');
    await user.click(button);
    button.focus();
    await user.keyboard('{Enter}');
    expect(onClick).toHaveBeenCalledTimes(2);

    rerender(<StatTile label="Failed" value={3} onClick={onClick} selected />);
    expect(screen.getByRole('button')).toHaveAttribute('aria-pressed', 'true');
    rerender(<StatTile label="Failed" value={3} onClick={onClick} selected={false} />);
    expect(screen.getByRole('button')).toHaveAttribute('aria-pressed', 'false');
  });

  it('is no button, and has no aria-pressed, without onClick', () => {
    render(<StatTile label="Failed" value={3} selected />);
    expect(screen.queryByRole('button')).toBeNull();
    expect(document.querySelector('[aria-pressed]')).toBeNull();
  });
});

describe('StatGrid', () => {
  it('wraps its tiles in columns by its container width, no fixed column count', () => {
    const { container } = render(
      <StatGrid label="This week">
        <StatTile label="Runs" value={42} />
        <StatTile label="Failed" value={3} />
      </StatGrid>,
    );
    const grid = screen.getByRole('group', { name: 'This week' });
    expect(grid).toHaveClass('ui-stat-grid');
    expect(container.querySelectorAll('.ui-stat-tile')).toHaveLength(2);
    expect(CSS).toMatch(/\.ui-stat-grid\s*\{[^}]*grid-template-columns:\s*repeat\(auto-fill,\s*minmax\(min\(100%,/);
    expect(CSS).not.toMatch(/repeat\(\d/);
    expect(CSS).not.toMatch(/@media[^{]*width/);
  });

  it('is a plain block without a label', () => {
    render(<StatGrid><StatTile label="Runs" value={42} /></StatGrid>);
    expect(screen.queryByRole('group')).toBeNull();
  });

  it('passes axe with tiles in every state', async () => {
    render(
      <main>
        <StatGrid label="Week">
          <StatTile label="Runs" value={42} unit="runs" icon={<Activity />} delta={{ text: '+4', direction: 'up', tone: 'success' }} />
          <StatTile label="Cost" value={null} unknownReason="No price" />
          <StatTile label="Tokens" value="1" loading />
          <StatTile label="Failed" value={3} onClick={() => {}} selected hint="Click to filter" />
        </StatGrid>
      </main>,
    );
    const results = await act(() => runAxe(document.querySelector('main')!));
    expect(describeViolations(results)).toEqual([]);
  });
});
