// KT-1489 — a validated project whose docs were edited outside Kronn says so
// in step 4, and « Valider cette version » records them as they stand without
// an agent, or lists the citations that still block it.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { AuditTimeline, type AuditTimelineProps } from '../AuditTimeline';
import type { AgentDetection } from '../../types/generated';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true, path: '/usr/bin/claude',
  version: '1.0.0', latest_version: null, origin: 'host', install_command: null, host_managed: false,
  host_label: null, runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

function props(over: Partial<AuditTimelineProps> = {}): AuditTimelineProps {
  return {
    projectId: 'p1', auditStatus: 'Validated', agents: [AGENT],
    selectedAgent: 'ClaudeCode', selectedTier: 'reasoning', modelTiers: null, selectedConnectionId: null, onSelect: vi.fn(),
    briefingDone: true, onBriefingSaved: vi.fn(), auditActive: false, liveStep: 0, liveTotal: 0,
    liveFile: '', liveElapsed: null, liveTool: null, liveStartedAt: null, liveToolCalls: null, liveStepTokens: null,
    liveTotalTokens: null, onResumeBriefingDiscussion: null, onCancel: vi.fn(), resumable: null,
    onLaunch: vi.fn(), validationInProgress: false,
    onMarkValid: vi.fn().mockResolvedValue(undefined), onOpenValidation: vi.fn(), techDebtEntry: null,
    onVerifyDocs: vi.fn(), refreshTrigger: 0, toast: vi.fn(), ...over,
  };
}

const provenance = (outside: string[]) => ({ docs: [], to_reconfirm: 0, outside_kronn: outside });

beforeEach(() => {
  vi.mocked(projectsApi.docProvenance).mockReset();
  vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance([]));
  vi.mocked(projectsApi.validateCurrentDocs).mockReset();
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: [{ id: 'run-1', kind: 'Full', project_id: 'p1', agent_type: 'ClaudeCode', started_at: '', status: 'Completed', td_total: 0 }],
    steps: [], recorded_audits: [], recorded_validated_at: '2026-10-27',
    recorded_validated_by: 'Romu', recorded_validation_started_by: null,
    latest_validation: null, lease_orphaned: false, validation_block: null,
  } as never);
});

describe('AuditTimeline — docs modified outside Kronn (KT-1489)', () => {
  it('flags the validated docs that changed, with the validation date', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance(['docs/AGENTS.md', 'docs/repo-map.md']));
    wrap(<AuditTimeline {...props()} />);

    const banner = await screen.findByTestId('audit-timeline-outside-kronn');
    expect(banner).toHaveTextContent('hors de Kronn');
    expect(banner).toHaveTextContent('2026-10-27');
    expect(within(banner).getByText('docs/repo-map.md')).toBeInTheDocument();
    expect(within(banner).getByTestId('audit-timeline-validate-version')).toBeEnabled();
  });

  it('offers one button and keeps the changed docs folded', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance(['docs/AGENTS.md', 'docs/repo-map.md']));
    wrap(<AuditTimeline {...props()} />);

    const banner = await screen.findByTestId('audit-timeline-outside-kronn');
    expect(within(banner).getAllByRole('button')).toHaveLength(1);
    expect(within(banner).getByRole('button')).toHaveTextContent("Valider la doc telle qu'elle est");
    // The button carries its explanation as visible text, not as a tooltip.
    expect(within(banner).getByRole('button')).not.toHaveAttribute('title');
    expect(banner).toHaveTextContent('Contrôle gratuit des citations, sans agent.');
    const docs = within(banner).getByTestId('audit-timeline-outside-docs') as HTMLDetailsElement;
    expect(docs.open).toBe(false);
    expect(docs).toHaveTextContent('Voir les 2 doc(s) modifiée(s)');
  });

  it('shows no banner while an audit is running', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance(['docs/AGENTS.md']));
    wrap(<AuditTimeline {...props({ auditActive: true })} />);

    await waitFor(() => expect(projectsApi.docProvenance).toHaveBeenCalled());
    await screen.findByTestId('audit-timeline-validated-by');
    expect(screen.queryByTestId('audit-timeline-outside-kronn')).toBeNull();
  });

  it('does not print an empty date when the validation has none recorded', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance(['docs/AGENTS.md']));
    vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
      runs: [{ id: 'run-1', kind: 'Full', project_id: 'p1', agent_type: 'ClaudeCode', started_at: '', status: 'Completed', td_total: 0 }],
      steps: [], recorded_audits: [], recorded_validated_at: null,
      recorded_validation_started_by: null, recorded_validated_by: null,
      latest_validation: null, lease_orphaned: false, validation_block: null,
    } as never);
    wrap(<AuditTimeline {...props()} />);

    const banner = await screen.findByTestId('audit-timeline-outside-kronn');
    expect(banner).toHaveTextContent('depuis la dernière validation');
    expect(banner).not.toHaveTextContent(/validation du\s*[—.]/);
  });

  it('says nothing when no doc differs from what Kronn recorded', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance([]));
    wrap(<AuditTimeline {...props()} />);

    await waitFor(() => expect(projectsApi.docProvenance).toHaveBeenCalled());
    await screen.findByTestId('audit-timeline-validated-by');
    expect(screen.queryByTestId('audit-timeline-outside-kronn')).toBeNull();
  });

  it('does not look for outside edits on a project that is not validated', async () => {
    wrap(<AuditTimeline {...props({ auditStatus: 'Audited' })} />);

    await screen.findByText(/Documentation validée/);
    expect(projectsApi.docProvenance).not.toHaveBeenCalled();
    expect(screen.queryByTestId('audit-timeline-outside-kronn')).toBeNull();
  });

  it('validates the current version directly and clears the banner', async () => {
    // The docs read as validated once the server has recorded them.
    vi.mocked(projectsApi.docProvenance).mockResolvedValueOnce(provenance(['docs/AGENTS.md']));
    vi.mocked(projectsApi.validateCurrentDocs).mockResolvedValue({ validated: true, broken: [], reanchored: 0, docs: 3 });
    const onVersionValidated = vi.fn();
    const toast = vi.fn();
    wrap(<AuditTimeline {...props({ onVersionValidated, toast })} />);

    await userEvent.click(await screen.findByTestId('audit-timeline-validate-version'));

    await waitFor(() => expect(projectsApi.validateCurrentDocs).toHaveBeenCalledWith('p1'));
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-outside-kronn')).toBeNull());
    expect(onVersionValidated).toHaveBeenCalledTimes(1);
    expect(toast).toHaveBeenCalledWith(expect.any(String), 'success');
  });

  it('swaps the primary for the fix action and folds the broken citations when the approval is refused', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance(['docs/AGENTS.md']));
    vi.mocked(projectsApi.validateCurrentDocs).mockResolvedValue({
      validated: false, reanchored: 0, docs: 0,
      broken: [{
        doc: 'docs/AGENTS.md',
        citation: { cite: '[src: file: src/gone.rs:3]', state: 'file_missing' },
      }],
    } as never);
    const onVersionValidated = vi.fn();
    const onVerifyDocs = vi.fn();
    wrap(<AuditTimeline {...props({ onVersionValidated, onVerifyDocs })} />);

    await userEvent.click(await screen.findByTestId('audit-timeline-validate-version'));

    const banner = screen.getByTestId('audit-timeline-outside-kronn');
    const broken = await within(banner).findByTestId('audit-timeline-outside-broken') as HTMLDetailsElement;
    expect(broken.open).toBe(false);
    expect(broken).toHaveTextContent('Voir les 1 citation(s) à corriger');
    expect(broken).toHaveTextContent('src/gone.rs:3');
    expect(banner).toHaveTextContent('ne pointent plus vers le code');
    expect(onVersionValidated).not.toHaveBeenCalled();
    // One primary, named by its outcome: the same action as the header's « Vérifier la doc ».
    // The retry is a quiet link, not a second button of the same weight.
    const primaries = within(banner).getAllByRole('button').filter(b => b.className.includes('audit-tl-btn-primary'));
    expect(primaries).toHaveLength(1);
    expect(within(banner).queryByTestId('audit-timeline-validate-version')).toBeNull();
    await userEvent.click(within(banner).getByRole('button', { name: 'Corriger les citations et chemins' }));
    expect(onVerifyDocs).toHaveBeenCalledTimes(1);
  });

  it('lets the user check again in the open view after fixing the citations by hand', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValueOnce(provenance(['docs/AGENTS.md']));
    vi.mocked(projectsApi.validateCurrentDocs)
      .mockResolvedValueOnce({
        validated: false, reanchored: 0, docs: 0,
        broken: [{ doc: 'docs/AGENTS.md', citation: { cite: 'src/gone.rs:3', state: 'file_missing' } }],
      } as never)
      .mockResolvedValueOnce({ validated: true, broken: [], reanchored: 0, docs: 3 });
    const onVersionValidated = vi.fn();
    wrap(<AuditTimeline {...props({ onVersionValidated })} />);

    await userEvent.click(await screen.findByTestId('audit-timeline-validate-version'));
    const banner = screen.getByTestId('audit-timeline-outside-kronn');
    await within(banner).findByTestId('audit-timeline-outside-broken');
    expect(within(banner).queryByTestId('audit-timeline-validate-version')).toBeNull();

    // The files were repaired outside Kronn: the same panel offers the retry.
    await userEvent.click(within(banner).getByTestId('audit-timeline-recheck-version'));

    await waitFor(() => expect(projectsApi.validateCurrentDocs).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByTestId('audit-timeline-outside-kronn')).toBeNull());
    expect(onVersionValidated).toHaveBeenCalledTimes(1);
  });

  it('keeps the refusal and the retry when the docs are still broken on the second try', async () => {
    vi.mocked(projectsApi.docProvenance).mockResolvedValue(provenance(['docs/AGENTS.md']));
    const refused = {
      validated: false, reanchored: 0, docs: 0,
      broken: [{ doc: 'docs/AGENTS.md', citation: { cite: 'src/gone.rs:3', state: 'file_missing' } }],
    };
    vi.mocked(projectsApi.validateCurrentDocs).mockResolvedValue(refused as never);
    wrap(<AuditTimeline {...props()} />);

    await userEvent.click(await screen.findByTestId('audit-timeline-validate-version'));
    const banner = screen.getByTestId('audit-timeline-outside-kronn');
    await userEvent.click(await within(banner).findByTestId('audit-timeline-recheck-version'));

    await waitFor(() => expect(projectsApi.validateCurrentDocs).toHaveBeenCalledTimes(2));
    expect(await within(banner).findByTestId('audit-timeline-outside-broken')).toBeInTheDocument();
    expect(within(banner).getByTestId('audit-timeline-recheck-version')).toBeEnabled();
  });
});
