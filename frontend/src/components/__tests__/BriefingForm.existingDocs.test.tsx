// KT-999 — the briefing asks about existing documentation (pre-filled from the
// repository) and lets the linked repos be edited in place.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { BriefingDetectedDoc, LinkedRepo } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { BriefingForm } from '../BriefingForm';
import { projects as projectsApi } from '../../lib/api';

const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

const DETECTED: BriefingDetectedDoc[] = [
  { path: 'README.md', kind: 'readme', is_dir: false, last_commit: '2024-03-02' },
  { path: 'docs', kind: 'docs', is_dir: true, last_commit: null },
  { path: 'README.fr.md', kind: 'readme', is_dir: false, last_commit: '2023-01-10' },
];

const wrap = (linkedRepos?: LinkedRepo[]) =>
  render(
    <I18nProvider>
      <BriefingForm
        projectId="p-docs"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={() => {}}
        linkedRepos={linkedRepos}
        onLinkedReposChange={() => {}}
      />
    </I18nProvider>,
  );

const row = (path: string) => {
  const el = document.querySelector(`[data-doc-path="${path}"]`);
  if (!el) throw new Error(`row ${path} missing`);
  return el as HTMLElement;
};

const fillMandatory = () => {
  const textareas = document.querySelectorAll('textarea');
  ['Purpose', 'Team', 'Maturity', 'Deps', 'Traps'].forEach((v, i) =>
    fireEvent.change(textareas[i], { target: { value: v } }),
  );
};

const submit = () => fireEvent.click(screen.getByRole('button', { name: /^(Save|Enregistrer|Guardar)$/i }));

const sot = (path: string) =>
  within(row(path)).getByRole('checkbox', { name: new RegExp(`^${path.replace('.', '\\.')} est la source`) }) as HTMLInputElement;

beforeEach(() => {
  vi.clearAllMocks();
  mocked(projectsApi.briefingDocs).mockResolvedValue(DETECTED);
});

describe('BriefingForm existing documentation (KT-999)', () => {
  it('pre-checks every detected location and shows its last commit date', async () => {
    wrap();
    await waitFor(() => expect(row('README.md')).toBeTruthy());
    expect(projectsApi.briefingDocs).toHaveBeenCalledWith('p-docs');
    for (const doc of DETECTED) {
      expect((within(row(doc.path)).getByRole('checkbox', { name: `Garder ${doc.path}` }) as HTMLInputElement).checked).toBe(true);
    }
    expect(row('README.md').textContent).toContain('Dernier commit 2024-03-02');
    expect(row('docs').textContent).toContain('Aucun commit trouvé');
    expect(row('docs').textContent).toContain('docs/');
  });

  it('keeps one source of truth per kind and none on an obsolete location', async () => {
    wrap();
    await waitFor(() => expect(row('README.md')).toBeTruthy());
    fireEvent.click(sot('README.md'));
    fireEvent.click(sot('docs'));
    fireEvent.click(sot('README.fr.md'));
    expect(sot('README.md').checked).toBe(false);
    expect(sot('README.fr.md').checked).toBe(true);
    expect(sot('docs').checked).toBe(true);

    fireEvent.click(within(row('docs')).getByRole('radio', { name: 'Obsolète' }));
    expect(sot('docs').checked).toBe(false);
    expect(sot('docs').disabled).toBe(true);
  });

  it('sends the kept locations with their state, note and source of truth', async () => {
    mocked(projectsApi.saveBriefing).mockResolvedValueOnce(true);
    wrap();
    await waitFor(() => expect(row('README.md')).toBeTruthy());
    fillMandatory();
    fireEvent.click(within(row('README.md')).getByRole('radio', { name: 'Référence' }));
    fireEvent.change(within(row('README.md')).getByRole('textbox', { name: 'Note sur README.md' }), {
      target: { value: '  setup only  ' },
    });
    fireEvent.click(sot('README.md'));
    fireEvent.click(within(row('README.fr.md')).getByRole('checkbox', { name: 'Garder README.fr.md' }));

    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const payload = mocked(projectsApi.saveBriefing).mock.calls[0][1];
    expect(payload.existing_docs).toEqual({
      locations: [
        { path: 'README.md', kind: 'readme', state: 'reference', note: 'setup only', source_of_truth: true, last_commit: null },
        { path: 'docs', kind: 'docs', state: 'unknown', note: '', source_of_truth: false, last_commit: null },
      ],
    });
  });

  it('adds a typed path once the server accepts it and shows its refusal otherwise', async () => {
    mocked(projectsApi.checkBriefingDoc)
      .mockRejectedValueOnce(new Error('documentation path `../x` must be relative to the repository, without `..`'))
      .mockResolvedValueOnce({ path: 'handbook', kind: 'docs', is_dir: true, last_commit: '2022-05-01' });
    wrap();
    await waitFor(() => expect(row('README.md')).toBeTruthy());
    const input = screen.getByRole('textbox', { name: 'Autre chemin de documentation' });

    fireEvent.change(input, { target: { value: '../x' } });
    // Enter adds the path and is consumed, so it cannot submit the briefing form.
    expect(fireEvent.keyDown(input, { key: 'Enter' })).toBe(false);
    expect(await screen.findByRole('alert')).toHaveTextContent('must be relative');
    expect(document.querySelector('[data-doc-path="../x"]')).toBeNull();

    fireEvent.change(input, { target: { value: './handbook/' } });
    fireEvent.click(screen.getByTestId('briefing-docs-add'));
    await waitFor(() => expect(row('handbook')).toBeTruthy());
    expect(projectsApi.checkBriefingDoc).toHaveBeenLastCalledWith('p-docs', './handbook/');
    expect(row('handbook').textContent).toContain('2022-05-01');
    expect(screen.queryByRole('alert')).toBeNull();
    // Enter in the path field never submitted the briefing.
    expect(projectsApi.saveBriefing).not.toHaveBeenCalled();
  });

  it('shows the linked repos drawer with one-click suggestions when given the repos', async () => {
    mocked(projectsApi.linkedReposCandidates).mockResolvedValue([
      { id: 'p-api', name: 'api', path: '/r/api', proximity_hint: 'same-parent' },
    ]);
    wrap([]);
    const section = await screen.findByTestId('briefing-linked-repos');
    const suggestion = await within(section).findByRole('button', { name: /api/ });
    fireEvent.click(suggestion);
    // The draft opens on the suggestion; its description is left to fill in.
    expect((within(section).getByPlaceholderText(/nom/i) as HTMLInputElement).value).toBe('api');
    expect(projectsApi.saveBriefing).not.toHaveBeenCalled();
  });

  it('leaves the linked repos section out when the parent gives no repos', async () => {
    wrap(undefined);
    await waitFor(() => expect(row('README.md')).toBeTruthy());
    expect(screen.queryByTestId('briefing-linked-repos')).toBeNull();
  });
});
