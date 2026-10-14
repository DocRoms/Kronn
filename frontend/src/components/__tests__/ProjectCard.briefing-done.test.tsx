// KT-1213 — a briefing counts as done only when it holds text: notes made of
// whitespace leave the briefing step to fill (and so the audit launch blocked).
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
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

const project = (briefing_notes: string | null): Project => ({
  id: 'p-briefing', name: 'briefing-target', path: '/repos/briefing-target',
  repo_url: null, token_override: null, ai_config: { detected: false, configs: [] },
  audit_status: 'TemplateInstalled', briefing_notes, ai_todo_count: 0, tech_debt_count: 0,
  needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
});

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

function renderCard(notes: string | null) {
  const props = {
    project: project(notes), isOpen: true, onToggleOpen: noop, discussions: [],
    driftStatus: null, agents: [AGENT], allSkills: [], mcpConfigs: [],
    workflows: [], configLanguage: 'fr',
    toast: vi.fn(), onNavigate: vi.fn(), onSetDiscPrefill: noop,
    onAutoRunDiscussion: vi.fn(), onOpenDiscussion: vi.fn(),
    onRefetch: vi.fn(), onRefetchDiscussions: vi.fn(), onRefetchSkills: noop,
    onRefetchDrift: vi.fn(),
  };
  return render(<ProjectCard {...(props as any)} />);
}

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
  vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: [], steps: [], recorded_audits: [], recorded_validated_at: null, latest_validation: null, lease_orphaned: false,
  } as never);
});

afterEach(() => {
  localStorage.clear();
});

describe('ProjectCard — briefing done (KT-1213)', () => {
  it('treats whitespace-only notes as no briefing: the step stays to fill', async () => {
    renderCard('  \n\t \n');
    await waitFor(() => expect(screen.getByText('auditTimeline.status.toFill')).toBeInTheDocument());
    expect(screen.queryByText('auditTimeline.status.saved')).toBeNull();
  });

  it('counts a briefing with text as done', async () => {
    renderCard('# Project Briefing');
    await waitFor(() => expect(screen.getByText('auditTimeline.status.saved')).toBeInTheDocument());
    expect(screen.queryByText('auditTimeline.status.toFill')).toBeNull();
  });
});
