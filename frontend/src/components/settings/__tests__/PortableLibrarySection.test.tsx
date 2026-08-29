import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { PortableLibrarySection } from '../PortableLibrarySection';
import { portableLibrary } from '../../../lib/api';

vi.mock('../../../lib/api', () => ({ portableLibrary: {
  state: vi.fn().mockResolvedValue({ scope: 'project', project_id: 'p1', drift: 'drifted', approved: false, items: [{ kind: 'skill', id: 'review', scope: 'global', source: 'skills/review/SKILL.md', content_sha256: 'abc', content: 'body' }] }),
  sync: vi.fn().mockResolvedValue({}), check: vi.fn().mockResolvedValue({}), approve: vi.fn().mockResolvedValue(true),
  migrate: vi.fn().mockResolvedValue({ created: [], unchanged: [] }), export: vi.fn().mockResolvedValue([]), import: vi.fn().mockResolvedValue({}),
} }));
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key, locale: 'en-US' }),
}));

describe('PortableLibrarySection', () => {
  it('selects a DB project, exposes provenance and runs project operations', async () => {
    render(<PortableLibrarySection projects={[{ id: 'p1', name: 'Project One' } as never]} toast={vi.fn()} />);
    expect(screen.getByText('config.portableLibrary.whatTitle')).toBeInTheDocument();
    expect(screen.getByLabelText('config.portableLibrary.flowAria')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'config.portableLibrary.actionSync' })).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('config.portableLibrary.scopeAria'), { target: { value: 'p1' } });
    await screen.findByText('review');
    expect(screen.getByText('skills/review/SKILL.md')).toBeInTheDocument();
    expect(screen.getByText('config.portableLibrary.status.drifted')).toBeInTheDocument();
    expect(screen.getByText('config.portableLibrary.kind.skill')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'config.portableLibrary.actionApprove' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'config.portableLibrary.actionSync' }));
    await waitFor(() => expect(portableLibrary.sync).toHaveBeenCalledWith('p1'));
  });

  it('surfaces backend failures visibly', async () => {
    vi.mocked(portableLibrary.state).mockRejectedValueOnce(new Error('project path is unavailable'));
    render(<PortableLibrarySection projects={[]} toast={vi.fn()} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('project path is unavailable');
  });

  it('ignores a synchronous double-click and fires only one request', async () => {
    render(<PortableLibrarySection projects={[{ id: 'p1', name: 'Project One' } as never]} toast={vi.fn()} />);
    fireEvent.change(screen.getByLabelText('config.portableLibrary.scopeAria'), { target: { value: 'p1' } });
    await screen.findByText('review');
    vi.mocked(portableLibrary.sync).mockClear();
    const syncButton = screen.getByRole('button', { name: 'config.portableLibrary.actionSync' });
    fireEvent.click(syncButton);
    fireEvent.click(syncButton);
    await waitFor(() => expect(portableLibrary.sync).toHaveBeenCalledTimes(1));
  });

  it('imports in global scope without a carrier project (no project_id sent)', async () => {
    render(<PortableLibrarySection projects={[{ id: 'p1', name: 'Project One' } as never]} toast={vi.fn()} />);
    await screen.findByText('review');
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    expect(input).not.toBeDisabled();
    const file = new File([JSON.stringify({ items: [{ kind: 'skill', id: 'x', scope: 'global', content: 'body' }] })], 'lib.json', { type: 'application/json' });
    fireEvent.change(input, { target: { files: [file] } });
    await waitFor(() => expect(portableLibrary.import).toHaveBeenCalledWith(undefined, [{ kind: 'skill', id: 'x', scope: 'global', content: 'body' }]));
  });

  it('imports in project scope with the selected project_id', async () => {
    render(<PortableLibrarySection projects={[{ id: 'p1', name: 'Project One' } as never]} toast={vi.fn()} />);
    fireEvent.change(screen.getByLabelText('config.portableLibrary.scopeAria'), { target: { value: 'p1' } });
    await screen.findByText('review');
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File([JSON.stringify({ items: [{ kind: 'skill', id: 'y', scope: 'project', content: 'body' }] })], 'lib.json', { type: 'application/json' });
    fireEvent.change(input, { target: { files: [file] } });
    await waitFor(() => expect(portableLibrary.import).toHaveBeenCalledWith('p1', [{ kind: 'skill', id: 'y', scope: 'project', content: 'body' }]));
  });
});
