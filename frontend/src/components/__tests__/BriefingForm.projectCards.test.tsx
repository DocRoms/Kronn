// KT-1173 — the briefing's project facts are closed cards (choices), not prose.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { BriefingForm } from '../BriefingForm';
import { BRIEFING_PROJECT_CARDS } from '../briefingProjectCards';
import { projects as projectsApi } from '../../lib/api';

const wrap = () =>
  render(
    <I18nProvider>
      <BriefingForm
        projectId="p-cards"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={() => {}}
      />
    </I18nProvider>,
  );

const card = (key: string) => {
  const el = document.querySelector(`[data-card="${key}"]`);
  if (!el) throw new Error(`card ${key} missing`);
  return el as HTMLElement;
};

/** The card's value field (key, repository, URL), never its optional note. */
const valueInput = (key: string) => {
  const el = card(key).querySelector('input:not([data-note])');
  if (!el) throw new Error(`value input of ${key} missing`);
  return el as HTMLInputElement;
};

const fillMandatory = () => {
  const textareas = document.querySelectorAll('textarea');
  ['Purpose', 'Team', 'Maturity', 'Deps', 'Traps'].forEach((v, i) =>
    fireEvent.change(textareas[i], { target: { value: v } }),
  );
};

const submit = () => fireEvent.click(screen.getByRole('button', { name: /^(Save|Enregistrer|Guardar)$/i }));

beforeEach(() => {
  vi.clearAllMocks();
});

describe('BriefingForm project cards (KT-1173)', () => {
  it('renders every project card as a radio group with a single unknown default and no free text', () => {
    wrap();
    for (const def of BRIEFING_PROJECT_CARDS) {
      const group = within(card(def.key)).getByRole('radiogroup');
      const radios = within(group).getAllByRole('radio');
      expect(radios).toHaveLength(def.options.length);
      const checked = radios.filter(r => r.getAttribute('aria-checked') === 'true');
      expect(checked).toHaveLength(1);
      // No prose field inside a choice card until a choice asks for a value.
      expect(card(def.key).querySelector('textarea')).toBeNull();
    }
    // The six legacy questions stay the only textareas.
    expect(document.querySelectorAll('textarea')).toHaveLength(6);
    expect(card('tracker').querySelector('input:not([data-note])')).toBeNull();
  });

  it('a choice is exclusive inside its card', () => {
    wrap();
    const radios = within(card('merge_strategy')).getAllByRole('radio');
    fireEvent.click(radios[0]);
    fireEvent.click(radios[1]);
    const checked = radios.filter(r => r.getAttribute('aria-checked') === 'true');
    expect(checked).toEqual([radios[1]]);
  });

  it('sends the chosen option ids, the Jira key, environments and roles to saveBriefing', async () => {
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true);
    wrap();
    fillMandatory();

    fireEvent.click(within(card('tracker')).getByRole('radio', { name: 'Jira' }));
    fireEvent.change(valueInput('tracker'), { target: { value: 'KR' } });
    fireEvent.click(within(card('forge')).getByRole('radio', { name: 'GitHub' }));
    fireEvent.change(valueInput('forge'), { target: { value: 'DocRoms/Kronn' } });
    fireEvent.click(within(card('default_branch')).getByRole('radio', { name: 'main' }));

    const envs = within(card('environments')).getAllByRole('checkbox');
    fireEvent.click(envs[4]); // production
    fireEvent.change(valueInput('environments'), {
      target: { value: 'https://www.example.com' },
    });

    const roles = within(card('people')).getAllByRole('checkbox');
    fireEvent.click(roles[0]); // arbiter
    fireEvent.click(roles[4]); // qa, left empty: dropped
    const [name] = within(card('people')).getAllByRole('textbox');
    fireEvent.change(name, { target: { value: 'Alice' } });

    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const payload = (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mock.calls[0][1];
    expect(payload.purpose).toBe('Purpose');
    expect(payload.project).toEqual({
      tracker: 'jira',
      tracker_key: 'KR',
      forge: 'github',
      forge_repo: 'DocRoms/Kronn',
      default_branch: 'main',
      commit_convention: 'unknown',
      signoff_required: 'unknown',
      merge_strategy: 'unknown',
      review_policy: 'unknown',
      release_cadence: 'unknown',
      environments: [{ kind: 'production', url: 'https://www.example.com' }],
      people: [{ role: 'arbiter', name: 'Alice', contact: '' }],
    });
  });

  it('asks where the external documentation lives and sends its kind and address (KT-819)', async () => {
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true);
    wrap();
    fillMandatory();

    // Nothing answered yet: the card shows kinds, no address field.
    expect(card('external_docs').querySelector('input:not([data-note])')).toBeNull();
    const kinds = within(card('external_docs')).getAllByRole('checkbox');
    expect(kinds).toHaveLength(4);
    fireEvent.click(kinds[0]); // confluence
    fireEvent.change(valueInput('external_docs'), {
      target: { value: 'https://acme.atlassian.net/wiki/spaces/ENG' },
    });
    // A kind chosen and unchosen again leaves no trace.
    fireEvent.click(kinds[1]); // notion
    fireEvent.click(kinds[1]);

    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const payload = (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mock.calls[0][1];
    expect(payload.project.external_docs).toEqual([
      { kind: 'confluence', url: 'https://acme.atlassian.net/wiki/spaces/ENG' },
    ]);
  });

  it('an unanswered external documentation card sends no documentation', async () => {
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true);
    wrap();
    fillMandatory();
    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const payload = (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mock.calls[0][1];
    expect(payload.project.external_docs ?? []).toEqual([]);
  });

  it('drops a typed key once the tracker no longer takes one', async () => {
    (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true);
    wrap();
    fillMandatory();
    fireEvent.click(within(card('tracker')).getByRole('radio', { name: 'Jira' }));
    fireEvent.change(valueInput('tracker'), { target: { value: 'KR' } });
    fireEvent.click(within(card('tracker')).getByRole('radio', { name: 'GitHub Issues' }));
    expect(card('tracker').querySelector('input:not([data-note])')).toBeNull();
    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const payload = (projectsApi.saveBriefing as ReturnType<typeof vi.fn>).mock.calls[0][1];
    expect(payload.project.tracker).toBe('github_issues');
    expect(payload.project.tracker_key).toBe('');
  });
});
