// KT-1196 — the briefing form re-opens with the saved answers, and every
// project fact takes an optional note.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { BriefingDetectedDoc, BriefingFact, SavedBriefing } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { BriefingForm } from '../BriefingForm';
import { cleanFactNotes, emptyProjectAnswers, projectUnanswered } from '../briefingProjectCards';
import { mergeSavedDocs } from '../briefingDocs';
import { projects as projectsApi } from '../../lib/api';

const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

const ALL_FACTS: BriefingFact[] = [
  'tracker', 'tracker_key', 'forge', 'forge_repo', 'default_branch', 'commit_convention',
  'signoff', 'merge_strategy', 'review', 'release_cadence', 'environments', 'people',
];

const DETECTED: BriefingDetectedDoc[] = [
  { path: 'README.md', kind: 'readme', is_dir: false, last_commit: '2024-03-02' },
  { path: 'docs', kind: 'docs', is_dir: true, last_commit: null },
];

const SAVED: SavedBriefing = {
  purpose: 'News site\n\nTwo paragraphs',
  team: 'Four devs',
  maturity: 'Production',
  dependencies: '- Jira\n- Fastly',
  traps: 'Cache purge',
  additional: '',
  project: {
    ...emptyProjectAnswers(),
    tracker: 'jira',
    tracker_key: 'EW',
    forge: 'github',
    forge_repo: 'euronews/front',
    review_policy: 'two_approvals',
    environments: [{ kind: 'production', url: 'https://www.example.com' }],
    people: [{ role: 'devops', name: 'Sam', contact: '#devops' }],
    notes: { review: '2 approbations, CODEOWNERS sur backend/', tracker_key: 'board EW only' },
  },
  existing_docs: {
    locations: [
      { path: 'README.md', kind: 'readme', state: 'reference', note: 'setup', source_of_truth: true, last_commit: '2024-03-02' },
    ],
  },
  td_to_tasks: true,
  cross_audit_agents: ['ClaudeCode', 'Codex'],
};

const wrap = () =>
  render(
    <I18nProvider>
      <BriefingForm
        projectId="p-prefill"
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
const note = (fact: BriefingFact) => {
  const el = document.querySelector(`input[data-note="${fact}"]`);
  if (!el) throw new Error(`note ${fact} missing`);
  return el as HTMLInputElement;
};
const checked = (key: string) =>
  within(card(key)).getAllByRole('radio').find(r => r.getAttribute('aria-checked') === 'true')?.textContent;
const submit = () => fireEvent.click(screen.getByRole('button', { name: /^(Save|Enregistrer|Guardar)$/i }));
const payload = () => mocked(projectsApi.saveBriefing).mock.calls[0][1];

beforeEach(() => {
  vi.clearAllMocks();
  mocked(projectsApi.briefingDocs).mockResolvedValue(DETECTED);
});

describe('BriefingForm prefill (KT-1196)', () => {
  it('pre-fills every section from the saved briefing', async () => {
    mocked(projectsApi.savedBriefing).mockResolvedValueOnce(SAVED);
    wrap();
    await screen.findByTestId('briefing-prefilled');
    expect(projectsApi.savedBriefing).toHaveBeenCalledWith('p-prefill');

    const textareas = [...document.querySelectorAll('textarea')].map(t => t.value);
    expect(textareas).toEqual(['News site\n\nTwo paragraphs', 'Four devs', 'Production', '- Jira\n- Fastly', 'Cache purge', '']);

    expect(checked('tracker')).toBe('Jira');
    expect(checked('forge')).toBe('GitHub');
    expect(checked('review_policy')).toBe('Deux approbations');
    expect(screen.getByLabelText('Clé du projet')).toHaveValue('EW');
    expect(card('forge').querySelector('input:not([data-note])')).toHaveValue('euronews/front');
    expect(card('environments').querySelector('input[type="url"]')).toHaveValue('https://www.example.com');
    expect(within(card('people')).getAllByRole('textbox').map(i => (i as HTMLInputElement).value))
      .toEqual(['Sam', '#devops', '']);
    expect(note('review')).toHaveValue('2 approbations, CODEOWNERS sur backend/');
    expect(note('tracker_key')).toHaveValue('board EW only');
    expect(note('forge')).toHaveValue('');

    expect(screen.getByTestId('briefing-td-to-tasks')).toBeChecked();
    expect(screen.getByTestId('briefing-cross-audit')).toBeChecked();
    expect(screen.getByTestId('briefing-cross-agent-ClaudeCode')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('briefing-cross-agent-Codex')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('briefing-cross-agent-Kiro')).toHaveAttribute('aria-checked', 'false');

    // The saved location keeps its answers; one detected since stays unchecked.
    const readme = await waitFor(() => {
      const el = document.querySelector('[data-doc-path="README.md"]');
      if (!el) throw new Error('README row missing');
      return el as HTMLElement;
    });
    expect(within(readme).getByRole('checkbox', { name: /README\.md$/ })).toBeChecked();
    expect(within(readme).getByRole('radio', { checked: true })).toHaveTextContent('Référence');
    expect(within(readme).getByDisplayValue('setup')).toBeInTheDocument();
    const docsRow = document.querySelector('[data-doc-path="docs"]') as HTMLElement;
    expect(within(docsRow).getAllByRole('checkbox')[0]).not.toBeChecked();
  });

  it('saving unchanged sends back exactly what was loaded', async () => {
    mocked(projectsApi.savedBriefing).mockResolvedValueOnce(SAVED);
    mocked(projectsApi.saveBriefing).mockResolvedValueOnce(true);
    wrap();
    await screen.findByTestId('briefing-prefilled');
    await waitFor(() => expect(document.querySelector('[data-doc-path="docs"]')).not.toBeNull());
    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const sent = payload();
    expect(sent.purpose).toBe(SAVED.purpose);
    expect(sent.team).toBe(SAVED.team);
    expect(sent.maturity).toBe(SAVED.maturity);
    expect(sent.dependencies).toBe(SAVED.dependencies);
    expect(sent.traps).toBe(SAVED.traps);
    expect(sent.additional).toBe('');
    expect(sent.project).toEqual(SAVED.project);
    expect(sent.existing_docs).toEqual({
      locations: [{ ...SAVED.existing_docs!.locations[0], last_commit: null }],
    });
    expect(sent.td_to_tasks).toBe(true);
    expect(sent.cross_audit_agents).toEqual(['ClaudeCode', 'Codex']);
  });

  it('without a saved briefing the form starts empty and keeps every detected location', async () => {
    wrap();
    await waitFor(() => expect(document.querySelector('[data-doc-path="docs"]')).not.toBeNull());
    expect(screen.queryByTestId('briefing-prefilled')).toBeNull();
    expect([...document.querySelectorAll('textarea')].every(t => t.value === '')).toBe(true);
    const docsRow = document.querySelector('[data-doc-path="docs"]') as HTMLElement;
    expect(within(docsRow).getAllByRole('checkbox')[0]).toBeChecked();
  });

  it('a click that only focuses a field does not stop the prefill', async () => {
    let resolve: (v: SavedBriefing) => void = () => {};
    mocked(projectsApi.savedBriefing).mockReturnValueOnce(new Promise<SavedBriefing>(r => { resolve = r; }));
    wrap();
    const purpose = document.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.click(purpose);
    fireEvent.focus(purpose);
    resolve(SAVED);
    await screen.findByTestId('briefing-prefilled');
    expect(purpose.value).toBe(SAVED.purpose);
    expect(screen.queryByTestId('briefing-late-prefill')).toBeNull();
  });

  it('after a real change, keeps the typed text, fills only the empty sections and offers the rest', async () => {
    let resolve: (v: SavedBriefing) => void = () => {};
    mocked(projectsApi.savedBriefing).mockReturnValueOnce(new Promise<SavedBriefing>(r => { resolve = r; }));
    mocked(projectsApi.saveBriefing).mockResolvedValue(true);
    wrap();
    const purpose = document.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.change(purpose, { target: { value: 'Typed first' } });
    resolve(SAVED);
    const notice = await screen.findByTestId('briefing-late-prefill');
    expect(purpose.value).toBe('Typed first');
    expect(screen.queryByTestId('briefing-prefilled')).toBeNull();
    await waitFor(() => expect(document.querySelector('[data-doc-path="docs"]')).not.toBeNull());

    // Saving now loses nothing the user did not change.
    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    const sent = payload();
    expect(sent.purpose).toBe('Typed first');
    expect(sent.team).toBe(SAVED.team);
    expect(sent.traps).toBe(SAVED.traps);
    expect(sent.project).toEqual(SAVED.project);
    expect(sent.existing_docs.locations.map((l: { path: string }) => l.path)).toEqual(['README.md']);
    expect(sent.td_to_tasks).toBe(true);
    expect(sent.cross_audit_agents).toEqual(['ClaudeCode', 'Codex']);

    // « Charger le briefing enregistré » replaces the typed text too.
    fireEvent.click(within(notice).getByRole('button', { name: 'Charger le briefing enregistré' }));
    expect(purpose.value).toBe(SAVED.purpose);
    expect(screen.queryByTestId('briefing-late-prefill')).toBeNull();
    expect(screen.getByTestId('briefing-prefilled')).toBeInTheDocument();
  });

  it('after a real change to the project cards, the saved facts do not replace them', async () => {
    let resolve: (v: SavedBriefing) => void = () => {};
    mocked(projectsApi.savedBriefing).mockReturnValueOnce(new Promise<SavedBriefing>(r => { resolve = r; }));
    wrap();
    fireEvent.click(within(card('tracker')).getByRole('radio', { name: 'Linear' }));
    resolve(SAVED);
    await screen.findByTestId('briefing-late-prefill');
    expect(checked('tracker')).toBe('Linear');
    expect(document.querySelectorAll('textarea')[1]).toHaveValue(SAVED.team);
  });

  it('every project fact takes an optional note, sent trimmed with the answers', async () => {
    mocked(projectsApi.saveBriefing).mockResolvedValueOnce(true);
    wrap();
    const textareas = document.querySelectorAll('textarea');
    ['P', 'T', 'M', 'D', 'X'].forEach((v, i) => fireEvent.change(textareas[i], { target: { value: v } }));
    // The key and repository notes show with their field.
    expect(document.querySelector('input[data-note="tracker_key"]')).toBeNull();
    fireEvent.click(within(card('tracker')).getByRole('radio', { name: 'Jira' }));
    fireEvent.click(within(card('forge')).getByRole('radio', { name: 'GitHub' }));
    for (const fact of ALL_FACTS) {
      expect(note(fact)).toHaveAttribute('maxLength', '300');
      expect(note(fact).getAttribute('aria-label')).toMatch(/^Note \(facultatif\) — /);
      fireEvent.change(note(fact), { target: { value: `  how ${fact}  ` } });
    }
    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalledTimes(1));
    expect(payload().project.notes).toEqual(Object.fromEntries(ALL_FACTS.map(f => [f, `how ${f}`])));
  });
});

describe('briefing prefill helpers (KT-1196)', () => {
  it('cleanFactNotes drops empty notes and notes on a field the answers no longer carry', () => {
    const project = {
      ...emptyProjectAnswers(),
      tracker: 'github_issues' as const,
      notes: { tracker_key: 'stale', forge_repo: 'stale', review: '  ok  ', people: '   ' },
    };
    expect(cleanFactNotes(project)).toEqual({ review: 'ok' });
    expect(cleanFactNotes(emptyProjectAnswers())).toBeUndefined();
  });

  it('projectUnanswered is true only while no card and no note is filled', () => {
    expect(projectUnanswered(emptyProjectAnswers())).toBe(true);
    expect(projectUnanswered({ ...emptyProjectAnswers(), notes: { review: '  ' } })).toBe(true);
    expect(projectUnanswered({ ...emptyProjectAnswers(), notes: { review: 'x' } })).toBe(false);
    expect(projectUnanswered({ ...emptyProjectAnswers(), merge_strategy: 'squash' })).toBe(false);
  });

  it('mergeSavedDocs keeps the saved order and answers, and offers new locations unchecked', () => {
    const rows = mergeSavedDocs(DETECTED, [
      { path: 'docs', kind: 'docs', state: 'old', note: 'n', source_of_truth: false, last_commit: null },
      { path: 'gone.md', kind: 'markdown', state: 'partial', note: '', source_of_truth: false, last_commit: '2020-01-01' },
    ]);
    expect(rows.map(r => [r.path, r.included, r.state])).toEqual([
      ['docs', true, 'old'], ['gone.md', true, 'partial'], ['README.md', false, 'unknown'],
    ]);
    expect(rows[0].is_dir).toBe(true);
    expect(rows[1].last_commit).toBe('2020-01-01');
    expect(mergeSavedDocs(DETECTED, null).every(r => r.included)).toBe(true);
  });
});
