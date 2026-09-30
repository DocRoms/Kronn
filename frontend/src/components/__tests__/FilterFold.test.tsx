import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { FilterFold } from '../FilterFold';

// vitest blanks CSS, so the stylesheet is read as text.
const foldCss = readFileSync(join(import.meta.dirname, '..', 'FilterFold.css'), 'utf-8');

const stubViewport = (matches: boolean) => vi.stubGlobal('matchMedia', vi.fn().mockImplementation((query: string) => ({
  matches, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn(),
})));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('FilterFold', () => {
  it('renders its filters in place on a wide screen, with no button of its own', () => {
    stubViewport(false);
    render(<FilterFold label="Filtres" activeCount={3}><button type="button">Chip</button></FilterFold>);
    expect(screen.getByRole('button', { name: 'Chip' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Filtres/ })).toBeNull();
  });

  it('names the number of set filters on the button, and none when there is none', () => {
    stubViewport(true);
    const { rerender } = render(<FilterFold label="Filtres" activeCount={0}><button type="button">Chip</button></FilterFold>);
    expect(screen.getByRole('button', { name: 'Filtres' })).not.toHaveAttribute('data-active');
    rerender(<FilterFold label="Filtres" activeCount={3}><button type="button">Chip</button></FilterFold>);
    expect(screen.getByRole('button', { name: 'Filtres (3)' })).toHaveAttribute('data-active', 'true');
  });

  it('lets nothing outgrow the viewport, and uses design tokens only', () => {
    expect(foldCss).toMatch(/\.kr-filter-fold \{[^}]*width: 100%[^}]*min-width: 0/);
    expect(foldCss).toMatch(/\.kr-filter-fold-panel \{[^}]*min-width: 0/);
    expect(foldCss).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(|var\(--(?!kr-)/);
  });
});
