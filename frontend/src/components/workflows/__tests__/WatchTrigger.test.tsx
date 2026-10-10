// KT-1099 — Watch trigger and per-trigger timezone in the wizard, and the
// poll history a Watch workflow shows on its card.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { buildApiMock } from '../../../test/apiMock';
import type { Workflow, WorkflowStep, WatchStatus } from '../../../types/generated';

const { updateMock, quickApiListMock, cronPreviewMock } = vi.hoisted(() => ({
  updateMock: vi.fn(),
  quickApiListMock: vi.fn(),
  cronPreviewMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  workflows: { update: updateMock as never, cronPreview: cronPreviewMock as never },
  quickApis: { list: quickApiListMock as never },
}));

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join(',')}` : key,
    locale: 'en',
    setLocale: () => {},
  }),
}));

import { WorkflowWizard } from '../WorkflowWizard';
import { WatchStatusLine } from '../WatchStatusLine';
import { buildCronTrigger, buildWatchTrigger, formatFires, watchDraftFrom } from '../../../lib/watchTrigger';

const step = (name: string): WorkflowStep => ({
  id: null,
  name,
  step_type: { type: 'Agent' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: 'do the thing',
  mode: { type: 'Normal' },
  output_format: { type: 'Structured' },
} as WorkflowStep);

const workflow = (trigger: Workflow['trigger']): Workflow => ({
  id: 'wf-1',
  name: 'Watched',
  project_id: null,
  trigger,
  steps: [step('main'), step('second')],
  actions: [],
  safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
  workspace_config: null,
  concurrency_limit: null,
  enabled: false,
  pinned: false,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
} as Workflow);

const props: ComponentProps<typeof WorkflowWizard> = {
  projects: [],
  onDone: vi.fn(),
  onCancel: vi.fn(),
  installedAgentTypes: ['ClaudeCode'],
};

/** Infos → Trigger. */
const openTrigger = () => fireEvent.click(screen.getByText('wiz.next'));
/** Trigger → Steps → Config → Summary, then save. */
const saveFromTrigger = async () => {
  for (let i = 0; i < 3; i++) fireEvent.click(screen.getByText('wiz.next'));
  fireEvent.click(screen.getByText('wiz.save'));
  await waitFor(() => expect(updateMock).toHaveBeenCalledTimes(1));
  return updateMock.mock.calls[0][1].trigger;
};

beforeEach(() => {
  updateMock.mockReset();
  updateMock.mockResolvedValue({});
  quickApiListMock.mockReset();
  cronPreviewMock.mockReset();
  cronPreviewMock.mockResolvedValue({ timezone: 'UTC', inherited: true, next: [] });
  quickApiListMock.mockResolvedValue([]);
  vi.stubGlobal('confirm', vi.fn(() => true));
});

describe('watchTrigger helpers', () => {
  it('omits empty fields and keeps the source the editor does not show', () => {
    const draft = watchDraftFrom({
      type: 'Watch', api_plugin_slug: 'github', api_config_id: 'cfg', api_endpoint_path: '/repos/o/r',
      api_query: { per_page: '1' }, interval: '*/5 * * * *', detection: { type: 'Body' },
    });
    expect(buildWatchTrigger(draft)).toEqual({
      type: 'Watch', api_plugin_slug: 'github', api_config_id: 'cfg', api_endpoint_path: '/repos/o/r',
      api_query: { per_page: '1' }, interval: '*/5 * * * *', detection: { type: 'Body' },
    });
    expect(buildWatchTrigger({ ...draft, source: 'quick_api', quickApiId: 'qa-1', timezone: ' Europe/Paris ' }))
      .toEqual({
        type: 'Watch', quick_api_id: 'qa-1', api_query: { per_page: '1' }, interval: '*/5 * * * *',
        timezone: 'Europe/Paris', detection: { type: 'Body' },
      });
  });

  it('leaves a Cron without a timezone in UTC', () => {
    expect(buildCronTrigger('0 7 * * *', '  ')).toEqual({ type: 'Cron', schedule: '0 7 * * *' });
    expect(buildCronTrigger('0 7 * * *', 'Europe/Paris')).toEqual({ type: 'Cron', schedule: '0 7 * * *', timezone: 'Europe/Paris' });
  });
});

describe('WorkflowWizard — Watch trigger', () => {
  it('edits a Watch trigger and saves its detection mode and timezone', async () => {
    render(<WorkflowWizard {...props} editWorkflow={workflow({
      type: 'Watch', api_plugin_slug: 'github', api_config_id: 'cfg', api_endpoint_path: '/repos/o/r/commits',
      interval: '*/10 * * * *', detection: { type: 'Validators' },
    })} />);
    openTrigger();
    expect(screen.getByTestId('watch-trigger-editor')).toBeInTheDocument();
    expect(screen.getByLabelText('wiz.watchEndpoint')).toHaveValue('/repos/o/r/commits');
    fireEvent.change(screen.getByLabelText('wiz.watchDetection'), { target: { value: 'JsonPath' } });
    fireEvent.change(screen.getByLabelText('wiz.watchJsonPath'), { target: { value: '$[0].sha' } });
    fireEvent.change(screen.getByLabelText('wiz.timezone'), { target: { value: 'Europe/Paris' } });

    expect(await saveFromTrigger()).toEqual({
      type: 'Watch', api_plugin_slug: 'github', api_config_id: 'cfg', api_endpoint_path: '/repos/o/r/commits',
      interval: '*/10 * * * *', timezone: 'Europe/Paris', detection: { type: 'JsonPath', path: '$[0].sha' },
    });
  });

  it('switches a manual workflow to a Watch on a saved GET Quick API', async () => {
    quickApiListMock.mockResolvedValue([
      { id: 'qa-get', name: 'Commits', icon: '📡', api_method: 'GET', api_endpoint_path: '/commits' },
      { id: 'qa-post', name: 'Create', icon: '✍️', api_method: 'POST', api_endpoint_path: '/issues' },
    ]);
    render(<WorkflowWizard {...props} editWorkflow={workflow({ type: 'Manual' })} />);
    openTrigger();
    fireEvent.click(screen.getByText('wiz.triggerWatch'));
    fireEvent.change(screen.getByLabelText('wiz.watchSource'), { target: { value: 'quick_api' } });
    const picker = screen.getByLabelText('wiz.watchQuickApi');
    await waitFor(() => expect(picker.querySelectorAll('option')).toHaveLength(2));
    expect(picker.querySelector('option[value="qa-post"]')).toBeNull();
    fireEvent.change(picker, { target: { value: 'qa-get' } });

    expect(await saveFromTrigger()).toEqual({
      type: 'Watch', quick_api_id: 'qa-get', interval: '*/5 * * * *', detection: { type: 'Validators' },
    });
  });

  it('keeps the query override of a Quick API Watch when only its interval changes', async () => {
    quickApiListMock.mockResolvedValue([
      { id: 'qa-get', name: 'Commits', icon: '📡', api_method: 'GET', api_endpoint_path: '/commits' },
    ]);
    render(<WorkflowWizard {...props} editWorkflow={workflow({
      type: 'Watch', quick_api_id: 'qa-get', api_query: { since: '2026-10-01' },
      interval: '*/5 * * * *', detection: { type: 'Body' },
    })} />);
    openTrigger();
    fireEvent.change(screen.getByLabelText('wiz.watchInterval'), { target: { value: '*/15 * * * *' } });
    fireEvent.change(screen.getByLabelText('wiz.watchDetection'), { target: { value: 'Validators' } });

    expect(await saveFromTrigger()).toEqual({
      type: 'Watch', quick_api_id: 'qa-get', api_query: { since: '2026-10-01' },
      interval: '*/15 * * * *', detection: { type: 'Validators' },
    });
  });

  it('reads a Cron in Kronn\'s zone unless a timezone is set, and says so', async () => {
    render(<WorkflowWizard {...props} editWorkflow={workflow({ type: 'Cron', schedule: '0 7 * * 1-5' })} />);
    openTrigger();
    expect(screen.getByText('wiz.timezoneDefaultNote')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('wiz.timezone'), { target: { value: 'Europe/Paris' } });
    expect(await saveFromTrigger()).toEqual({ type: 'Cron', schedule: '0 7 * * 1-5', timezone: 'Europe/Paris' });
  });

  // KT-1103 — the editor shows the zone in use and the next 3 firings in it.
  it('shows the zone in use and the next three firings in that zone', async () => {
    cronPreviewMock.mockImplementation(async ({ timezone }: { timezone?: string }) => timezone
      ? { timezone, inherited: false, next: ['2026-10-26T07:00:00+01:00', '2026-10-27T07:00:00+01:00', '2026-10-28T07:00:00+01:00'] }
      : { timezone: 'Asia/Tokyo', inherited: true, next: ['2026-10-26T07:00:00+09:00'] });
    render(<WorkflowWizard {...props} editWorkflow={workflow({ type: 'Cron', schedule: '0 7 * * 1-5' })} />);
    openTrigger();
    const preview = () => screen.getByTestId(/wf-cron-timezone.*-preview/);
    await waitFor(() => expect(preview()).toHaveTextContent('wiz.timezoneInUseKronn:Asia/Tokyo'));
    expect(cronPreviewMock).toHaveBeenLastCalledWith({ schedule: '0 7 * * 1-5' });
    expect(screen.getByLabelText('wiz.timezone')).toHaveAttribute('placeholder', 'Asia/Tokyo');

    fireEvent.change(screen.getByLabelText('wiz.timezone'), { target: { value: 'Europe/Paris' } });
    await waitFor(() => expect(preview()).toHaveTextContent('wiz.timezoneInUse:Europe/Paris'));
    expect(cronPreviewMock).toHaveBeenLastCalledWith({ schedule: '0 7 * * 1-5', timezone: 'Europe/Paris' });
    const fires = formatFires(['2026-10-26T07:00:00+01:00', '2026-10-27T07:00:00+01:00', '2026-10-28T07:00:00+01:00'], 'Europe/Paris', 'en');
    expect(preview()).toHaveTextContent(`wiz.cronNextFires:${fires}`);
  });

  it('formats the firings in the schedule\'s zone, not the browser\'s', () => {
    const at = ['2026-10-26T06:00:00Z'];
    expect(formatFires(at, 'Europe/Paris', 'en')).toContain('07:00');
    expect(formatFires(at, 'Asia/Tokyo', 'en')).toContain('15:00');
    expect(formatFires(at, 'UTC', 'fr')).toMatch(/lun\.?.*06:00/);
  });

  it('does not add a timezone to an existing Cron saved unchanged', async () => {
    render(<WorkflowWizard {...props} editWorkflow={workflow({ type: 'Cron', schedule: '0 7 * * 1-5' })} />);
    openTrigger();
    expect(await saveFromTrigger()).toEqual({ type: 'Cron', schedule: '0 7 * * 1-5' });
  });
});

describe('WatchStatusLine', () => {
  const status = (over: Partial<WatchStatus> = {}): WatchStatus => ({
    last_poll_at: '2026-10-09T10:00:00Z',
    last_result: 'unchanged',
    last_http_status: 304,
    last_error: null,
    last_change_at: null,
    unchanged_count: 12,
    changed_count: 2,
    error_count: 1,
    consecutive_failures: 0,
    failing: false,
    ...over,
  });

  it('shows the last poll, its result and the counters', () => {
    render(<WatchStatusLine status={status()} />);
    expect(screen.getByText(/wf\.watch\.lastPoll:/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.watch\.result\.unchanged/)).toHaveTextContent('(304)');
    expect(screen.getByText('wf.watch.counters:12,2,1')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('shows consecutive failures as an error with the last error as its title', () => {
    render(<WatchStatusLine status={status({
      last_result: 'error', last_http_status: 500, last_error: 'HTTP 500 on GET /status',
      consecutive_failures: 3, failing: true,
    })} />);
    const badge = screen.getByRole('alert');
    expect(badge).toHaveTextContent('wf.watch.failing:3');
    expect(badge).toHaveAttribute('title', 'HTTP 500 on GET /status');
    expect(screen.getByTestId('watch-status')).toHaveAttribute('data-failing', 'true');
  });
});
