// A finished validation discussion is auto-archived: the audit timeline must
// validate from the card instead of offering a new validation discussion.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
    locale: 'fr',
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { Project, AgentDetection, Discussion } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-val', name: 'val-target', path: '/repos/val-target', repo_url: null, token_override: null,
  ai_config: { detected: false, configs: [] }, audit_status: 'Audited',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const ARCHIVED_VALIDATION = {
  id: 'd-val', project_id: 'p-val', title: 'Validation audit AI', archived: true,
} as unknown as Discussion;

function mockTimeline(latestValidation: unknown, runs: unknown[] = [{
  id: 'run-1', project_id: 'p-val', kind: 'Full', agent_type: 'ClaudeCode', started_at: '',
  status: 'Completed', td_total: 0, validation_discussion_id: 'd-val',
}]) {
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs,
    steps: [], recorded_audits: [], recorded_validated_at: null, latest_validation: latestValidation,
  } as never);
}

function renderCard() {
  const props = {
    project: PROJECT, isOpen: true, onToggleOpen: noop, discussions: [ARCHIVED_VALIDATION],
    driftStatus: undefined, agents: [AGENT], allSkills: [], mcpConfigs: [], workflows: [],
    configLanguage: 'fr', toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: vi.fn(),
    onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(), onRefetch: vi.fn(),
    onRefetchDiscussions: vi.fn(), onRefetchSkills: noop, onRefetchDrift: vi.fn(),
  };
  render(<ProjectCard {...(props as any)} />);
  return props;
}

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
  vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
});

describe('ProjectCard — validating after the validation discussion finished', () => {
  it('an archived, finished, linked validation validates the audit and refreshes', async () => {
    mockTimeline({ discussion_id: 'd-val', finished: true, archived: true });
    vi.mocked(projectsApi.validateAudit).mockResolvedValue('Validated' as never);
    const props = renderCard();

    const block = await screen.findByTestId('audit-timeline-validation-finished');
    await act(async () => { fireEvent.click(within(block).getByRole('button', { name: /audit\.validate/ })); });
    await waitFor(() => expect(projectsApi.validateAudit).toHaveBeenCalledWith('p-val'));
    await waitFor(() => expect(props.toast).toHaveBeenCalledWith('audit.done', 'success'));
    expect(props.onRefetch).toHaveBeenCalled();
    expect(props.onRefetchDiscussions).toHaveBeenCalled();
    expect(props.onSetDiscPrefill).not.toHaveBeenCalled();

    fireEvent.click(within(block).getByRole('button', { name: /auditTimeline\.validation\.viewDiscussion/ }));
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-val');
    expect(props.onNavigate).toHaveBeenCalledWith('discussions');
  });

  it('a refused validation is shown in an error toast', async () => {
    mockTimeline({ discussion_id: 'd-val', finished: true, archived: true });
    vi.mocked(projectsApi.validateAudit).mockRejectedValue(new Error('An audit is currently running'));
    const props = renderCard();

    const block = await screen.findByTestId('audit-timeline-validation-finished');
    await act(async () => { fireEvent.click(within(block).getByRole('button', { name: /audit\.validate/ })); });
    await waitFor(() => expect(props.toast).toHaveBeenCalledWith(expect.stringContaining('An audit is currently running'), 'error'));
    expect(props.toast).not.toHaveBeenCalledWith('audit.done', 'success');
  });

  it('an unfinished linked validation resumes the linked discussion without creating another', async () => {
    mockTimeline({ discussion_id: 'd-val', finished: false, archived: true });
    const props = renderCard();

    await waitFor(() => expect(projectsApi.auditTimeline).toHaveBeenCalled());
    const resume = await screen.findByRole('button', { name: /audit\.resumeValidation/ });
    expect(screen.queryByTestId('audit-timeline-validation-finished')).toBeNull();
    fireEvent.click(resume);
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-val');
    expect(props.onSetDiscPrefill).not.toHaveBeenCalled();
    expect(projectsApi.validateAudit).not.toHaveBeenCalled();
  });

  it('uses the latest specialized completed run and its exact validation link', async () => {
    mockTimeline(
      { discussion_id: 'd-security', finished: false, archived: false },
      [{
        id: 'run-security', project_id: 'p-val', kind: 'Security', agent_type: 'ClaudeCode', started_at: '',
        status: 'Completed', td_total: 0, validation_discussion_id: 'd-security',
      }],
    );
    const props = renderCard();

    const badge = await screen.findByTestId('project-validation-badge');
    fireEvent.click(badge);
    expect(props.onOpenDiscussion).toHaveBeenCalledWith('d-security');
    expect(props.onNavigate).toHaveBeenCalledWith('discussions');
    expect(props.onSetDiscPrefill).not.toHaveBeenCalled();
  });

  it.each(['Interrupted', 'Cancelled'])('does not validate an older completed Full run after a newer Security %s run', async (status) => {
    mockTimeline(
      { discussion_id: 'd-val', finished: true, archived: true },
      [
        {
          id: 'run-security', project_id: 'p-val', kind: 'Security', agent_type: 'ClaudeCode', started_at: '',
          status, td_total: 0, validation_discussion_id: null,
        },
        {
          id: 'run-full', project_id: 'p-val', kind: 'Full', agent_type: 'ClaudeCode', started_at: '',
          status: 'Completed', td_total: 0, validation_discussion_id: 'd-val',
        },
      ],
    );
    renderCard();

    expect(await screen.findByTestId('audit-timeline-validation-unavailable')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /audit\.validate/ })).toBeNull();
    expect(projectsApi.validateAudit).not.toHaveBeenCalled();
  });
});
