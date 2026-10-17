// BriefingForm — 0.8.5 unit coverage.
//
// Pins the 0-token désagentified briefing form added in 0.8.4 (#285):
//   - renders the 6 questions with labels + textarea (Q1-Q5 required, Q6 optional)
//   - submit blocked while Q1-Q5 has at least one empty field (HTML required)
//   - on submit, calls saveBriefing(projectId, form) THEN startBriefing(projectId, agent, tier)
//   - emits a success toast + onSaved(discussionId) when the disc spawn succeeds
//   - falls back to a warning toast + onSaved(null) when the spawn fails but save succeeds
//   - submit button toggles to "saving…" state while in flight + buttons disabled

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { BriefingForm } from '../BriefingForm';
import { projects as projectsApi } from '../../lib/api';

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const fillMandatory = () => {
  // Q1-Q5 are required textareas (placeholder-anchored to avoid coupling
  // to i18n strings inside the test).
  const textareas = screen.getAllByRole('textbox');
  // 6 textareas total; first 5 are required.
  fireEvent.change(textareas[0], { target: { value: 'Drive editorial decisions in real time' } });
  fireEvent.change(textareas[1], { target: { value: '3 devs + 1 PO' } });
  fireEvent.change(textareas[2], { target: { value: 'Production, ~6 months' } });
  fireEvent.change(textareas[3], { target: { value: 'PostgreSQL + Redis' } });
  fireEvent.change(textareas[4], { target: { value: 'Migrations sans backfill' } });
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('BriefingForm (0.8.5)', () => {
  it('renders 6 textareas: 5 required + 1 optional', () => {
    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        toast={() => {}}
      />
    );
    // The single-line inputs (KT-999 documentation path) are not questions.
    const textareas = Array.from(document.querySelectorAll('textarea'));
    expect(textareas).toHaveLength(6);
    expect(textareas[0]).toBeRequired();
    expect(textareas[1]).toBeRequired();
    expect(textareas[2]).toBeRequired();
    expect(textareas[3]).toBeRequired();
    expect(textareas[4]).toBeRequired();
    expect(textareas[5]).not.toBeRequired();
  });

  it('calls saveBriefing then startBriefing and forwards the spawned id on success', async () => {
    const onSaved = vi.fn();
    const toast = vi.fn();
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce({});
    (projectsApi.startBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ discussion_id: 'disc-42' });

    wrap(
      <BriefingForm
        projectId="p-99"
        onClose={() => {}}
        onSaved={onSaved}
        agent="ClaudeCode"
        tier="reasoning"
        toast={toast}
      />
    );
    fillMandatory();
    fireEvent.click(screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i }));

    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    expect(projectsApi.saveBriefing).toHaveBeenCalledWith(
      'p-99',
      expect.objectContaining({
        purpose: expect.any(String),
        team: expect.any(String),
        maturity: expect.any(String),
        dependencies: expect.any(String),
        traps: expect.any(String),
      })
    );
    expect(projectsApi.startBriefing).toHaveBeenCalledWith('p-99', 'ClaudeCode', 'reasoning');
    expect(onSaved).toHaveBeenCalledWith('disc-42');
    expect(toast).toHaveBeenCalledWith(expect.any(String), 'success');
  });

  it('emits a warning toast + onSaved(null) when the disc spawn fails but save succeeded', async () => {
    const onSaved = vi.fn();
    const toast = vi.fn();
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce({});
    (projectsApi.startBriefing as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('no agent'));

    wrap(
      <BriefingForm
        projectId="p-fail"
        onClose={() => {}}
        onSaved={onSaved}
        agent="ClaudeCode"
        tier="reasoning"
        toast={toast}
      />
    );
    fillMandatory();
    fireEvent.click(screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i }));

    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(null));
    // The warning toast for "saved but no disc" was emitted, and no success toast fired.
    const calls = toast.mock.calls.map(c => c[1]);
    expect(calls).toContain('warning');
    expect(calls).not.toContain('success');
  });

  it('disables both buttons while submitting and re-enables on settle', async () => {
    let resolveSave: (v: unknown) => void = () => {};
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockReturnValueOnce(
      new Promise(r => { resolveSave = r; })
    );
    (projectsApi.startBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ discussion_id: 'disc-1' });

    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        toast={() => {}}
      />
    );
    fillMandatory();
    const submit = screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i });
    const cancel = screen.getByRole('button', { name: /Cancel|Annuler|Cancelar/i });

    fireEvent.click(submit);

    // In-flight: both buttons disabled.
    await waitFor(() => expect(submit).toBeDisabled());
    expect(cancel).toBeDisabled();

    // Resolve the save → the form proceeds to startBriefing → eventually completes.
    resolveSave({});
    await waitFor(() => expect(projectsApi.startBriefing).toHaveBeenCalled());
  });

  it('clicking the close button fires onClose without saving', () => {
    const onClose = vi.fn();
    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={onClose}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        toast={() => {}}
      />
    );
    fireEvent.click(screen.getByRole('button', { name: /common\.close|Close|Fermer|Cerrar/i }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(projectsApi.saveBriefing).not.toHaveBeenCalled();
  });

  // KT-1167 — the option travels with the answers, unchecked by default.
  it('sends the TD-to-tasks option with the answers', async () => {
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValue({});
    const props = {
      onClose: () => {}, onSaved: () => {}, agent: 'ClaudeCode' as const,
      tier: 'reasoning' as const, review: false, toast: () => {},
    };
    const { unmount } = wrap(<BriefingForm projectId="p-off" {...props} />);
    fillMandatory();
    fireEvent.click(screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i }));
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledWith(
      'p-off', expect.objectContaining({ td_to_tasks: false }),
    ));
    unmount();

    wrap(<BriefingForm projectId="p-on" {...props} />);
    fillMandatory();
    fireEvent.click(screen.getByTestId('briefing-td-to-tasks'));
    fireEvent.click(screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i }));
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledWith(
      'p-on', expect.objectContaining({ td_to_tasks: true }),
    ));
  });
  // KT-1183 — experimental, off by default; 2 or 3 agents; cost shown first.
  it('sends the cross-agent audit option with its agents, never by default', async () => {
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValue({});
    (projectsApi.crossAuditEstimate as ReturnType<typeof vi.fn>).mockResolvedValue({
      agents: 2, cross_checked_steps: ['docs/inconsistencies-security.md'], low_factor: 2, high_factor: 4,
      max_cross_checked: 30, base_run_id: 'run-1', low_cost_usd_micros: 200_000, high_cost_usd_micros: 400_000,
      estimated: true,
    });
    const toast = vi.fn();
    const props = {
      onClose: () => {}, onSaved: () => {}, agent: 'ClaudeCode' as const,
      tier: 'reasoning' as const, review: false, toast,
    };
    const { unmount } = wrap(<BriefingForm projectId="p-off" {...props} />);
    expect(screen.getByTestId('briefing-cross-audit')).not.toBeChecked();
    fillMandatory();
    fireEvent.click(screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i }));
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledWith(
      'p-off', expect.objectContaining({ cross_audit_agents: [] }),
    ));
    expect(projectsApi.crossAuditEstimate).not.toHaveBeenCalled();
    unmount();

    wrap(<BriefingForm projectId="p-on" {...props} />);
    fillMandatory();
    fireEvent.click(screen.getByTestId('briefing-cross-audit'));
    fireEvent.click(screen.getByTestId('briefing-cross-agent-ClaudeCode'));
    // One agent is not a cross-check: refused before the round-trip.
    fireEvent.click(screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i }));
    expect(toast).toHaveBeenCalledWith(expect.any(String), 'error');
    expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByTestId('briefing-cross-agent-Codex'));
    await waitFor(() => expect(projectsApi.crossAuditEstimate).toHaveBeenCalledWith('p-on', 2));
    await waitFor(() => expect(screen.getByTestId('briefing-cross-estimate').textContent).toMatch(/×2.*×4/));
    fireEvent.click(screen.getByRole('button', { name: /Save|Enregistrer|Guardar/i }));
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledWith(
      'p-on', expect.objectContaining({ cross_audit_agents: ['ClaudeCode', 'Codex'] }),
    ));
  });

  it('pre-fills the AI doc folder and sends it with the answers (KT-1192)', async () => {
    vi.mocked(projectsApi.briefingDefaults).mockResolvedValueOnce({
      docs_dir: '.ai', docs_dir_source: 'global_default', folder_exists: true, folder: { kind: 'missing' },
    });
    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={() => {}}
      />
    );
    const input = await screen.findByTestId('briefing-docs-dir-input');
    await waitFor(() => expect(input).toHaveValue('.ai'));
    expect(screen.getByTestId('briefing-docs-dir')).toHaveTextContent('.ai/');
    fireEvent.change(input, { target: { value: 'context' } });
    fillMandatory();
    fireEvent.submit(input.closest('form')!);
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalled());
    expect(vi.mocked(projectsApi.saveBriefing).mock.calls[0][1]).toMatchObject({ docs_dir: 'context' });
  });

  it('saves default answers in one click (KT-1192)', async () => {
    const onSaved = vi.fn();
    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={() => {}}
        onSaved={onSaved}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={() => {}}
      />
    );
    const button = await screen.findByTestId('briefing-use-defaults');
    await waitFor(() => expect(button).not.toBeDisabled());
    fireEvent.click(button);
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    expect(vi.mocked(projectsApi.saveBriefing).mock.calls[0][1]).toMatchObject({
      purpose: 'Not provided', traps: 'Not provided', docs_dir: 'docs',
    });
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(null));
  });

  it('asks for an explicit choice before touching human docs, with the warning (KT-1192)', async () => {
    vi.mocked(projectsApi.briefingDefaults).mockResolvedValueOnce({
      docs_dir: 'docs', docs_dir_source: 'existing', folder_exists: true,
      folder: { kind: 'human_docs', files: 4, site_generator: 'mkdocs.yml', legacy_exists: false },
    } as never);
    const toast = vi.fn();
    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={toast}
      />
    );
    const choice = await screen.findByTestId('briefing-existing-docs');
    expect(choice).toHaveTextContent('mkdocs.yml');
    expect(choice).toHaveTextContent('4');
    fillMandatory();
    const form = screen.getByTestId('briefing-docs-dir-input').closest('form')!;
    fireEvent.submit(form);
    await waitFor(() => expect(toast).toHaveBeenCalledWith(expect.any(String), 'error'));
    expect(projectsApi.saveBriefing).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('briefing-existing-legacy'));
    fireEvent.submit(form);
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    expect(vi.mocked(projectsApi.saveBriefing).mock.calls[0][1]).toMatchObject({
      docs_dir: 'docs', move_existing_to_legacy: true,
    });
  });

  it('keeps the human docs where they are when another folder is typed', async () => {
    vi.mocked(projectsApi.briefingDefaults).mockResolvedValueOnce({
      docs_dir: 'docs', docs_dir_source: 'existing', folder_exists: true,
      folder: { kind: 'human_docs', files: 4, site_generator: null, legacy_exists: false },
    } as never);
    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={() => {}}
      />
    );
    await screen.findByTestId('briefing-existing-docs');
    fireEvent.change(screen.getByTestId('briefing-docs-dir-input'), { target: { value: 'kronn-docs' } });
    expect(screen.queryByTestId('briefing-existing-docs')).toBeNull();
    fillMandatory();
    fireEvent.submit(screen.getByTestId('briefing-docs-dir-input').closest('form')!);
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const sent = vi.mocked(projectsApi.saveBriefing).mock.calls[0][1];
    expect(sent).toMatchObject({ docs_dir: 'kronn-docs' });
    expect(sent).not.toHaveProperty('move_existing_to_legacy');
  });

  it('says a Kronn folder is updated in place, with nothing to choose', async () => {
    vi.mocked(projectsApi.briefingDefaults).mockResolvedValueOnce({
      docs_dir: 'docs', docs_dir_source: 'existing', folder_exists: true, folder: { kind: 'kronn_managed' },
    } as never);
    wrap(
      <BriefingForm
        projectId="p-1"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={() => {}}
      />
    );
    await waitFor(() => expect(screen.getByTestId('briefing-docs-dir-state').textContent).toMatch(/Kronn/));
    expect(screen.queryByTestId('briefing-existing-docs')).toBeNull();
  });
});
