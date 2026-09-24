import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ArtifactImportPreview, ArtifactImportResult } from '../../types/generated';
import { ArtifactImportDialog } from '../ArtifactImportDialog';

const mocks = vi.hoisted(() => ({ preview: vi.fn(), commit: vi.fn(), projects: vi.fn() }));
vi.mock('../../lib/api', () => ({ pages: { previewImport: mocks.preview, importArtifact: mocks.commit }, projects: { list: mocks.projects } }));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string, ...args: unknown[]) => args.length ? `${key}:${args.join(',')}` : key }) }));

const preview: ArtifactImportPreview = {
  title: 'Équipe 🦀', entries: [
    { kind: 'artifact', source_id: 'a', name: 'Équipe 🦀', disposition: 'create', existing_id: 'a', reason: 'root' },
    { kind: 'quick_api', source_id: 'qa', name: 'Metrics', disposition: 'reuse', existing_id: 'qa', reason: 'identical' },
  ], issues: [], warnings: [], digest: 'digest-1', can_import: true,
};
const result: ArtifactImportResult = { artifact: {
  id: 'new-a', title: 'Équipe 🦀', slug: 'new-a', project_id: null, current_revision_id: 'new-r', data_revision: 0,
  created_at: '2026-09-23T00:00:00Z', updated_at: '2026-09-23T00:00:00Z', last_published_at: null, pinned: false, archived: false,
}, entries: preview.entries };
const content = JSON.stringify({ kind: 'kronn.artifact', version: 1 });
const chooseFile = async (text = content, size?: number) => {
  const file = new File([text], 'team.kronn-artifact.json', { type: 'application/json' });
  if (size !== undefined) Object.defineProperty(file, 'size', { value: size });
  fireEvent.change(screen.getByLabelText('pages.import.file'), { target: { files: [file] } });
  await waitFor(() => expect(screen.getByLabelText('pages.import.file')).not.toBeDisabled());
};
const inspect = async () => {
  await chooseFile();
  fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
  await screen.findByRole('button', { name: 'pages.import.confirm' });
};

beforeEach(() => { vi.clearAllMocks(); mocks.projects.mockResolvedValue([]); mocks.preview.mockResolvedValue(preview); mocks.commit.mockResolvedValue(result); });
afterEach(cleanup);

describe('ArtifactImportDialog', () => {
  it('lists the secrets the export masked before the import is confirmed', async () => {
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={vi.fn()} />);
    await chooseFile(JSON.stringify({ kind: 'kronn.artifact', version: 1, redacted_fields: [
      { kind: 'quick_exec', resource_id: 'qe', name: 'Collector', field: 'args.1' },
    ] }));
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
    const note = await screen.findByTestId('import-redacted-fields');
    expect(note).toHaveTextContent('imp.redactedTitle');
    expect(note).toHaveTextContent('imp.redactedKind.quick_exec « Collector » : args.1');
    expect(mocks.commit).not.toHaveBeenCalled();
  });

  it('shows exact commands and requests approval for every new Quick Exec before importing', async () => {
    const entries: ArtifactImportPreview['entries'] = [
      preview.entries[0],
      { kind: 'quick_exec', source_id: 'qe', name: 'Collector', disposition: 'create', existing_id: null, reason: 'missing',
        quick_exec: { command: 'bash', args: ['-c', 'printf "%s" "Équipe 🦀"'], approved: false } },
      { kind: 'quick_api', source_id: 'qa', name: 'Metrics', disposition: 'create', existing_id: null, reason: 'missing',
        quick_api: { method: 'POST', endpoint: '/tickets', plugin: 'jira' } },
    ];
    mocks.preview.mockImplementation(async request => ({
      ...preview, digest: request.approved_quick_exec_ids.length ? 'approved-digest' : 'unapproved-digest',
      can_import: request.approved_quick_exec_ids.includes('qe'),
      entries: entries.map(entry => entry.quick_exec ? { ...entry, quick_exec: { ...entry.quick_exec, approved: request.approved_quick_exec_ids.includes('qe') } } : entry),
    }));
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={vi.fn()} />);
    await inspect();
    expect(document.querySelector('pre')?.textContent).toBe(JSON.stringify({
      command: 'bash', args: ['-c', 'printf "%s" "Équipe 🦀"'],
    }, null, 2));
    expect(screen.getByText('POST /tickets')).toBeInTheDocument();
    expect(screen.getByText('jira')).toBeInTheDocument();
    const approve = screen.getByRole('checkbox', { name: 'pages.import.approveExec:Collector' });
    expect(approve).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeDisabled();
    fireEvent.click(approve);
    expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeEnabled());
    expect(mocks.preview).toHaveBeenLastCalledWith(expect.objectContaining({ approved_quick_exec_ids: ['qe'] }));
    fireEvent.click(approve);
    expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeDisabled();
    expect(mocks.commit).not.toHaveBeenCalled();
    fireEvent.click(approve);
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.confirm' }));
    await waitFor(() => expect(mocks.commit).toHaveBeenCalledWith(expect.objectContaining({
      approved_quick_exec_ids: ['qe'], preview_digest: 'approved-digest',
    })));
    // A new file requires fresh approvals, including one with the same source IDs.
    await chooseFile();
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
    expect(await screen.findByRole('checkbox', { name: 'pages.import.approveExec:Collector' })).not.toBeChecked();
    expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeDisabled();
  });

  it('previews without importing, then commits once with the reviewed digest despite synchronous clicks', async () => {
    const imported = vi.fn();
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={imported} />);
    await inspect();
    expect(mocks.commit).not.toHaveBeenCalled();
    expect(screen.getByText('pages.import.reason.identical')).toBeInTheDocument();
    const button = screen.getByRole('button', { name: 'pages.import.confirm' });
    act(() => { button.click(); button.click(); });
    await waitFor(() => expect(imported).toHaveBeenCalledWith(result.artifact));
    expect(mocks.commit).toHaveBeenCalledTimes(1);
    expect(mocks.commit).toHaveBeenCalledWith({ content, project_id: null, choices: [], approved_quick_exec_ids: [], preview_digest: 'digest-1' });
  });

  it('requires a new preview after changing a dependency choice', async () => {
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={vi.fn()} />);
    await inspect();
    fireEvent.change(screen.getByRole('combobox', { name: 'pages.import.choice:Metrics' }), { target: { value: 'create' } });
    expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
    await waitFor(() => expect(mocks.preview).toHaveBeenCalledTimes(2));
    expect(mocks.preview).toHaveBeenLastCalledWith(expect.objectContaining({ choices: [{ kind: 'quick_api', source_id: 'qa', action: 'create', target_id: null }] }));
  });

  it('shows missing local configuration before allowing import and setup later', async () => {
    mocks.preview.mockResolvedValue({ ...preview, warnings: [{ kind: 'plugin_connection', id: 'jira' }] });
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={vi.fn()} />);
    await chooseFile();
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
    expect(await screen.findByText('pages.import.warning.plugin_connection:jira')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'pages.import.confirmWithSetup' })).toBeEnabled();
    expect(mocks.commit).not.toHaveBeenCalled();
  });

  it.each([
    ['wrong kind', JSON.stringify({ kind: 'kronn.workflow', version: 1 }), undefined, 'pages.import.invalidFile'],
    ['future version', JSON.stringify({ kind: 'kronn.artifact', version: 2 }), undefined, 'pages.import.invalidFile'],
    ['oversized file', content, 16 * 1024 * 1024 + 1, 'pages.import.tooLarge'],
  ])('rejects %s before a backend preview', async (_case, text, size, error) => {
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={vi.fn()} />);
    await chooseFile(text, size);
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
    expect(mocks.preview).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'pages.import.preview' })).toBeDisabled();
  });

  it('requires a new review after a stale import error and never reports success', async () => {
    mocks.commit.mockRejectedValue(new Error('stale preview'));
    const imported = vi.fn();
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={imported} />);
    await inspect();
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.confirm' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('stale preview');
    expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeDisabled();
    expect(imported).not.toHaveBeenCalled();
  });

  it('keeps conflicting definitions blocked until an explicit choice is reviewed', async () => {
    mocks.preview.mockResolvedValue({ ...preview, can_import: false,
      entries: [preview.entries[0], { ...preview.entries[1], disposition: 'conflict', reason: 'changed' }] });
    render(<ArtifactImportDialog onClose={vi.fn()} onImported={vi.fn()} />);
    await inspect();
    expect(screen.getByText('pages.import.reason.changed')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'pages.import.confirm' })).toBeDisabled();
    fireEvent.change(screen.getByRole('combobox', { name: 'pages.import.choice:Metrics' }), { target: { value: 'reuse' } });
    fireEvent.click(screen.getByRole('button', { name: 'pages.import.preview' }));
    await waitFor(() => expect(mocks.preview).toHaveBeenCalledTimes(2));
    expect(mocks.preview).toHaveBeenLastCalledWith(expect.objectContaining({ choices: [{ kind: 'quick_api', source_id: 'qa', action: 'reuse', target_id: 'qa' }] }));
  });
});
