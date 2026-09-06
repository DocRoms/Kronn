import { render, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { KronnMark } from '../KronnMark';

afterEach(() => cleanup());

describe('KronnMark', () => {
  /// The mark replaced a lucide bolt next to the app name, so it has to work
  /// where an icon works: sized by a prop, decorative by default.
  it('renders at the asked size and stays out of the accessibility tree', () => {
    const { container } = render(<KronnMark size={20} />);
    const svg = container.querySelector('svg')!;
    expect(svg.getAttribute('width')).toBe('20');
    expect(svg.getAttribute('height')).toBe('20');
    expect(svg.getAttribute('aria-hidden')).toBe('true');
  });

  it('takes a name when it stands alone', () => {
    const { container } = render(<KronnMark title="Kronn" />);
    const svg = container.querySelector('svg')!;
    expect(svg.getAttribute('role')).toBe('img');
    expect(svg.getAttribute('aria-label')).toBe('Kronn');
    expect(svg.querySelector('title')?.textContent).toBe('Kronn');
  });

  /// Two marks on one page — the header and the settings card — would share a
  /// gradient id if it were a constant, and the second would paint with the
  /// first one's stops.
  it('gives each instance its own gradient ids', () => {
    const { container } = render(
      <>
        <KronnMark />
        <KronnMark />
      </>,
    );
    const ids = [...container.querySelectorAll('linearGradient')].map(g => g.id);
    expect(ids).toHaveLength(8);
    expect(new Set(ids).size).toBe(8);
  });

  /// The frame gradient spans the whole mark rather than each stroke: with the
  /// default per-element units every arc restated the full cyan-to-magenta run
  /// and the hexagon read as three unrelated gradients.
  it('runs one gradient across the whole frame', () => {
    const { container } = render(<KronnMark />);
    const frame = container.querySelector('linearGradient')!;
    expect(frame.getAttribute('gradientUnits')).toBe('userSpaceOnUse');
  });
});
