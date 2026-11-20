import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { WorkflowsPage } from '../WorkflowsPage';
import { triggerDownload } from '../../lib/downloadBlob';
import type { Workflow, WorkflowSummary } from '../../types/generated';

// KT-1843 — an export that names what it does not hold says so before the file leaves.

const mockSkillsApi = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
}));
const mockProjectsApi = vi.hoisted(() => ({
  usedSkills: vi.fn(),
  usedSkillFile: vi.fn(),
}));
const mockWorkflowsApi = vi.hoisted(() => ({
  list: vi.fn(),
  get: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
  trigger: vi.fn(),
  listRuns: vi.fn().mockResolvedValue([]),
  countRuns: vi.fn().mockResolvedValue(0),
  getRun: vi.fn(),
  deleteRun: vi.fn(),
  deleteAllRuns: vi.fn(),
  cancelRun: vi.fn(),
  triggerStream: vi.fn(),
  exportWorkflow: vi.fn(),
  importWorkflow: vi.fn(),
  previewImport: vi.fn().mockResolvedValue({ skills: [], missing_skills: [] }),
  autoDisabled: vi.fn().mockResolvedValue([]),
  reenable: vi.fn().mockResolvedValue([]),
}));
const mockQuickPromptsApi = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  setPinned: vi.fn(),
  delete: vi.fn(),
  batchRun: vi.fn(),
  compareAgents: vi.fn(),
  exportQp: vi.fn(),
  importQp: vi.fn(),
}));

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: () => ({ connected: false, connectionState: 'connecting' }),
}));

vi.mock('../../lib/api', () => ({
  workflows: mockWorkflowsApi,
  skills: mockSkillsApi,
  projects: mockProjectsApi,
  profiles: { list: vi.fn().mockResolvedValue([]), get: vi.fn(), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  directives: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  quickPrompts: mockQuickPromptsApi,
  quickApis: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), setPinned: vi.fn(), delete: vi.fn() },
  quickExecs: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), setPinned: vi.fn(), delete: vi.fn() },
  config: { getUiLanguage: vi.fn().mockResolvedValue('fr'), saveUiLanguage: vi.fn().mockResolvedValue(undefined), getServerConfig: vi.fn().mockResolvedValue({ default_model_tier: 'default' }) },
  mcps: {
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], customized_contexts: [], incompatibilities: [] }),
    registry: vi.fn().mockResolvedValue([]),
  },
  discussions: { create: vi.fn() },
  modelCatalogApi: { list: vi.fn().mockResolvedValue({ targets: [] }) },
  externalApi: { list: vi.fn().mockResolvedValue([]) },
}));
vi.mock('../../lib/downloadBlob', () => ({ triggerDownload: vi.fn() }));

const SUMMARY = {
  id: 'wf-review', name: 'PR review', project_id: null, project_name: null,
  trigger_type: 'manual', step_count: 0, misconfigured_step_count: 0, unsafe_step_count: 0,
  enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z',
} as WorkflowSummary;
const WORKFLOW = {
  id: SUMMARY.id, name: SUMMARY.name, project_id: null,
  trigger: { type: 'Manual' }, steps: [], actions: [],
  safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
  workspace_config: null, concurrency_limit: null, enabled: true, pinned: false,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
} as Workflow;
const NOT_CARRIED = [
  { kind: 'skill', id: 'custom-gone', named_in: 'PR review › publish' },
  { kind: 'directive', id: 'custom-tone', named_in: 'PR review › review' },
];
const file = (envelope: object) => ({
  filename: 'pr-review.kronn-workflow.json',
  blob: new Blob([JSON.stringify({ version: 1, ...envelope })], { type: 'application/json' }),
});

afterEach(() => {
  cleanup();
  vi.mocked(triggerDownload).mockClear();
  mockWorkflowsApi.exportWorkflow.mockReset();
});

const openAndExport = async (envelope: object) => {
  mockSkillsApi.list.mockResolvedValue([]);
  mockProjectsApi.usedSkills.mockResolvedValue([]);
  mockQuickPromptsApi.list.mockResolvedValue([]);
  mockWorkflowsApi.list.mockResolvedValue([SUMMARY]);
  mockWorkflowsApi.get.mockResolvedValue(WORKFLOW);
  mockWorkflowsApi.exportWorkflow.mockResolvedValue(file(envelope));
  const toast = vi.fn();
  await act(async () => {
    render(
      <I18nProvider>
        <WorkflowsPage projects={[]} toast={toast} selection={{ tab: 'workflows', resourceId: SUMMARY.id, runId: null }} />
      </I18nProvider>,
    );
  });
  const button = await screen.findByText('Exporter');
  await act(async () => { fireEvent.click(button); });
  await waitFor(() => expect(mockWorkflowsApi.exportWorkflow).toHaveBeenCalledWith(SUMMARY.id));
  return toast;
};

describe('WorkflowsPage — export report (KT-1843)', () => {
  it('downloads at once an export that carries everything it names', async () => {
    const toast = await openAndExport({});
    await waitFor(() => expect(triggerDownload).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId('export-not-carried')).toBeNull();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('PR review'), 'success');
  });

  it('lists what the file names and does not hold, and downloads only once confirmed', async () => {
    const toast = await openAndExport({ not_carried: NOT_CARRIED });
    const dialog = await screen.findByTestId('export-not-carried');
    expect(triggerDownload).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
    const rows = within(dialog).getAllByTestId('export-not-carried-row');
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('custom-gone');
    expect(rows[0]).toHaveTextContent('PR review › publish');

    await act(async () => { fireEvent.click(within(dialog).getByTestId('export-not-carried-download')); });
    await waitFor(() => expect(triggerDownload).toHaveBeenCalledTimes(1));
    expect(vi.mocked(triggerDownload).mock.calls[0][0]).toBe('pr-review.kronn-workflow.json');
    expect(screen.queryByTestId('export-not-carried')).toBeNull();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('PR review'), 'success');
  });

  it('downloads nothing when the reader closes the list', async () => {
    const toast = await openAndExport({ not_carried: NOT_CARRIED });
    const dialog = await screen.findByTestId('export-not-carried');
    await act(async () => { fireEvent.click(within(dialog).getByRole('button', { name: 'Annuler' })); });
    expect(screen.queryByTestId('export-not-carried')).toBeNull();
    expect(triggerDownload).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
  });
});
