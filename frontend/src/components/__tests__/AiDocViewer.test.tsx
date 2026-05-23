// 0.8.6 UX — the doc viewer now shows an explicit `docs/` root folder
// (expanded by default) plus the project's root README, instead of dumping
// the loose contents of docs/ with no context. These guard that the wrapped
// tree renders with docs/ open and the README surfaced at the root.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

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
  },
}));

import { AiDocViewer } from '../AiDocViewer';

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
});
