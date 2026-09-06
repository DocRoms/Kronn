// `?raw` resolves relative to this file, so the test does not depend on the
// directory vitest happens to be launched from.
import faviconSource from '../../../public/favicon.svg?raw';
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

  /// The mark exists in two files that cannot import each other: this
  /// component, and the static `public/favicon.svg` the browser reads for the
  /// tab. They already drifted apart once — the favicon kept a dark plate and
  /// paler nodes, and the tab showed what read as a second logo. Nothing but a
  /// test notices when one is edited and the other is not.
  it('draws exactly what the favicon draws', () => {
    const favicon = new DOMParser()
      .parseFromString(faviconSource, 'image/svg+xml').documentElement;
    const { container } = render(<KronnMark />);
    const mark = container.querySelector('svg')!;

    expect(signature(favicon)).toEqual(signature(mark));
  });
});

/// Everything that makes the drawing, and nothing that makes the file: the
/// gradient ids differ by design (the component suffixes them per instance)
/// and the rendered size is a prop.
function signature(svg: Element) {
  const shapes = [...svg.querySelectorAll('path, circle')].map(shape =>
    ['d', 'cx', 'cy', 'r', 'stroke-width', 'stroke-linecap', 'stroke-linejoin', 'fill']
      .map(name => `${name}=${shape.getAttribute(name) ?? ''}`)
      .join(' ')
      // `url(#kronn-hub)` and `url(#kronn-hub-:r0:)` are the same paint — but
      // the hub and the peers are not, so only the id's suffix is dropped.
      .replace(/url\(#(kronn-[a-z]+)[^)]*\)/g, 'url(#$1)'),
  );
  const stops = [...svg.querySelectorAll('stop')].map(
    stop => `${stop.getAttribute('offset')}:${stop.getAttribute('stop-color')}`,
  );
  return { viewBox: svg.getAttribute('viewBox'), shapes, stops };
}
