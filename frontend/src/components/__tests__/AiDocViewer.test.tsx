// 0.8.6 UX — the doc viewer now shows an explicit `docs/` root folder
// (expanded by default) plus the project's root README, instead of dumping
// the loose contents of docs/ with no context. These guard that the wrapped
// tree renders with docs/ open and the README surfaced at the root.

import { describe, it, expect, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { projects } from '../../lib/api';

// Tree is inlined in the factory (vi.mock is hoisted above module consts).
vi.mock('../../lib/api', () => buildApiMock({
  projects: {
    listAiFiles: vi.fn().mockResolvedValue([
      {
        path: 'docs', name: 'docs', is_dir: true, children: [
          {
            path: 'docs/architecture', name: 'architecture', is_dir: true, children: [
              { path: 'docs/architecture/overview.md', name: 'overview.md', is_dir: false, children: [] },
            ],
          },
          { path: 'docs/AGENTS.md', name: 'AGENTS.md', is_dir: false, children: [] },
        ],
      },
      { path: 'README.md', name: 'README.md', is_dir: false, children: [] },
    ]),
    readAiFile: vi.fn().mockResolvedValue({
      path: 'docs/AGENTS.md',
      content: [
        '<p align="center"><img src="https://img.shields.io/badge/Demo-blue" alt="Demo badge" /></p>',
        '',
        '<script>window.__xss_doc = true;</script>',
        '',
        '# Heading',
        '',
        'Body text.',
      ].join('\n'),
    }),
    searchAiFiles: vi.fn().mockResolvedValue([]),
  },
}));

import { AiDocViewer } from '../AiDocViewer';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(next => { resolve = next; });
  return { promise, resolve };
}

describe('AiDocViewer — docs/ root folder + project README', () => {
  it('shows the docs/ folder expanded with its top-level contents, plus the root README', async () => {
    render(<AiDocViewer projectId="p1" />);

    // Tree loads asynchronously from listAiFiles.
    await waitFor(() => expect(screen.getByText('docs')).toBeInTheDocument());

    // docs/ is seeded open → its immediate children are visible with no click.
    expect(screen.getByText('AGENTS.md')).toBeInTheDocument();
    expect(screen.getByText('architecture')).toBeInTheDocument();

    // The project's README sits at the tree root, next to docs/ — the new
    // "see the rest, and preview it without an IDE" affordance.
    expect(screen.getByText('README.md')).toBeInTheDocument();

    // Nested subfolders stay collapsed — only the docs/ root is auto-opened.
    expect(screen.queryByText('overview.md')).toBeNull();
  });

  it('renders embedded HTML (centered badge image) and strips scripts', async () => {
    render(<AiDocViewer projectId="p1" />);

    // The HTML <img> inside <p align="center"> is rendered as a real image
    // (pre-fix it showed as raw "<p align=...>" text).
    const badge = await screen.findByAltText('Demo badge');
    expect(badge).toBeInTheDocument();
    expect(badge.tagName).toBe('IMG');
    // align="center" → text-align:center on the wrapping <p>.
    expect((badge.closest('p') as HTMLElement).style.textAlign).toBe('center');
    // Markdown still renders alongside the HTML.
    expect(screen.getByText('Heading')).toBeInTheDocument();
    // The <script> is sanitized away — neither executed nor shown as text.
    expect((window as unknown as Record<string, unknown>).__xss_doc).toBeUndefined();
    expect(document.body.textContent).not.toContain('window.__xss_doc');
  });

  it('keeps the newest search result when an older request resolves last', async () => {
    const older = deferred<Array<{ path: string; match_count: number }>>();
    const newer = deferred<Array<{ path: string; match_count: number }>>();
    vi.mocked(projects.searchAiFiles)
      .mockImplementationOnce(() => older.promise)
      .mockImplementationOnce(() => newer.promise);

    const { container } = render(<AiDocViewer projectId="p1" />);
    await screen.findByText('AGENTS.md');
    const input = container.querySelector<HTMLInputElement>('.aidoc-search-input');
    expect(input).not.toBeNull();

    fireEvent.change(input!, { target: { value: 'older' } });
    await waitFor(() => expect(projects.searchAiFiles).toHaveBeenCalledWith('p1', 'older'));
    fireEvent.change(input!, { target: { value: 'newer' } });
    await waitFor(() => expect(projects.searchAiFiles).toHaveBeenCalledWith('p1', 'newer'));

    await act(async () => {
      newer.resolve([{ path: 'docs/AGENTS.md', match_count: 2 }]);
      await newer.promise;
    });
    expect(await screen.findByText('1 / 2')).toBeInTheDocument();

    await act(async () => {
      older.resolve([{ path: 'README.md', match_count: 1 }]);
      await older.promise;
    });
    expect(screen.getByText('1 / 2')).toBeInTheDocument();
    expect(container.querySelector('.aidoc-search-results .text-dim')).toBeNull();
  });
});
