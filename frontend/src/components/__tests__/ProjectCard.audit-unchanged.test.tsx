// KT-1130 — a Full audit step whose target was already up to date ends
// `unchanged`: the card says so neutrally, never as an error.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, screen } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { Project, AgentDetection } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-reaudit', name: 'reaudit-target', path: '/repos/reaudit-target',
  repo_url: null, token_override: null, ai_config: { detected: false, configs: [] },
  audit_status: 'TemplateInstalled', briefing_notes: '# Project Briefing', ai_todo_count: 0, tech_debt_count: 0,
  needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

function renderCard(project: Project = PROJECT) {
  const props = {
    project, isOpen: true, onToggleOpen: noop, discussions: [],
    driftStatus: null, agents: [AGENT], allSkills: [], mcpConfigs: [],
    workflows: [], configLanguage: 'fr',
    toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: noop,
    onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
    onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: noop,
    onRefetchDrift: vi.fn(),
  };
  return { ...render(<ProjectCard {...(props as any)} />), props };
}

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
  vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
});
afterEach(() => { vi.restoreAllMocks(); });

describe('ProjectCard — unchanged audit step', () => {
  it('flags a briefing written with default answers (KT-1192)', () => {
    renderCard({
      ...PROJECT,
      briefing_notes: '# Project Briefing\n<!-- kronn:briefing-defaults -->\n',
    });
    expect(screen.getByTestId('project-briefing-defaults')).toBeInTheDocument();
  });

  it('shows no flag for a briefing a human wrote', () => {
    renderCard();
    expect(screen.queryByTestId('project-briefing-defaults')).toBeNull();
  });

  it('toasts an unchanged step as neutral info, and a failed one with the way to resume', async () => {
    vi.mocked(projectsApi.fullAuditStream).mockImplementation(async (_id, _req, handlers) => {
      handlers.onStepUnchanged?.(4, 'docs/coding-rules.md');
      handlers.onStepWarning?.(5, 'docs/testing-quality.md', 'placeholders remain', false);
      handlers.onDone('d-1', false, 'complete');
    });
    const { props } = renderCard();
    const btn = await screen.findByText(/audit.startFullAudit/);
    await act(async () => { fireEvent.click(btn); await Promise.resolve(); });

    const calls = (props.toast as ReturnType<typeof vi.fn>).mock.calls as [string, string][];
    expect(calls).toContainEqual(['audit.stepUnchanged docs/coding-rules.md', 'info']);
    // The unchanged step never reaches an error toast.
    expect(calls.filter(([, kind]) => kind === 'error').map(([msg]) => msg))
      .toEqual(['audit.stepWarning docs/testing-quality.md placeholders remain']);
  });
});
