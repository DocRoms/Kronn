// KT-1990 — the Artifact library holds one folder per run whose executions a
// person turned into Artifacts, with the batch-folder interaction of the
// Discussions sidebar: the header opens the folder, its « … » menu opens the
// renders in a mosaic or deletes the folder.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import type { LivePage, StepArtifactFolder, StepArtifactLink } from '../../types/generated';
import { visibleRunFolders } from '../../lib/stepArtifacts';
import { ArtifactRunFolders } from '../ArtifactRunFolders';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);

const page = (id: string, extra: Partial<LivePage> = {}): LivePage => ({
  id, project_id: null, title: `Jeux · jeu · ${id}`, slug: id, current_revision_id: `rev-${id}`, data_revision: 0,
  created_at: '2026-11-21T00:00:00Z', updated_at: '2026-11-21T00:00:00Z', last_published_at: null,
  pinned: false, archived: false, html_size_bytes: 10, ...extra,
});
const link = (pageId: string, shadowId: string, model?: string): StepArtifactLink => ({
  page_id: pageId, run_id: 'run-1', result_index: 0, step_name: 'jeu', shadow_id: shadowId,
  label: shadowId || 'main', model, created_at: '2026-11-21T00:00:00Z',
});
const folder: StepArtifactFolder = {
  run_id: 'run-1', workflow_id: 'wf', workflow_name: 'Jeux', run_number: 3, run_started_at: '2026-11-21T00:00:00Z',
  artifacts: [link('p-main', '', 'opus'), link('p-qwen', 'qwen', 'qwen3'), link('p-argon', 'argon', 'argon')],
};
const pages = new Map(['p-main', 'p-qwen', 'p-argon'].map(id => [id, page(id)]));

function renderFolders(onDeleteFolder = vi.fn(async () => true), selectedId: string | null = null, searching?: boolean) {
  const views = visibleRunFolders([folder], pages, () => true);
  render(
    <ArtifactRunFolders
      views={views}
      selectedId={selectedId}
      renderRow={({ link: l, page: p }) => <div key={p.id} data-testid={`row-${p.id}`}>{l.label} · {l.model}</div>}
      onDeleteFolder={onDeleteFolder}
      collapsed={false}
      onToggleSection={() => {}}
      searching={searching}
      t={t}
    />,
  );
  return { onDeleteFolder };
}

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('ArtifactRunFolders', () => {
  it('shows one folder per run, named after its workflow and run number, closed until asked', () => {
    renderFolders();
    const node = screen.getByTestId('artifact-run-folder-run-1');
    expect(within(node).getByText('pages.runFolders.folder:Jeux,3')).toBeTruthy();
    expect(screen.queryByTestId('row-p-main')).toBeNull();
    fireEvent.click(within(node).getByText('pages.runFolders.folder:Jeux,3'));
    // Labelled by execution and model, main first then the shadows in start order.
    expect(screen.getAllByTestId(/^row-/).map(row => row.textContent)).toEqual(['main · opus', 'qwen · qwen3', 'argon · argon']);
  });

  it('opens by itself while it holds the Artifact shown', () => {
    renderFolders(undefined, 'p-qwen');
    expect(screen.getByTestId('row-p-qwen')).toBeTruthy();
  });

  it('opens every folder while the library is searched', () => {
    renderFolders(undefined, null, true);
    expect(screen.getAllByTestId(/^row-/).map(row => row.textContent)).toEqual(['main · opus', 'qwen · qwen3', 'argon · argon']);
  });

  it('opens the folder in a mosaic from its menu, in its order', () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    renderFolders();
    fireEvent.click(screen.getByTestId('artifact-run-folder-menu-run-1'));
    fireEvent.click(screen.getByTestId('artifact-run-folder-mosaic-run-1'));
    const [url, target, features] = open.mock.calls[0];
    expect(String(url)).toContain('/standalone/pages/mosaic?');
    expect(new URLSearchParams(String(url).split('?')[1]).getAll('page')).toEqual(['p-main', 'p-qwen', 'p-argon']);
    expect(target).toBe('_blank');
    expect(features).toBe('noopener,noreferrer');
    expect(screen.queryByTestId('artifact-run-folder-mosaic-run-1')).toBeNull();
  });

  it('deletes the folder from its menu', () => {
    const { onDeleteFolder } = renderFolders();
    fireEvent.click(screen.getByTestId('artifact-run-folder-menu-run-1'));
    fireEvent.click(screen.getByTestId('artifact-run-folder-delete-run-1'));
    expect(onDeleteFolder).toHaveBeenCalledWith(folder);
  });

  it('closes its menu on Escape', () => {
    renderFolders();
    fireEvent.click(screen.getByTestId('artifact-run-folder-menu-run-1'));
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByTestId('artifact-run-folder-delete-run-1')).toBeNull();
  });

  it('counts on its header the rows it lists, not every Artifact of the run', () => {
    const shown = new Map(pages);
    shown.set('p-argon', page('p-argon', { archived: true }));
    const views = visibleRunFolders([folder], shown, p => p.id !== 'p-qwen');
    render(
      <ArtifactRunFolders
        views={views}
        selectedId={null}
        renderRow={({ page: p }) => <div key={p.id} data-testid={`row-${p.id}`} />}
        onDeleteFolder={vi.fn(async () => true)}
        collapsed={false}
        onToggleSection={() => {}}
        t={t}
      />,
    );
    const header = screen.getByTestId('artifact-run-folder-run-1').querySelector('.disc-batch-header')!;
    expect(header.querySelector('.disc-group-count')?.textContent).toBe('1');
    fireEvent.click(within(header as HTMLElement).getByText('pages.runFolders.folder:Jeux,3'));
    expect(screen.getAllByTestId(/^row-/)).toHaveLength(1);
  });

  it('keeps only the Artifacts the library holds and the search shows', () => {
    const fewer = new Map(pages);
    fewer.delete('p-argon');
    fewer.set('p-qwen', page('p-qwen', { archived: true }));
    const [view] = visibleRunFolders([folder], fewer, () => true);
    expect(view.items.map(item => item.page.id)).toEqual(['p-main']);
    expect(visibleRunFolders([folder], pages, p => p.id === 'nothing')).toEqual([]);
  });
});
