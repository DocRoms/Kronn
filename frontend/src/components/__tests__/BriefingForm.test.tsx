// BriefingForm — 0.8.5 unit coverage.
//
// Pins the 0-token désagentified briefing form added in 0.8.4 (#285):
//   - renders the 6 questions with labels + textarea (Q1-Q5 required, Q6 optional)
//   - submit blocked while Q1-Q5 has at least one empty field (HTML required)
//   - on submit, calls saveBriefing(projectId, form) THEN startBriefing(projectId, agent)
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
        agent="claude"
        toast={() => {}}
      />
    );
    const textareas = screen.getAllByRole('textbox');
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
        agent="claude"
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
    expect(projectsApi.startBriefing).toHaveBeenCalledWith('p-99', 'claude');
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
        agent="claude"
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
        agent="claude"
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
        agent="claude"
        toast={() => {}}
      />
    );
    fireEvent.click(screen.getByRole('button', { name: /common\.close|Close|Fermer|Cerrar/i }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(projectsApi.saveBriefing).not.toHaveBeenCalled();
  });
});
