/**
 * KT-65 — the panel must ASK the backend with the filters the user picked, and
 * page instead of pulling everything. These tests pin the request it builds
 * (that is where a silent bug would hide) and the hand-off of a chosen result.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, cleanup, waitFor } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  discussions: { searchMessages: vi.fn() },
}));

import { GlobalSearchPanel } from '../GlobalSearchPanel';
import { discussions as discussionsApi } from '../../lib/api';
import type { MessageSearchHit, Project } from '../../types/generated';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}(${args.join(',')})` : key;

const makeHit = (id: string, overrides: Partial<MessageSearchHit> = {}): MessageSearchHit => ({
  disc_id: 'd-1',
  disc_title: 'Kronn 0.9.2',
  message_id: id,
  sort_order: 42,
  role: 'Agent',
  timestamp: '2026-07-27T10:00:00Z',
  snippet: `…extrait ${id}…`,
  agent_type: 'Codex',
  // ts-rs types these as required-nullable (it ignores `skip_serializing_if`),
  // so spell them out rather than letting the spread make them optional.
  author_pseudo: null,
  project_id: null,
  ...overrides,
});

const projects: Project[] = [
  {
    id: 'proj-1',
    name: 'Kronn',
    path: '/repos/kronn',
    repo_url: null,
    token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate',
  } as unknown as Project,
];

const searchMessages = () => discussionsApi.searchMessages as ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  searchMessages().mockResolvedValue([]);
});
afterEach(cleanup);

const renderPanel = (
  onOpenResult = vi.fn(),
  onClose = vi.fn(),
  initialQuery = '',
  onQueryChange = vi.fn(),
) => {
  render(
    <GlobalSearchPanel
      projects={projects}
      authors={['Codex', 'Romu - mac']}
      initialQuery={initialQuery}
      onQueryChange={onQueryChange}
      onOpenResult={onOpenResult}
      onClose={onClose}
      t={t}
    />,
  );
  return { onOpenResult, onClose, onQueryChange };
};

const submit = async (query: string) => {
  const input = document.querySelector('[data-testid="global-search-input"]') as HTMLInputElement;
  fireEvent.change(input, { target: { value: query } });
  await act(async () => {
    fireEvent.submit(input.closest('form')!);
    await Promise.resolve();
  });
};

describe('GlobalSearchPanel', () => {
  it('does not query the backend on an empty term', async () => {
    renderPanel();
    await submit('   ');
    expect(searchMessages()).not.toHaveBeenCalled();
  });

  it('reuses and updates the sidebar quick-search query', () => {
    const { onQueryChange } = renderPanel(vi.fn(), vi.fn(), 'Fastly');
    const input = document.querySelector('[data-testid="global-search-input"]') as HTMLInputElement;
    expect(input.value).toBe('Fastly');
    fireEvent.change(input, { target: { value: 'Fastly purge' } });
    expect(onQueryChange).toHaveBeenCalledWith('Fastly purge');
  });

  it('runs a non-empty sidebar query immediately when the panel opens', async () => {
    renderPanel(vi.fn(), vi.fn(), 'Fastly');

    await waitFor(() => {
      expect(searchMessages()).toHaveBeenCalledWith(expect.objectContaining({
        q: 'Fastly',
        limit: 20,
        offset: 0,
      }));
    });
  });

  it('sends the picked filters and widens a date to cover the whole day', async () => {
    renderPanel();
    fireEvent.change(document.querySelector('[data-testid="global-search-project"]') as HTMLSelectElement, {
      target: { value: 'proj-1' },
    });
    fireEvent.change(document.querySelector('[data-testid="global-search-author"]') as HTMLSelectElement, {
      target: { value: 'Codex' },
    });
    fireEvent.change(document.querySelector('[data-testid="global-search-since"]') as HTMLInputElement, {
      target: { value: '2026-07-01' },
    });
    fireEvent.change(document.querySelector('[data-testid="global-search-until"]') as HTMLInputElement, {
      target: { value: '2026-07-27' },
    });
    await submit('probe Fastly');

    expect(searchMessages()).toHaveBeenCalledWith(expect.objectContaining({
      q: 'probe Fastly',
      projectId: 'proj-1',
      author: 'Codex',
      // A bare day would exclude everything said that day after midnight.
      since: '2026-07-01T00:00:00Z',
      until: '2026-07-27T23:59:59Z',
      limit: 20,
      offset: 0,
    }));
  });

  it('pages with an offset instead of asking for the whole history', async () => {
    renderPanel();
    searchMessages().mockResolvedValue(Array.from({ length: 20 }, (_, i) => makeHit(`m${i}`)));
    await submit('Fastly');

    const more = document.querySelector('[data-testid="global-search-more"]') as HTMLButtonElement;
    expect(more).not.toBeNull();
    searchMessages().mockResolvedValue([makeHit('m20')]);
    await act(async () => {
      fireEvent.click(more);
      await Promise.resolve();
    });

    expect(searchMessages()).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 20 }));
    expect(document.querySelectorAll('.disc-global-search-hit')).toHaveLength(21);
    // A short page means the end — no more button to click.
    expect(document.querySelector('[data-testid="global-search-more"]')).toBeNull();
  });

  it('hands the chosen hit back with its message identity', async () => {
    const { onOpenResult } = renderPanel();
    searchMessages().mockResolvedValue([makeHit('m-target', { sort_order: 99 })]);
    await submit('Fastly');

    fireEvent.click(document.querySelector('.disc-global-search-hit') as HTMLButtonElement);
    expect(onOpenResult).toHaveBeenCalledWith(expect.objectContaining({
      message_id: 'm-target',
      disc_id: 'd-1',
      sort_order: 99,
    }));
  });

  it('reports a failure instead of showing an empty result', async () => {
    renderPanel();
    searchMessages().mockRejectedValue(new Error('backend down'));
    await submit('Fastly');
    expect(document.querySelector('.disc-global-search-error')?.textContent).toContain('backend down');
    expect(document.querySelector('.disc-global-search-empty')).toBeNull();
  });

  it('marks the searched term inside the excerpt', async () => {
    renderPanel();
    searchMessages().mockResolvedValue([
      makeHit('m1', { snippet: '…le probe Fastly répond 200, Fastly côté Docker ?…' }),
    ]);
    await submit('fastly');

    const marks = document.querySelectorAll('.disc-global-search-hit-snippet mark');
    expect(marks).toHaveLength(2);
    // Case-insensitive match, but the ORIGINAL casing is displayed.
    expect(marks[0].textContent).toBe('Fastly');
    // The surrounding text survives intact.
    expect(document.querySelector('.disc-global-search-hit-snippet')?.textContent)
      .toBe('…le probe Fastly répond 200, Fastly côté Docker ?…');
  });

  it('never lets a query reach the DOM as markup', async () => {
    renderPanel();
    searchMessages().mockResolvedValue([makeHit('m1', { snippet: 'avant <b>x</b> après' })]);
    await submit('<b>x</b>');
    const snippet = document.querySelector('.disc-global-search-hit-snippet')!;
    expect(snippet.querySelector('b')).toBeNull();
    expect(snippet.textContent).toBe('avant <b>x</b> après');
  });

  it('highlights the term the results came from, not the one being typed', async () => {
    renderPanel();
    searchMessages().mockResolvedValue([makeHit('m1', { snippet: 'un extrait Fastly ici' })]);
    await submit('Fastly');

    // The user starts composing the next query; the marks must not follow.
    const input = document.querySelector('[data-testid="global-search-input"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'extrait' } });
    expect(document.querySelector('.disc-global-search-hit-snippet mark')?.textContent).toBe('Fastly');
  });

  it('closes on Escape', async () => {
    const { onClose } = renderPanel();
    await act(async () => { fireEvent.keyDown(window, { key: 'Escape' }); });
    expect(onClose).toHaveBeenCalled();
  });
});
