// KT-1084 — the recap shows what the run consumed per phase and in total.
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import AuditRecapPanel from '../AuditRecapPanel';
import fr from '../../lib/i18n/locales/fr';
import type { AuditPhaseCost, AuditRunCost } from '../../types/generated';

const run = {
  id: 'run-1', project_id: 'p1', kind: 'Full', agent_type: 'ClaudeCode',
  started_at: '2026-10-02T10:00:00Z', ended_at: '2026-10-02T10:20:00Z', duration_ms: 1_200_000,
  status: 'Completed', td_total: 0, health_score: 90,
};

const none: AuditPhaseCost = {
  items: 0, measured_items: 0, split_unknown_items: 0, cost_estimated: false,
  cost_unknown_items: 0, carried_items: 0,
};
const measured = (extra: Partial<AuditPhaseCost>): AuditPhaseCost => ({
  ...none, items: 2, measured_items: 2, ...extra,
});

async function renderWith(cost: AuditRunCost | null) {
  vi.mocked(projectsApi.auditHistory).mockResolvedValueOnce([run] as never);
  vi.mocked(projectsApi.auditRunCost).mockResolvedValueOnce(cost as never);
  render(<I18nProvider><AuditRecapPanel projectId="p1" /></I18nProvider>);
  await screen.findByTestId('audit-recap-latest');
}

describe('AuditRecapPanel — cost per phase', () => {
  it('shows each phase and the grand total', async () => {
    await renderWith({
      run_id: 'run-1',
      briefing: measured({ total_tokens: 3_000, cost_usd_micros: 100_000, cost_estimated: true }),
      audit: measured({ total_tokens: 1_000_000, cost_usd_micros: 2_500_000, carried_items: 1 }),
      validation: measured({ total_tokens: 11_000, cost_usd_micros: 750_000, cost_estimated: true }),
      total: measured({ total_tokens: 1_014_000, cost_usd_micros: 3_350_000, cost_estimated: true }),
    });
    expect(await screen.findByTestId('audit-recap-cost-briefing-tokens')).toHaveTextContent('3.0k');
    expect(screen.getByTestId('audit-recap-cost-audit-tokens')).toHaveTextContent('1.00M');
    // Reported: no mark. Estimated: the ≈ mark.
    expect(screen.getByTestId('audit-recap-cost-audit-usd')).toHaveTextContent(/^~2[.,]50 \$$/);
    expect(screen.getByTestId('audit-recap-cost-briefing-usd')).toHaveTextContent(/^≈0[.,]10 \$$/);
    expect(screen.getByTestId('audit-recap-cost-validation-usd')).toHaveTextContent(/^≈0[.,]75 \$$/);
    expect(screen.getByTestId('audit-recap-cost-total-tokens')).toHaveTextContent('1.01M');
    expect(screen.getByTestId('audit-recap-cost-total-usd')).toHaveTextContent(/^≈3[.,]35 \$$/);
    expect(screen.getByTestId('audit-recap-cost-total')).toHaveAttribute('data-total', 'true');
  });

  it('says not measured, never 0, for a phase no agent reported', async () => {
    await renderWith({
      run_id: 'run-1',
      briefing: measured({ measured_items: 0, cost_unknown_items: 2 }),
      audit: measured({ total_tokens: 500, cost_usd_micros: 40_000 }),
      validation: none,
      total: measured({ total_tokens: 500, cost_usd_micros: 40_000, cost_unknown_items: 2 }),
    });
    const notMeasured = fr['projects.docAi.auditRecap.cost.notMeasured'];
    expect(await screen.findByTestId('audit-recap-cost-briefing-tokens')).toHaveTextContent(notMeasured);
    expect(screen.getByTestId('audit-recap-cost-briefing-usd')).toHaveTextContent(notMeasured);
    expect(screen.getByTestId('audit-recap-cost-briefing-usd')).not.toHaveTextContent('0');
    // A phase that did not happen is neither measured nor free: a dash.
    expect(screen.getByTestId('audit-recap-cost-validation-usd')).toHaveTextContent('—');
    // The total over an unknown cost is a floor.
    expect(screen.getByTestId('audit-recap-cost-total-usd')).toHaveTextContent(/^≥ 0[.,]04 \$$/);
  });

  it('shows tokens without a cost when only the cost is unknown', async () => {
    await renderWith({
      run_id: 'run-1',
      briefing: none,
      audit: measured({ total_tokens: 500, measured_items: 1, cost_unknown_items: 2 }),
      validation: none,
      total: measured({ total_tokens: 500, measured_items: 1, cost_unknown_items: 2 }),
    });
    expect(await screen.findByTestId('audit-recap-cost-audit-tokens')).toHaveTextContent('≥ 500');
    expect(screen.getByTestId('audit-recap-cost-audit-usd'))
      .toHaveTextContent(fr['projects.docAi.auditRecap.cost.notMeasured']);
  });

  it('renders no phase table when the cost is unavailable', async () => {
    await renderWith(null);
    expect(screen.queryByTestId('audit-recap-cost')).toBeNull();
  });
});
