// KT-1131 — the recap tells succeeded, unchanged, accepted and failed steps apart.
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import AuditRecapPanel from '../AuditRecapPanel';
import { auditStepOutcome } from '../../lib/audit-step-outcome';
import fr from '../../lib/i18n/locales/fr';

const run = {
  id: 'run-1', project_id: 'p1', kind: 'Full', agent_type: 'ClaudeCode',
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:20:00Z', duration_ms: 1_200_000,
  status: 'Completed', td_total: 0, health_score: 90,
};

const step = (index: number, extra: Record<string, unknown>) => ({
  audit_run_id: 'run-1', step_index: index, file_label: `docs/step-${index}.md`,
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:01:00Z', duration_ms: 1000,
  step_tokens: 10, cumulative_tokens: 10, cli_success: true, step_warning: null,
  step_repaired_from_template: false, ...extra,
});

describe('AuditRecapPanel — step outcomes', () => {
  it('classifies each recorded step', () => {
    expect(auditStepOutcome({ cli_success: true })).toBe('succeeded');
    expect(auditStepOutcome({ cli_success: true, unchanged: true })).toBe('unchanged');
    expect(auditStepOutcome({ cli_success: false, step_warning: 'x' })).toBe('failed');
    expect(auditStepOutcome({ cli_success: false, step_warning: 'x', accepted_at: '2026-10-02T11:00:00Z' })).toBe('accepted');
  });

  it('labels unchanged and accepted rows, and never marks an accepted one failed', async () => {
    vi.mocked(projectsApi.auditHistory).mockResolvedValueOnce([run] as never);
    vi.mocked(projectsApi.auditRunSteps).mockResolvedValueOnce([
      step(1, {}),
      step(2, { unchanged: true }),
      step(3, { cli_success: false, step_warning: 'placeholders remain', accepted_at: '2026-10-02T11:00:00Z', accepted_by: 'romu', accepted_reason: 'kept' }),
      step(4, { cli_success: false, step_warning: 'placeholders remain' }),
    ] as never);
    render(<I18nProvider><AuditRecapPanel projectId="p1" /></I18nProvider>);

    const accepted = await screen.findByTestId('audit-recap-latest-row-3');
    expect(accepted).toHaveAttribute('data-outcome', 'accepted');
    expect(accepted).toHaveAttribute('data-failed', 'false');
    expect(accepted).toHaveTextContent(fr['projects.docAi.auditRecap.outcome.accepted']);
    const unchanged = screen.getByTestId('audit-recap-latest-row-2');
    expect(unchanged).toHaveAttribute('data-outcome', 'unchanged');
    expect(unchanged).toHaveTextContent(fr['projects.docAi.auditRecap.outcome.unchanged']);
    expect(screen.getByTestId('audit-recap-latest-row-1')).toHaveAttribute('data-outcome', 'succeeded');
    expect(screen.getByTestId('audit-recap-latest-row-4')).toHaveAttribute('data-failed', 'true');
  });
});
