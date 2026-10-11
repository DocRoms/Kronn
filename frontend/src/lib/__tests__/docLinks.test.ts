import { describe, it, expect } from 'vitest';
import { buildDocIndex, resolveDocLink, slugifyHeading } from '../docLinks';
import type { AiFileNode } from '../../types/generated';

const file = (path: string): AiFileNode => ({ path, name: path.split('/').pop() ?? path, is_dir: false, children: [] });
const TREE: AiFileNode[] = [
  {
    path: 'docs', name: 'docs', is_dir: true, children: [
      file('docs/AGENTS.md'),
      file('docs/testing-quality.md'),
      { path: 'docs/architecture', name: 'architecture', is_dir: true, children: [file('docs/architecture/overview.md')] },
      { path: 'docs/tech-debt', name: 'tech-debt', is_dir: true, children: [file('docs/tech-debt/TD-001-x.md')] },
    ],
  },
  file('README.md'),
];
const index = buildDocIndex(TREE);
const from = (href: string, current = 'docs/architecture/overview.md') => resolveDocLink(href, current, index);

describe('resolveDocLink', () => {
  it('resolves docs links relative to the current doc', () => {
    expect(from('../testing-quality.md')).toEqual({ kind: 'doc', path: 'docs/testing-quality.md', anchor: null });
    expect(from('./overview.md#flow')).toEqual({ kind: 'doc', path: 'docs/architecture/overview.md', anchor: 'flow' });
    expect(from('testing-quality.md', 'docs/AGENTS.md')).toEqual({ kind: 'doc', path: 'docs/testing-quality.md', anchor: null });
    expect(from('../tech-debt/TD-001-x.md')).toMatchObject({ kind: 'doc', path: 'docs/tech-debt/TD-001-x.md' });
  });

  it('falls back to the repo root, and honours a leading slash', () => {
    expect(from('docs/testing-quality.md')).toMatchObject({ kind: 'doc', path: 'docs/testing-quality.md' });
    expect(from('/README.md')).toMatchObject({ kind: 'doc', path: 'README.md' });
    expect(from('../../README.md')).toMatchObject({ kind: 'doc', path: 'README.md' });
  });

  it('treats a bare fragment as an in-page anchor', () => {
    expect(from('#run-the-tests')).toEqual({ kind: 'anchor', anchor: 'run-the-tests' });
  });

  it('opens source files at their line, root-relative when the path has folders', () => {
    expect(from('backend/src/lib.rs:120')).toEqual({ kind: 'code', path: 'backend/src/lib.rs', line: 120 });
    expect(from('backend/src/lib.rs:120-140')).toEqual({ kind: 'code', path: 'backend/src/lib.rs', line: 120 });
    expect(from('../../backend/src/lib.rs#L42')).toEqual({ kind: 'code', path: 'backend/src/lib.rs', line: 42 });
    expect(from('/Makefile')).toEqual({ kind: 'code', path: 'Makefile', line: null });
    expect(from('../../CHANGELOG.md')).toEqual({ kind: 'code', path: 'CHANGELOG.md', line: null });
  });

  it('reports missing docs and paths above the project root', () => {
    expect(from('ghost.md')).toEqual({ kind: 'missing', path: 'docs/architecture/ghost.md' });
    expect(from('../../../etc/passwd')).toEqual({ kind: 'outside', href: '../../../etc/passwd' });
  });

  it('keeps web links external and neutralises other schemes', () => {
    expect(from('https://example.com')).toEqual({ kind: 'external', href: 'https://example.com' });
    expect(from('mailto:a@b.c')).toEqual({ kind: 'external', href: 'mailto:a@b.c' });
    expect(from('javascript:alert(1)')).toEqual({ kind: 'inert' });
    expect(from('//evil.example/x.md')).toEqual({ kind: 'inert' });
    expect(from(undefined as unknown as string)).toEqual({ kind: 'inert' });
  });
});

describe('slugifyHeading', () => {
  it('matches the GitHub slug rule, unicode included', () => {
    expect(slugifyHeading('Run the tests!')).toBe('run-the-tests');
    expect(slugifyHeading('0. Anti-Hallucination Protocol')).toBe('0-anti-hallucination-protocol');
    expect(slugifyHeading('Qualité & tests')).toBe('qualité--tests');
  });
});
