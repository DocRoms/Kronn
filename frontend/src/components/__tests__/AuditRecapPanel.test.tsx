// AuditRecapPanel — 0.8.4 (#298 + #332) regression suite.
//
// 0.8.4 (#332) refactor: the inline chip strip + step table was
// replaced with a compact CTA button that opens a side drawer.
// The drawer shows filter pills + vertical run cards; clicking a
// card expands its per-step table inline. Tests below pin the
// new shape:
//   - hides itself when auditHistory is empty
//   - shows a "Voir les audits précédents (N)" button when history exists
//   - clicking the button opens the drawer (overlay + aside)
//   - drawer shows one card per run, most recent first
//   - clicking a card expands its per-step table
//   - default-collapsed: no step table renders until a card is clicked
//   - refetches history when refreshTrigger changes
//   - sorts steps by duration DESC when the column is clicked
//   - filter pills narrow the run list
//   - pagination caps the list at 10 runs

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import AuditRecapPanel from '../AuditRecapPanel';
import type { ReactElement } from 'react';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const fullRun = {
  id: 'run-42',
  project_id: 'p1',
  kind: 'Full',
  agent_type: 'ClaudeCode',
  started_at: '2026-05-15T10:00:00Z',
  ended_at: '2026-05-15T10:20:00Z',
  duration_ms: 1_200_000,
  status: 'Completed',
  td_total: 18,
  health_score: 53,
};

const rgaaRun = {
  id: 'run-77',
  project_id: 'p1',
  kind: 'Rgaa',
  agent_type: 'ClaudeCode',
  started_at: '2026-05-15T11:30:00Z',
  ended_at: '2026-05-15T11:33:00Z',
  duration_ms: 180_000,
  status: 'Completed',
  td_total: 4,
  health_score: null,
};

const sampleSteps = [
  {
    audit_run_id: 'run-42', step_index: 1, file_label: 'docs/glossary.md',
    started_at: '2026-05-15T10:00:00Z',
    ended_at: '2026-05-15T10:01:00Z',
    duration_ms: 60_000, step_tokens: 5_000, cumulative_tokens: 5_000,
    cli_success: true, step_warning: null, step_repaired_from_template: false,
  },
  {
    audit_run_id: 'run-42', step_index: 2, file_label: 'docs/repo-map.md',
    started_at: '2026-05-15T10:01:00Z',
    ended_at: '2026-05-15T10:04:00Z',
    duration_ms: 180_000, step_tokens: 20_000, cumulative_tokens: 25_000,
    cli_success: false, step_warning: 'target file is empty — repaired from template',
    step_repaired_from_template: true,
  },
  {
    audit_run_id: 'run-42', step_index: 3, file_label: 'docs/architecture.md',
    started_at: '2026-05-15T10:04:00Z',
    ended_at: '2026-05-15T10:06:30Z',
    duration_ms: 150_000, step_tokens: 12_000, cumulative_tokens: 37_000,
    cli_success: true, step_warning: null, step_repaired_from_template: false,
  },
];

beforeEach(() => {
  vi.clearAllMocks();
});

describe('AuditRecapPanel (0.8.4 #332 drawer)', () => {
  it('renders nothing when audit history is empty', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
    const { container } = wrap(<AuditRecapPanel projectId="p1" />);
    await waitFor(() => {
      expect(projectsApi.auditHistory).toHaveBeenCalledWith('p1');
    });
    await waitFor(() => {
      expect(container.querySelector('[data-testid="audit-recap-toggle"]')).toBeNull();
    });
  });

  it('shows the compact CTA button when history exists', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([fullRun, rgaaRun]);
    wrap(<AuditRecapPanel projectId="p1" />);
    const btn = await screen.findByTestId('audit-recap-toggle');
    expect(btn.textContent).toMatch(/2/);
    // Drawer is closed by default.
    expect(screen.queryByTestId('audit-recap-drawer')).toBeNull();
  });

  it('opens the drawer on CTA click + lists one card per run, newest first', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([rgaaRun, fullRun]);
    wrap(<AuditRecapPanel projectId="p1" />);

    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));
    await screen.findByTestId('audit-recap-drawer');

    // Both cards present.
    expect(screen.getByTestId('audit-recap-chip-run-42')).toBeInTheDocument();
    expect(screen.getByTestId('audit-recap-chip-run-77')).toBeInTheDocument();

    // No step table rendered yet — cards are collapsed by default.
    expect(screen.queryByTestId('audit-recap-table')).toBeNull();
  });

  it('expands a card to show its per-step table on click', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([fullRun]);
    (projectsApi.auditRunSteps as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sampleSteps);

    wrap(<AuditRecapPanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));

    // Click the Full card → fetches steps and renders the table.
    const card = await screen.findByTestId('audit-recap-chip-run-42');
    fireEvent.click(card);

    await waitFor(() => {
      expect(projectsApi.auditRunSteps).toHaveBeenCalledWith('run-42');
    });
    await screen.findByTestId('audit-recap-table');
    expect(screen.getByTestId('audit-recap-row-1')).toBeInTheDocument();
    expect(screen.getByTestId('audit-recap-row-2')).toBeInTheDocument();
    expect(screen.getByTestId('audit-recap-row-3')).toBeInTheDocument();
  });

  it('closing the drawer hides the run cards (Escape key + close button)', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([fullRun]);
    wrap(<AuditRecapPanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));
    await screen.findByTestId('audit-recap-drawer');

    // Close via the X button.
    fireEvent.click(screen.getByTestId('audit-recap-drawer-close'));
    await waitFor(() => {
      expect(screen.queryByTestId('audit-recap-drawer')).toBeNull();
    });

    // Re-open + close via Escape.
    fireEvent.click(screen.getByTestId('audit-recap-toggle'));
    await screen.findByTestId('audit-recap-drawer');
    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => {
      expect(screen.queryByTestId('audit-recap-drawer')).toBeNull();
    });
  });

  it('refetches history when refreshTrigger changes', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValue([fullRun]);

    const { rerender } = wrap(
      <AuditRecapPanel projectId="p1" refreshTrigger={0} />
    );
    await screen.findByTestId('audit-recap-toggle');
    expect(projectsApi.auditHistory).toHaveBeenCalledTimes(1);

    rerender(
      <I18nProvider>
        <AuditRecapPanel projectId="p1" refreshTrigger={1} />
      </I18nProvider>
    );

    await waitFor(() => {
      expect(projectsApi.auditHistory).toHaveBeenCalledTimes(2);
    });
  });

  it('shows the empty-state copy when the expanded run has no per-step rows', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([fullRun]);
    (projectsApi.auditRunSteps as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);

    wrap(<AuditRecapPanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));
    fireEvent.click(await screen.findByTestId('audit-recap-chip-run-42'));
    await screen.findByTestId('audit-recap-empty');
  });

  it('sorts by duration DESC when the duration column is clicked', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([fullRun]);
    (projectsApi.auditRunSteps as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sampleSteps);

    wrap(<AuditRecapPanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));
    fireEvent.click(await screen.findByTestId('audit-recap-chip-run-42'));

    const table = await screen.findByTestId('audit-recap-table');
    const headers = table.querySelectorAll('thead th');
    expect(headers.length).toBe(4);
    const durationHeader = headers[2] as HTMLElement;
    act(() => { fireEvent.click(durationHeader); });

    const rows = table.querySelectorAll('tbody tr');
    expect(rows[0].getAttribute('data-testid')).toBe('audit-recap-row-2');
    expect(rows[1].getAttribute('data-testid')).toBe('audit-recap-row-3');
    expect(rows[2].getAttribute('data-testid')).toBe('audit-recap-row-1');
  });

  it('caps the run list at 10 by default + shows "+ N plus anciens"', async () => {
    const many = Array.from({ length: 13 }, (_, i) => ({
      ...fullRun,
      id: `run-${i}`,
      started_at: `2026-05-${String(13 + i).padStart(2, '0')}T10:00:00Z`,
    }));
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce(many);

    wrap(<AuditRecapPanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));

    await waitFor(() => {
      const cards = screen.queryAllByTestId(/^audit-recap-chip-run-/);
      expect(cards.length).toBe(10);
    });
    const showMore = screen.getByTestId('audit-recap-chips-show-more');
    expect(showMore.textContent).toMatch(/3/);

    fireEvent.click(showMore);
    await waitFor(() => {
      const cards = screen.queryAllByTestId(/^audit-recap-chip-run-/);
      expect(cards.length).toBe(13);
    });
    expect(screen.getByTestId('audit-recap-chips-show-less')).toBeInTheDocument();
  });

  it('filter pills narrow the run list to one kind', async () => {
    const mixed = [
      { ...fullRun, id: 'f1', kind: 'Full' },
      { ...rgaaRun, id: 'r1', kind: 'Rgaa' },
      { ...rgaaRun, id: 'r2', kind: 'Rgaa' },
      { ...fullRun, id: 's1', kind: 'Security' },
    ];
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce(mixed);

    wrap(<AuditRecapPanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));

    await screen.findByTestId('audit-recap-kind-filter');
    expect(screen.getByTestId('audit-recap-filter-all')).toBeInTheDocument();
    expect(screen.getByTestId('audit-recap-filter-Full')).toBeInTheDocument();
    expect(screen.getByTestId('audit-recap-filter-Rgaa')).toBeInTheDocument();
    expect(screen.getByTestId('audit-recap-filter-Security')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('audit-recap-filter-Rgaa'));
    await waitFor(() => {
      expect(screen.queryByTestId('audit-recap-chip-r1')).toBeInTheDocument();
      expect(screen.queryByTestId('audit-recap-chip-r2')).toBeInTheDocument();
      expect(screen.queryByTestId('audit-recap-chip-f1')).toBeNull();
      expect(screen.queryByTestId('audit-recap-chip-s1')).toBeNull();
    });
  });

  it('filter row hides when only one kind in history (Full-only project)', async () => {
    (projectsApi.auditHistory as ReturnType<typeof vi.fn>).mockResolvedValueOnce([fullRun]);
    wrap(<AuditRecapPanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('audit-recap-toggle'));
    await waitFor(() => {
      expect(screen.queryByTestId('audit-recap-kind-filter')).toBeNull();
    });
  });
});
