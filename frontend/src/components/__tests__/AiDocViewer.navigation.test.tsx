// AiDocViewer — navigation / search / error-state coverage.
//
// Complements AiDocViewer.test.tsx (which pins the docs/ wrapped tree +
// embedded-HTML sanitization). This file targets the previously-uncovered
// behaviour: tree expand/collapse, file selection wiring readAiFile,
// debounced backend search via searchAiFiles, the catch branches of all
// three loaders, and the loading / empty / select-a-file states.
//
// No I18nProvider is mounted, so useT()'s default context returns the
// translation KEY verbatim (e.g. 'projects.docAi.loading') — same
// convention the sibling test relies on. We assert on those keys + on the
// raw file names / paths the component renders.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import type { AiFileNode, AiSearchResult, AiFileContent } from '../../types/generated';

// Hoisted mock fns so individual tests can re-program resolved/rejected
// values per-case (vi.mock factory is hoisted above module consts, so the
// fns must come from vi.hoisted to be referenceable inside it).
const { listAiFiles, readAiFile, searchAiFiles } = vi.hoisted(() => ({
  listAiFiles: vi.fn(),
  readAiFile: vi.fn(),
  searchAiFiles: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: { listAiFiles, readAiFile, searchAiFiles },
}));

// MermaidDiagram pulls a heavy ESM dep + canvas; stub it to a marker so the
// markdown render path doesn't drag it in.
vi.mock('../MermaidDiagram', () => ({
  MermaidDiagram: ({ source }: { source: string }) => <pre data-testid="mermaid">{source}</pre>,
}));

import { AiDocViewer } from '../AiDocViewer';

// ─── Fixtures ──────────────────────────────────────────────────────────────

const TREE: AiFileNode[] = [
  {
    path: 'docs', name: 'docs', is_dir: true, children: [
      {
        path: 'docs/guide', name: 'guide', is_dir: true, children: [
          { path: 'docs/guide/intro.md', name: 'intro.md', is_dir: false, children: [] },
        ],
      },
      { path: 'docs/AGENTS.md', name: 'AGENTS.md', is_dir: false, children: [] },
    ],
  },
  { path: 'README.md', name: 'README.md', is_dir: false, children: [] },
];

const fileContent = (content: string): AiFileContent => ({ path: 'x', content });
const searchHit = (path: string, match_count: number): AiSearchResult => ({ path, match_count });

beforeEach(() => {
  listAiFiles.mockReset();
  readAiFile.mockReset();
  searchAiFiles.mockReset();

  listAiFiles.mockResolvedValue(TREE);
  readAiFile.mockResolvedValue(fileContent('# Default doc\n\nDefault body.'));
  searchAiFiles.mockResolvedValue([]);
});

afterEach(() => {
  vi.clearAllMocks();
  vi.useRealTimers();
  cleanup();
});

// ─── Tree load + entry-file auto-selection ──────────────────────────────────

describe('AiDocViewer — tree load + entry selection', () => {
  it('shows the loading spinner before the tree resolves', () => {
    let resolveTree: (v: AiFileNode[]) => void = () => {};
    listAiFiles.mockReturnValueOnce(new Promise<AiFileNode[]>(r => { resolveTree = r; }));
    render(<AiDocViewer projectId="p1" />);
    // treeLoading branch renders the loading key.
    expect(screen.getByText('projects.docAi.loading')).toBeInTheDocument();
    // Settle the promise so the test doesn't leak an open microtask.
    act(() => resolveTree(TREE));
  });

  it('auto-selects docs/AGENTS.md and loads its content', async () => {
    readAiFile.mockResolvedValue(fileContent('# Agents entry\n\nWelcome.'));
    render(<AiDocViewer projectId="p1" />);

    await waitFor(() => expect(readAiFile).toHaveBeenCalledWith('p1', 'docs/AGENTS.md'));
    expect(await screen.findByText('Agents entry')).toBeInTheDocument();
    // Toolbar surfaces the selected path.
    expect(screen.getByText('docs/AGENTS.md')).toBeInTheDocument();
  });
});

// ─── Tree expand / collapse ─────────────────────────────────────────────────

describe('AiDocViewer — tree expand/collapse', () => {
  it('keeps nested subfolders collapsed until clicked, then reveals children', async () => {
    render(<AiDocViewer projectId="p1" />);
    await waitFor(() => expect(screen.getByText('docs')).toBeInTheDocument());

    // docs/ is seeded open, but docs/guide is a nested dir → collapsed.
    expect(screen.queryByText('intro.md')).toBeNull();

    fireEvent.click(screen.getByText('guide'));
    expect(await screen.findByText('intro.md')).toBeInTheDocument();

    // Toggling again collapses it back.
    fireEvent.click(screen.getByText('guide'));
    await waitFor(() => expect(screen.queryByText('intro.md')).toBeNull());
  });

  it('collapses the seeded docs/ root on click, hiding its direct children', async () => {
    render(<AiDocViewer projectId="p1" />);
    await waitFor(() => expect(screen.getByText('AGENTS.md')).toBeInTheDocument());

    fireEvent.click(screen.getByText('docs'));
    await waitFor(() => expect(screen.queryByText('AGENTS.md')).toBeNull());
  });
});

// ─── File selection ─────────────────────────────────────────────────────────

describe('AiDocViewer — file selection', () => {
  it('selecting a different file calls readAiFile and renders its content', async () => {
    readAiFile.mockResolvedValueOnce(fileContent('# Agents entry'));   // auto-selected
    readAiFile.mockResolvedValueOnce(fileContent('# The readme\n\nReadme body.'));
    render(<AiDocViewer projectId="p1" />);
    await waitFor(() => expect(screen.getByText('README.md')).toBeInTheDocument());

    fireEvent.click(screen.getByText('README.md'));

    await waitFor(() => expect(readAiFile).toHaveBeenCalledWith('p1', 'README.md'));
    expect(await screen.findByText('The readme')).toBeInTheDocument();
  });

  it('fires onDiscussFile with the selected path when the discuss button is clicked', async () => {
    const onDiscussFile = vi.fn();
    render(<AiDocViewer projectId="p1" onDiscussFile={onDiscussFile} />);

    const discussBtn = await screen.findByText('projects.docAi.discuss');
    fireEvent.click(discussBtn);
    expect(onDiscussFile).toHaveBeenCalledWith('docs/AGENTS.md');
  });

  it('shows the resolution-framed CTA for tech-debt TD files', async () => {
    const tdTree: AiFileNode[] = [
      {
        path: 'docs', name: 'docs', is_dir: true, children: [
          {
            path: 'docs/tech-debt', name: 'tech-debt', is_dir: true, children: [
              { path: 'docs/tech-debt/TD-001.md', name: 'TD-001.md', is_dir: false, children: [] },
            ],
          },
        ],
      },
    ];
    listAiFiles.mockResolvedValue(tdTree);
    render(
      <AiDocViewer
        projectId="p1"
        onDiscussFile={vi.fn()}
        initialExpandFolder="docs/tech-debt"
      />,
    );

    // initialExpandFolder preselects the first file under the folder, so the
    // tech-debt CTA ("fixThis") is what renders, not the generic "discuss".
    expect(await screen.findByText('projects.docAi.fixThis')).toBeInTheDocument();
    expect(screen.queryByText('projects.docAi.discuss')).toBeNull();
  });
});

// ─── Search ─────────────────────────────────────────────────────────────────

describe('AiDocViewer — search', () => {
  // Real timers: the debounce is 250ms, comfortably inside waitFor's 1000ms
  // default. Fake timers deadlock against @testing-library's waitFor (which
  // schedules on real timers), so we let the debounce run for real.
  it('debounces input then calls searchAiFiles and shows match badges + counts', async () => {
    searchAiFiles.mockResolvedValue([searchHit('docs/AGENTS.md', 3), searchHit('README.md', 2)]);

    render(<AiDocViewer projectId="p1" />);
    await screen.findByText('docs');

    const input = screen.getByPlaceholderText('projects.docAi.search');
    fireEvent.change(input, { target: { value: 'audit' } });

    // Not called synchronously — the 250ms debounce hasn't elapsed yet.
    expect(searchAiFiles).not.toHaveBeenCalled();

    await waitFor(() => expect(searchAiFiles).toHaveBeenCalledWith('p1', 'audit'));

    // 3 + 2 = 5 total matches → position bar reads "current / 5".
    await waitFor(() => expect(screen.getByText('/ 5', { exact: false })).toBeInTheDocument());
  });

  it('clearing the query via the X button resets results', async () => {
    searchAiFiles.mockResolvedValue([searchHit('docs/AGENTS.md', 1)]);
    render(<AiDocViewer projectId="p1" />);
    await screen.findByText('docs');

    const input = screen.getByPlaceholderText('projects.docAi.search');
    fireEvent.change(input, { target: { value: 'foo' } });
    await waitFor(() => expect(searchAiFiles).toHaveBeenCalled());

    // The clear (X) button appears once the query is non-empty.
    const clearBtn = input.parentElement!.querySelector('.aidoc-search-clear') as HTMLButtonElement;
    expect(clearBtn).toBeTruthy();
    fireEvent.click(clearBtn);

    expect((input as HTMLInputElement).value).toBe('');
    // Results bar (which only shows while searching) is gone.
    await waitFor(() => expect(screen.queryByText('/ 1', { exact: false })).toBeNull());
  });

  it('renders the no-results state when the backend returns no hits', async () => {
    searchAiFiles.mockResolvedValue([]);
    render(<AiDocViewer projectId="p1" />);
    await screen.findByText('docs');

    const input = screen.getByPlaceholderText('projects.docAi.search');
    fireEvent.change(input, { target: { value: 'zzz-no-match' } });

    await waitFor(() => expect(screen.getByText('projects.docAi.noResults')).toBeInTheDocument());
  });

  it('Escape in the search box clears the query', async () => {
    render(<AiDocViewer projectId="p1" />);
    await screen.findByText('docs');

    const input = screen.getByPlaceholderText('projects.docAi.search') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'something' } });
    expect(input.value).toBe('something');

    fireEvent.keyDown(input, { key: 'Escape' });
    expect(input.value).toBe('');
  });
});

// ─── Error / empty states ───────────────────────────────────────────────────

describe('AiDocViewer — error & empty states', () => {
  it('renders the empty state when listAiFiles returns no files', async () => {
    listAiFiles.mockResolvedValue([]);
    render(<AiDocViewer projectId="p1" />);
    expect(await screen.findByText('projects.docAi.empty')).toBeInTheDocument();
  });

  it('shows a retry-able error (NOT the empty state) when listAiFiles rejects', async () => {
    listAiFiles.mockRejectedValue(new Error('network down'));
    render(<AiDocViewer projectId="p1" />);
    // A fetch failure must be distinguishable from "no docs": the old code
    // rendered the identical empty message, so a transient 500/network error
    // looked like "this project has no documentation". Now: error + retry.
    expect(await screen.findByText('projects.docAi.loadError')).toBeInTheDocument();
    expect(screen.getByText('projects.docAi.retry')).toBeInTheDocument();
    expect(screen.queryByText('projects.docAi.empty')).toBeNull();
  });

  it('retry re-fetches and renders the tree after a transient failure', async () => {
    listAiFiles.mockRejectedValueOnce(new Error('network down'));
    render(<AiDocViewer projectId="p1" />);
    const retry = await screen.findByText('projects.docAi.retry');
    // The next call succeeds → clicking retry loads the real tree.
    listAiFiles.mockResolvedValue(TREE);
    fireEvent.click(retry);
    expect(await screen.findByText('docs')).toBeInTheDocument();
    expect(screen.queryByText('projects.docAi.loadError')).toBeNull();
  });

  it('falls back to the select-a-file pane when readAiFile rejects', async () => {
    readAiFile.mockRejectedValue(new Error('cannot read'));
    render(<AiDocViewer projectId="p1" />);
    await waitFor(() => expect(readAiFile).toHaveBeenCalled());
    // catch sets content=null → "select a file" placeholder, no crash.
    expect(await screen.findByText('projects.docAi.selectFile')).toBeInTheDocument();
  });

  it('keeps prior results when searchAiFiles rejects (no crash)', async () => {
    searchAiFiles.mockRejectedValue(new Error('search failed'));
    render(<AiDocViewer projectId="p1" />);
    await screen.findByText('docs');

    const input = screen.getByPlaceholderText('projects.docAi.search');
    fireEvent.change(input, { target: { value: 'boom' } });

    // The rejection catch clears results + loading → no-results shows, no throw.
    await waitFor(() => expect(screen.getByText('projects.docAi.noResults')).toBeInTheDocument());
  });
});

// ─── Banner ─────────────────────────────────────────────────────────────────

describe('AiDocViewer — banner', () => {
  it('renders the banner above the loading state', () => {
    let resolveTree: (v: AiFileNode[]) => void = () => {};
    listAiFiles.mockReturnValueOnce(new Promise<AiFileNode[]>(r => { resolveTree = r; }));
    render(<AiDocViewer projectId="p1" banner={<div>STATE_BANNER</div>} />);
    expect(screen.getByText('STATE_BANNER')).toBeInTheDocument();
    act(() => resolveTree(TREE));
  });

  it('renders the banner above the loaded tree', async () => {
    render(<AiDocViewer projectId="p1" banner={<div>STATE_BANNER</div>} />);
    await waitFor(() => expect(screen.getByText('docs')).toBeInTheDocument());
    expect(screen.getByText('STATE_BANNER')).toBeInTheDocument();
  });
});
