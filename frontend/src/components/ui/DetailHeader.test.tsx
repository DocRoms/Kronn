import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { DetailHeader } from './DetailHeader';
import { StatusBadge } from './StatusBadge';
import { describeHeaderContract } from '../../test/headerContract';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

describeHeaderContract('DetailHeader', DetailHeader, 'detail');

// `DetailHeaderShell` is deprecated in favour of this kind: its two current
// users move in lots 2-5, and no new one may appear meanwhile.
describe('DetailHeaderShell — deprecated, kept for its two current users (KT-1884)', () => {
  const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
  const ALLOWED = ['components/ChatHeader.tsx', 'pages/PagesPage.tsx'];
  const sources = (dir: string): string[] => readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === 'node_modules' ? [] : sources(path);
    return /\.(ts|tsx|js|jsx|mjs)$/.test(name) ? [path] : [];
  });
  // A static import, a re-export, a dynamic import or a vi.mock of the module.
  const IMPORT = /(?:\bfrom\s*|\bimport\s*\(\s*|\bimport\s+|\bvi\.mock\(\s*)['"][^'"]*\/DetailHeaderShell(?:\.tsx?)?['"]/;

  it('is imported by ChatHeader.tsx and pages/PagesPage.tsx only', () => {
    const importers = sources(SRC)
      .filter(path => !path.endsWith(`${sep}DetailHeaderShell.tsx`))
      .filter(path => IMPORT.test(readFileSync(path, 'utf8')))
      .map(path => relative(SRC, path).split(sep).join('/'))
      .sort();
    expect(importers).toEqual(ALLOWED);
  });

  it('says so where a developer reads it', () => {
    const shell = readFileSync(join(SRC, 'components', 'DetailHeaderShell.tsx'), 'utf8');
    expect(shell).toMatch(/@deprecated[\s\S]*DetailHeader[\s\S]*docs\/conventions\/header-kinds\.md/);
  });
});

describe('DetailHeader', () => {
  it('names the object (h2) with its id and state on the title line', () => {
    render(
      <DetailHeader
        title="Nightly digest"
        meta={<code>wf-42</code>}
        status={<StatusBadge tone="success">Active</StatusBadge>}
        description="Cron · every night"
        actions={<button type="button">Run now</button>}
      />,
    );
    const heading = screen.getByRole('heading', { level: 2 });
    expect(heading).toHaveTextContent('Nightly digest');
    // The id and the state sit beside the heading, not inside its name.
    expect(heading).not.toHaveTextContent('wf-42');
    const row = heading.parentElement!;
    expect(row).toHaveTextContent('wf-42');
    expect(row).toHaveTextContent('Active');
  });

  it('keeps a long title on one line so the actions stay on screen', () => {
    const long = 'A very long discussion title '.repeat(10);
    render(<DetailHeader title={long} actions={<button type="button">Run now</button>} />);
    expect(screen.getByRole('heading', { level: 2 })).toHaveClass('ui-header-title');
    expect(screen.getByRole('button', { name: 'Run now' }).closest('.ui-header-actions')).toBeTruthy();
  });
});
