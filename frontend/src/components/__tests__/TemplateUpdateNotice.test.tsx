import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { TemplateUpdateNotice } from '../TemplateUpdateNotice';
import { projects } from '../../lib/api';
import type { TemplateUpdate } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});
const update: TemplateUpdate = {
  available: true, installed_version: '1.0', current_version: '1.1',
  missing_files: [{ path: 'docs/repo-map.md', step: 3 }, { path: 'docs/workflow/tickets.md', step: null }],
  renames: [{ from: 'docs/old.md', to: 'docs/new.md', conflict: false }],
  unresolved_files: [], spec_modified: false,
};
const api = vi.mocked(projects);
const setup = (value = update, onRunSteps = vi.fn()) => {
  render(<I18nProvider><TemplateUpdateNotice projectId="p" update={value} onRunSteps={onRunSteps} /></I18nProvider>);
  return onRunSteps;
};
beforeEach(() => {
  vi.clearAllMocks();
  api.installTemplate.mockResolvedValue('Audited');
  api.checkDrift.mockResolvedValue({ audit_date: null, stale_sections: [], fresh_sections: [], total_sections: 0, sources_state: 'unknown', broken_citations: 0, placeholder_docs: [{ path: 'docs/repo-map.md', step: 3 }, { path: 'docs/glossary.md', step: 2 }] });
});
describe('TemplateUpdateNotice', () => {
  it('shows the real missing and renamed paths on an already audited project', () => {
    setup();
    expect(screen.getByText('docs/repo-map.md')).toBeTruthy();
    expect(screen.getByText('docs/workflow/tickets.md')).toBeTruthy();
    expect(screen.getByText('docs/old.md')).toBeTruthy();
    expect(screen.getByText('docs/new.md')).toBeTruthy();
    expect(api.installTemplate).not.toHaveBeenCalled();
  });
  it('installs additions once before offering only their owning steps', async () => {
    const onRunSteps = setup();
    const button = screen.getByTestId('template-update-install');
    act(() => { button.click(); button.click(); });
    await screen.findByTestId('project-placeholder-fill');
    expect(api.installTemplate).toHaveBeenCalledTimes(1);
    expect(api.installTemplate).toHaveBeenCalledWith('p');
    expect(onRunSteps).not.toHaveBeenCalled();
    expect(screen.queryByTestId('template-update-unowned')).toBeNull();
    fireEvent.click(screen.getByTestId('project-placeholder-fill'));
    expect(onRunSteps).toHaveBeenCalledWith([3]);
  });
  it('lists only remaining unowned placeholders for manual completion', async () => {
    api.checkDrift.mockResolvedValue({ audit_date: null, stale_sections: [], fresh_sections: [], total_sections: 0, sources_state: 'unknown', broken_citations: 0, placeholder_docs: [{ path: 'docs/workflow/tickets.md', step: null }] });
    setup();
    fireEvent.click(screen.getByTestId('template-update-install'));
    const manual = await screen.findByTestId('template-update-unowned');
    expect(within(manual).getByText('docs/workflow/tickets.md')).toBeTruthy();
    expect(within(manual).queryByText('docs/repo-map.md')).toBeNull();
    expect(screen.queryByTestId('project-placeholder-fill')).toBeNull();
  });
  it('keeps failures visible and retryable, without launching an audit', async () => {
    api.installTemplate.mockRejectedValueOnce(new Error('permission denied'));
    const onRunSteps = setup();
    fireEvent.click(screen.getByTestId('template-update-install'));
    expect(await screen.findByRole('alert')).toHaveTextContent('permission denied');
    expect(onRunSteps).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId('template-update-install'));
    await waitFor(() => expect(api.installTemplate).toHaveBeenCalledTimes(2));
  });
  it('hides a current template and reports conflicts without proposing replacements', () => {
    const { unmount } = render(<I18nProvider><TemplateUpdateNotice projectId="p" update={{ ...update, available: false }} onRunSteps={vi.fn()} /></I18nProvider>);
    expect(screen.queryByTestId('template-update-notice')).toBeNull();
    unmount();
    setup({ ...update, renames: [{ ...update.renames[0], conflict: true }], unresolved_files: ['docs/unknown.md'], spec_modified: true });
    expect(screen.getByTestId('template-update-conflict')).toBeTruthy();
    expect(screen.getByText('docs/unknown.md')).toBeTruthy();
    expect(screen.getByTestId('template-update-spec-modified')).toBeTruthy();
  });
});
