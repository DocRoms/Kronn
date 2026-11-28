// KT-1883 — LoadingState in three sizes and the Skeleton of a list or a
// detail pane. The cold-start behaviour of md and lg (draw nothing while the
// index.html screen is up) is tested in lib/__tests__/bootScreen.test.tsx,
// through the components/LoadingState re-export its users still import.
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LoadingState, Skeleton } from './LoadingState';
import { LOADING_SIZES, SKELETON_KINDS } from './feedbackKinds';
import * as legacy from '../LoadingState';
import { describeViolations, runAxe } from '../../test/axe';

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(join(HERE, 'LoadingState.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

describe('LoadingState', () => {
  it('is the same component its ten users import from components/LoadingState', () => {
    expect(legacy.LoadingState).toBe(LoadingState);
  });

  it.each(LOADING_SIZES)('announces its sentence politely at %s', size => {
    render(<LoadingState size={size} message="Loading the tasks…" />);
    const status = screen.getByRole('status');
    expect(status).toHaveAttribute('data-size', size);
    expect(status).toHaveAttribute('aria-live', 'polite');
    expect(status).toHaveTextContent('Loading the tasks…');
  });

  it('is inline at sm, a ring beside the text, with no mark', () => {
    render(<LoadingState size="sm" message="Saving…" />);
    const status = screen.getByRole('status');
    expect(status.tagName).toBe('SPAN');
    expect(status.querySelector('.ui-loading-ring')).toHaveAttribute('aria-hidden', 'true');
    expect(status.querySelector('svg')).toBeNull();
  });

  it('draws the mark larger at lg than at md, and at lg in full screen', () => {
    const edge = (el: Element) => Number(el.querySelector('svg')!.getAttribute('width'));
    const { unmount } = render(<LoadingState size="md" message="m" />);
    const md = edge(screen.getByRole('status'));
    unmount();
    const second = render(<LoadingState size="lg" message="l" />);
    const lg = edge(screen.getByRole('status'));
    second.unmount();
    expect(lg).toBeGreaterThan(md);
    render(<LoadingState fullscreen message="f" />);
    expect(screen.getByRole('status')).toHaveAttribute('data-size', 'lg');
    expect(screen.getByRole('status')).toHaveAttribute('data-fullscreen');
  });

  it('offers to retry when given onRetry', async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn();
    render(<LoadingState phase="slow" onRetry={onRetry} />);
    await user.click(screen.getByRole('button'));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it('stops every motion under prefers-reduced-motion', () => {
    const animated = [...CSS.matchAll(/([^{}]+)\{[^{}]*animation:/g)].map(m => m[1].trim());
    const reduced = CSS.slice(CSS.indexOf('@media (prefers-reduced-motion: reduce)'));
    expect(animated.length).toBeGreaterThan(0);
    for (const selectors of animated) {
      for (const selector of selectors.split(',').map(s => s.trim())) expect(reduced, selector).toContain(selector);
    }
    expect(reduced).toMatch(/animation:\s*none/);
  });
});

describe('Skeleton', () => {
  it('has a list and a detail shape', () => {
    expect(SKELETON_KINDS).toEqual(['list', 'detail']);
  });

  it('draws the rows of a list, hidden from assistive technology, under one named status that is read', () => {
    render(<Skeleton kind="list" rows={6} label="Loading the projects" />);
    const status = screen.getByRole('status');
    // aria-busy on a live region holds its announcement until it is no longer
    // busy; the skeleton unmounts before that, so the label would go unread.
    expect(status).not.toHaveAttribute('aria-busy');
    expect(status.querySelector('[aria-busy]')).toBeNull();
    expect(status).toHaveTextContent('Loading the projects');
    const list = status.querySelector('.ui-skeleton-list')!;
    expect(list).toHaveAttribute('aria-hidden', 'true');
    expect(list.querySelectorAll('.ui-skeleton-row')).toHaveLength(6);
  });

  it('draws the title and the body of a detail pane, with the default label', () => {
    render(<Skeleton kind="detail" />);
    const status = screen.getByRole('status');
    expect(status).toHaveTextContent('ui.loading.label');
    expect(status.querySelector('[data-line="title"]')).toBeInTheDocument();
    expect(status.querySelector('.ui-skeleton-paragraph')!.children.length).toBeGreaterThan(1);
  });

  it('gives every line width it draws a rule', () => {
    const { container } = render(<><Skeleton kind="list" /><Skeleton kind="detail" /></>);
    for (const line of container.querySelectorAll('[data-w]')) {
      expect(CSS, line.getAttribute('data-w')!).toContain(`.ui-skeleton-line[data-w="${line.getAttribute('data-w')}"]`);
    }
  });

  it('passes axe, with the three sizes beside it', async () => {
    const { container } = render(<main>
      {LOADING_SIZES.map(size => <LoadingState key={size} size={size} message={`Loading ${size}`} />)}
      <Skeleton kind="list" />
      <Skeleton kind="detail" />
    </main>);
    const results = await act(() => runAxe(container));
    expect(describeViolations(results)).toEqual([]);
  });
});
