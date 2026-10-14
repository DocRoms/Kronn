// ProjectCard — header accessibility regression.
//
// The card header contains interactive status and disclosure controls. A
// native button wrapper would therefore produce invalid nested buttons, so
// the outer wrapper remains a keyboard-operable div.
// `<div role="button" tabIndex={0}>` with explicit Enter/Space keyboard
// handlers, while keeping the same toggle behaviour.
//
// This test pins the contract:
// 1. The header is keyboard-focusable.
// 2. Pressing Enter or Space toggles the card.
// 3. The header is not a `<button>` element (would create the nested-
//    button HTML violation that this fix removed).

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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
import type { Project, AgentDetection, Discussion, DriftCheckResponse } from '../../types/generated';
import { projects as projectsApi } from '../../lib/api';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-1',
  name: 'demo',
  path: '/repos/demo',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code',
  agent_type: 'ClaudeCode',
  installed: true,
  enabled: true,
  path: '/usr/bin/claude',
  version: '1.0.0',
  latest_version: null,
  origin: 'host',
  install_command: null,
  host_managed: false,
  host_label: null,
  runtime_available: false,
  rtk_available: false,
  rtk_hook_configured: false,
};

function renderCard(opts: {
  onToggleOpen?: () => void;
  project?: Partial<Project>;
  isOpen?: boolean;
  discussions?: Discussion[];
  driftStatus?: DriftCheckResponse;
  onOpenDiscussion?: (id: string) => void;
  onNavigate?: (page: string) => void;
} = {}) {
  return render(
    <ProjectCard
      project={{ ...PROJECT, ...opts.project }}
      isOpen={opts.isOpen ?? false}
      onToggleOpen={opts.onToggleOpen ?? noop}
      discussions={opts.discussions ?? []}
      driftStatus={opts.driftStatus}
      agents={[AGENT]}
      allSkills={[]}
      mcpConfigs={[]}
      workflows={[]}
      configLanguage="fr"
      toast={vi.fn()}
      onNavigate={opts.onNavigate ?? noop}
      onSetDiscPrefill={noop}
      onAutoRunDiscussion={noop}
      onOpenDiscussion={opts.onOpenDiscussion ?? noop}
      onRefetch={noop}
      onRefetchDiscussions={noop}
      onRefetchSkills={noop}
      onRefetchDrift={noop}
    />
  );
}

describe('ProjectCard — header accessibility', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('header uses div role=button (not a real <button>) to avoid nested-button HTML violation', () => {
    renderCard();
    const header = document.querySelector('.dash-card-header');
    expect(header).not.toBeNull();
    // Must NOT be an actual <button> element — that's the bug we fixed
    // (would create a nested-button HTML violation with the inner drift
    // update button).
    expect(header!.tagName).toBe('DIV');
    expect(header!.getAttribute('role')).toBe('button');
    expect(header!.getAttribute('tabindex')).toBe('0');
  });

  it('clicking the header calls onToggleOpen', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header')!;
    act(() => { fireEvent.click(header); });
    expect(onToggleOpen).toHaveBeenCalledTimes(1);
  });

  it('Enter key on the header toggles the card', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    act(() => { fireEvent.keyDown(header, { key: 'Enter' }); });
    expect(onToggleOpen).toHaveBeenCalledTimes(1);
  });

  it('Space key on the header toggles the card', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    act(() => { fireEvent.keyDown(header, { key: ' ' }); });
    expect(onToggleOpen).toHaveBeenCalledTimes(1);
  });

  it('Tab key on the header does not toggle (browser handles focus)', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    act(() => { fireEvent.keyDown(header, { key: 'Tab' }); });
    expect(onToggleOpen).not.toHaveBeenCalled();
  });

  it('keeps only status, one next action, doc state, and Details in the compact header', async () => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 375 });
    vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
      runs: [{
        id: 'run-1', project_id: 'p-1', kind: 'Full', agent_type: 'ClaudeCode',
        started_at: '2026-10-12T10:00:00Z', status: 'Completed', td_total: 0,
      }],
      steps: [], recorded_audits: [], recorded_validated_at: null,
      latest_validation: null, lease_orphaned: false,
    } as never);
    vi.mocked(projectsApi.docProvenance).mockResolvedValue({ to_reconfirm: 1 } as never);
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValue({
      sources_state: 'needs_reverification', sections: [], agent_steps: [], reanchor_count: 2, human_count: 0,
    });
    renderCard({
      project: { audit_status: 'Audited' },
      driftStatus: {
        audit_date: '2026-10-12',
        stale_sections: [{ ai_file: 'docs/AGENTS.md', audit_step: 1, changed_sources: ['src/lib.rs'] }],
        fresh_sections: [], total_sections: 1, sources_state: 'needs_reverification',
        broken_citations: 1,
        placeholder_docs: [{ path: 'docs/glossary.md', step: 2 }],
      },
    });

    const header = document.querySelector('.dash-card-header') as HTMLElement;
    await waitFor(() => expect(within(header).getByTestId('project-doc-state-summary')).toHaveTextContent('docState.summaryAttention 5'));
    expect(header.querySelectorAll(':scope .project-audit-primary')).toHaveLength(1);
    expect(header.querySelectorAll(':scope .doc-state-summary')).toHaveLength(1);
    const details = header.querySelector(':scope .project-audit-details') as HTMLDetailsElement;
    expect(details).not.toBeNull();
    expect(details.open).toBe(false);
    expect(within(header).queryByTestId('project-drift-update')).toBeNull();
  });

  it('marks a validated project modified when re-anchoring is the only signal', async () => {
    vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
      runs: [{
        id: 'run-1', project_id: 'p-1', kind: 'Full', agent_type: 'ClaudeCode',
        started_at: '2026-10-12T10:00:00Z', status: 'Completed', td_total: 0,
      }],
      steps: [], recorded_audits: [], recorded_validated_at: null,
      latest_validation: null, lease_orphaned: false,
    } as never);
    vi.mocked(projectsApi.docProvenance).mockResolvedValue({ docs: [], to_reconfirm: 0 });
    vi.mocked(projectsApi.revalidationPlan).mockResolvedValue({
      sources_state: 'needs_reverification', sections: [], agent_steps: [], reanchor_count: 1, human_count: 0,
    });
    renderCard({
      project: { audit_status: 'Validated' },
      driftStatus: {
        audit_date: '2026-10-12', stale_sections: [], fresh_sections: [], total_sections: 0,
        sources_state: 'needs_reverification', broken_citations: 0, placeholder_docs: [],
      },
    });

    const header = document.querySelector('.dash-card-header') as HTMLElement;
    await waitFor(() => expect(within(header).getByText('docState.status.validatedModified')).toBeInTheDocument());
    expect(within(header).getByTestId('project-doc-state-summary')).toHaveTextContent('docState.summaryAttention 1');
  });

  it('keeps the doc state unknown when the revalidation plan cannot be loaded', async () => {
    vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
      runs: [{
        id: 'run-1', project_id: 'p-1', kind: 'Full', agent_type: 'ClaudeCode',
        started_at: '2026-10-12T10:00:00Z', status: 'Completed', td_total: 0,
      }],
      steps: [], recorded_audits: [], recorded_validated_at: null,
      latest_validation: null, lease_orphaned: false,
    } as never);
    vi.mocked(projectsApi.docProvenance).mockResolvedValue({ docs: [], to_reconfirm: 0 });
    vi.mocked(projectsApi.revalidationPlan).mockRejectedValue(new Error('unavailable'));
    renderCard({
      project: { audit_status: 'Audited' },
      driftStatus: {
        audit_date: '2026-10-12', stale_sections: [], fresh_sections: [], total_sections: 0,
        sources_state: 'clean', broken_citations: 0, placeholder_docs: [],
      },
    });

    const header = document.querySelector('.dash-card-header') as HTMLElement;
    await waitFor(() => expect(within(header).getByTestId('project-doc-state-summary')).toHaveTextContent('docState.summaryUnknown'));
    expect(within(header).getByText('docState.status.unknown')).toBeInTheDocument();
  });

  it('reports a legacy ai/ layout with migration as the only next action', () => {
    renderCard({ project: { audit_status: 'TemplateInstalled', needs_docs_migration: true } });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    expect(within(header).getByText('docState.status.legacy')).toBeInTheDocument();
    expect(within(header).getByTestId('project-audit-next-action')).toHaveTextContent('docState.action.migrate');
  });

  it('reports detected human docs and asks for the write folder', async () => {
    vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
      runs: [], steps: [], recorded_audits: [], recorded_validated_at: null,
      latest_validation: null, lease_orphaned: false,
    } as never);
    vi.mocked(projectsApi.briefingDefaults).mockResolvedValue({
      docs_dir: 'docs', docs_dir_source: 'existing', folder_exists: true,
      folder: { kind: 'human_docs', files: 3, site_generator: null, legacy_exists: false },
    } as never);
    renderCard({ project: { audit_status: 'TemplateInstalled' } });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    await waitFor(() => expect(within(header).getByText('docState.status.humanDocs')).toBeInTheDocument());
    expect(within(header).getByTestId('project-audit-next-action')).toHaveTextContent('docState.action.chooseFolder');
  });

  it('reports a cancelled first audit and offers relaunch', async () => {
    vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
      runs: [{
        id: 'run-cancelled', project_id: 'p-1', kind: 'Full', agent_type: 'ClaudeCode',
        started_at: '2026-10-12T10:00:00Z', ended_at: '2026-10-12T10:05:00Z',
        status: 'Cancelled', td_total: 0,
      }],
      steps: [], recorded_audits: [], recorded_validated_at: null,
      latest_validation: null, lease_orphaned: false,
    } as never);
    vi.mocked(projectsApi.briefingDefaults).mockResolvedValue({
      docs_dir: 'docs', docs_dir_source: 'default', folder_exists: false,
      folder: { kind: 'missing' },
    } as never);
    renderCard({ project: { audit_status: 'TemplateInstalled' } });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    await waitFor(() => expect(within(header).getByText(/docState\.status\.cancelled/)).toBeInTheDocument());
    expect(within(header).getByTestId('project-audit-next-action')).toHaveTextContent('docState.action.relaunch');
  });

  describe('clickable badges (KT-1072)', () => {
    it('the anti-hallu inject badge is a button activated by Enter, without toggling the card', async () => {
      vi.mocked(projectsApi.antiHalluStatus).mockResolvedValue({ present: false, file_exists: true });
      const onToggleOpen = vi.fn();
      renderCard({ onToggleOpen, isOpen: true, project: { audit_status: 'Audited' } });
      const badge = await screen.findByTestId('project-anti-hallu-inject');
      expect(badge.tagName).toBe('BUTTON');
      badge.focus();
      await userEvent.keyboard('{Enter}');
      await waitFor(() => expect(projectsApi.injectAntiHallu).toHaveBeenCalledWith('p-1'));
      expect(onToggleOpen).not.toHaveBeenCalled();
    });

    it('the validation badge opens the validation discussion from the keyboard', async () => {
      const onOpenDiscussion = vi.fn();
      const onNavigate = vi.fn();
      const validation = { id: 'd-val', project_id: 'p-1', title: 'Validation audit AI', archived: false } as unknown as Discussion;
      vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
        runs: [{
          id: 'run-1', project_id: 'p-1', kind: 'Full', agent_type: 'ClaudeCode',
          started_at: '2026-10-12T10:00:00Z', status: 'Completed', td_total: 0,
        }],
        steps: [], recorded_audits: [], recorded_validated_at: null,
        latest_validation: { discussion_id: 'd-val', finished: false, archived: false },
        lease_orphaned: false,
      } as never);
      renderCard({ isOpen: true, project: { audit_status: 'Audited' }, discussions: [validation], onOpenDiscussion, onNavigate });
      const badge = await screen.findByTestId('project-validation-badge');
      expect(badge.tagName).toBe('BUTTON');
      badge.focus();
      await userEvent.keyboard(' ');
      expect(onOpenDiscussion).toHaveBeenCalledWith('d-val');
      expect(onNavigate).toHaveBeenCalledWith('discussions');
    });

    it('the tech-debt badge is a keyboard-reachable button', async () => {
      const onToggleOpen = vi.fn();
      renderCard({ onToggleOpen, project: { audit_status: 'Audited', tech_debt_count: 3 } });
      const badge = await screen.findByTestId('project-tech-debt-badge');
      expect(badge.tagName).toBe('BUTTON');
      badge.focus();
      await userEvent.keyboard('{Enter}');
      expect(onToggleOpen).toHaveBeenCalledTimes(1);
    });
  });
});
