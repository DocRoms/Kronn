// KT-1169 — a recap that could not be read says so, with a button to read it again.
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import AuditRecapPanel from '../AuditRecapPanel';

const run = {
  id: 'run-1', project_id: 'p1', kind: 'Full', agent_type: 'ClaudeCode',
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:20:00Z', duration_ms: 1_200_000,
  status: 'Interrupted', td_total: 0, health_score: 90,
};

describe('AuditRecapPanel — read errors', () => {
  it('a failed history read is shown with "Réessayer", which reads it again', async () => {
    vi.mocked(projectsApi.auditHistory)
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce([run] as never);
    vi.mocked(projectsApi.auditRunSteps).mockResolvedValue([] as never);
    render(<I18nProvider><AuditRecapPanel projectId="p1" refreshTrigger={0} selectedRunId={null} /></I18nProvider>);

    const error = await screen.findByTestId('audit-recap-error');
    expect(error).toHaveTextContent('offline');
    fireEvent.click(screen.getByTestId('audit-recap-retry'));
    await waitFor(() => expect(screen.queryByTestId('audit-recap-error')).toBeNull());
    expect(projectsApi.auditHistory).toHaveBeenCalledTimes(2);
  });

  it('a failed read of the latest run\'s steps is shown with "Réessayer" too', async () => {
    vi.mocked(projectsApi.auditHistory).mockResolvedValue([run] as never);
    vi.mocked(projectsApi.auditRunSteps)
      .mockRejectedValueOnce(new Error('steps unavailable'))
      .mockResolvedValueOnce([] as never);
    render(<I18nProvider><AuditRecapPanel projectId="p1" refreshTrigger={0} selectedRunId={null} /></I18nProvider>);

    expect(await screen.findByTestId('audit-recap-error')).toHaveTextContent('steps unavailable');
    fireEvent.click(screen.getByTestId('audit-recap-retry'));
    await waitFor(() => expect(screen.queryByTestId('audit-recap-error')).toBeNull());
    expect(projectsApi.auditRunSteps).toHaveBeenCalledTimes(2);
  });
});
