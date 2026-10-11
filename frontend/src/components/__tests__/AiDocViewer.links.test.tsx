// AiDocViewer — links between docs, anchors, source files and the web (KT-1200).
// No I18nProvider: useT() returns the key itself, so assertions use the keys.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import type { AiFileNode, AiFileContent } from '../../types/generated';

const { listAiFiles, readAiFile, searchAiFiles } = vi.hoisted(() => ({
  listAiFiles: vi.fn(),
  readAiFile: vi.fn(),
  searchAiFiles: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: { listAiFiles, readAiFile, searchAiFiles },
}));

vi.mock('../MermaidDiagram', () => ({
  MermaidDiagram: ({ source }: { source: string }) => <pre data-testid="mermaid">{source}</pre>,
}));

vi.mock('../SourceCodeViewer', () => ({
  SourceCodeViewer: ({ initialPath, initialLine }: { initialPath?: string | null; initialLine?: number | null }) => (
    <div data-testid="source-viewer">{`${initialPath}@${initialLine ?? 'none'}`}</div>
  ),
}));

import { AiDocViewer } from '../AiDocViewer';

const file = (path: string): AiFileNode => ({ path, name: path.split('/').pop() ?? path, is_dir: false, children: [] });
const TREE: AiFileNode[] = [
  {
    path: 'docs', name: 'docs', is_dir: true, children: [
      file('docs/AGENTS.md'),
      file('docs/testing-quality.md'),
      { path: 'docs/tech-debt', name: 'tech-debt', is_dir: true, children: [file('docs/tech-debt/TD-001-x.md')] },
    ],
  },
];

const DOCS: Record<string, string> = {
  'docs/AGENTS.md': [
    '# Agents',
    '',
    '- [Testing](testing-quality.md)',
    '- [Testing section](testing-quality.md#run-the-tests)',
    '- [TD sheet](tech-debt/TD-001-x.md)',
    '- [Router](../backend/src/lib.rs:120)',
    '- [Ghost](ghost.md)',
    '- [Escape](../../outside.md)',
    '- [Web](https://example.com/page)',
    '- [Local](#local-part)',
    '',
    '## Local part',
    '',
    'Body.',
  ].join('\n'),
  'docs/testing-quality.md': '# Testing quality\n\nIntro.\n\n## Run the tests\n\nCommands.',
  'docs/tech-debt/TD-001-x.md': '# TD-001\n\n[Back to agents](../AGENTS.md)',
};

let scrolled: string[] = [];
const originalScrollIntoView = Element.prototype.scrollIntoView;

beforeEach(() => {
  listAiFiles.mockReset().mockResolvedValue(TREE);
  readAiFile.mockReset().mockImplementation((_id: string, path: string): Promise<AiFileContent> =>
    path in DOCS ? Promise.resolve({ path, content: DOCS[path] }) : Promise.reject(new Error('missing')));
  searchAiFiles.mockReset().mockResolvedValue([]);
  scrolled = [];
  Element.prototype.scrollIntoView = vi.fn(function (this: Element) { scrolled.push(this.id); });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  Element.prototype.scrollIntoView = originalScrollIntoView;
});

async function openAgents() {
  render(<AiDocViewer projectId="p1" />);
  await screen.findByRole('heading', { name: 'Agents' });
}

describe('AiDocViewer — in-app doc links', () => {
  it('opens a relative docs link in the viewer without letting the browser navigate', async () => {
    await openAgents();
    const link = screen.getByRole('link', { name: 'Testing' });
    expect(link).not.toHaveAttribute('target');
    // fireEvent returns false when the handler called preventDefault.
    expect(fireEvent.click(link)).toBe(false);

    expect(await screen.findByRole('heading', { name: 'Testing quality' })).toBeInTheDocument();
    expect(readAiFile).toHaveBeenLastCalledWith('p1', 'docs/testing-quality.md');
  });

  it('resolves a link against the current doc folder', async () => {
    await openAgents();
    fireEvent.click(screen.getByRole('link', { name: 'TD sheet' }));
    await screen.findByRole('heading', { name: 'TD-001' });
    fireEvent.click(screen.getByRole('link', { name: 'Back to agents' }));
    expect(await screen.findByRole('heading', { name: 'Agents' })).toBeInTheDocument();
    expect(readAiFile).toHaveBeenLastCalledWith('p1', 'docs/AGENTS.md');
  });

  it('scrolls to the heading named by an anchor, in another doc or the same one', async () => {
    await openAgents();
    fireEvent.click(screen.getByRole('link', { name: 'Testing section' }));
    await screen.findByRole('heading', { name: 'Testing quality' });
    await waitFor(() => expect(scrolled).toContain('run-the-tests'));
    expect(screen.getByRole('heading', { name: 'Run the tests' })).toHaveAttribute('id', 'run-the-tests');

    fireEvent.click(screen.getByRole('button', { name: 'projects.docAi.back' }));
    await screen.findByRole('heading', { name: 'Agents' });
    expect(fireEvent.click(screen.getByRole('link', { name: 'Local' }))).toBe(false);
    await waitFor(() => expect(scrolled).toContain('local-part'));
  });

  it('opens a source link in SourceCodeViewer at the line', async () => {
    await openAgents();
    expect(fireEvent.click(screen.getByRole('link', { name: 'Router' }))).toBe(false);
    expect(await screen.findByTestId('source-viewer')).toHaveTextContent('backend/src/lib.rs@120');
    fireEvent.click(screen.getByRole('button', { name: 'projects.docAi.closeCode' }));
    expect(screen.queryByTestId('source-viewer')).toBeNull();
  });

  it('sends external links to a new tab with rel noopener noreferrer', async () => {
    await openAgents();
    const link = screen.getByRole('link', { name: 'Web' });
    expect(link).toHaveAttribute('href', 'https://example.com/page');
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('shows a message for a missing doc and for a path outside the project, and stays put', async () => {
    await openAgents();
    expect(fireEvent.click(screen.getByRole('link', { name: 'Ghost' }))).toBe(false);
    expect(screen.getByRole('alert')).toHaveTextContent('projects.docAi.linkMissing');
    expect(screen.getByRole('heading', { name: 'Agents' })).toBeInTheDocument();

    expect(fireEvent.click(screen.getByRole('link', { name: 'Escape' }))).toBe(false);
    expect(screen.getByRole('alert')).toHaveTextContent('projects.docAi.linkOutside');
    expect(readAiFile).not.toHaveBeenCalledWith('p1', expect.stringContaining('outside'));
  });

  it('goes back to the previous doc, then hides the back button', async () => {
    await openAgents();
    expect(screen.queryByRole('button', { name: 'projects.docAi.back' })).toBeNull();
    fireEvent.click(screen.getByRole('link', { name: 'Testing' }));
    await screen.findByRole('heading', { name: 'Testing quality' });

    fireEvent.click(screen.getByRole('button', { name: 'projects.docAi.back' }));
    expect(await screen.findByRole('heading', { name: 'Agents' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'projects.docAi.back' })).toBeNull();
  });

  it('swallows a middle-click on an internal link', async () => {
    await openAgents();
    const link = screen.getByRole('link', { name: 'Testing' });
    expect(fireEvent(link, new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 1 }))).toBe(false);
  });
});
