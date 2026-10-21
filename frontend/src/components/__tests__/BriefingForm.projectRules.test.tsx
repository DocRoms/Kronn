// KT-1193 — the briefing records the team's rules and bans; a ban may carry
// a pattern Kronn checks at each audit.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { BriefingForm } from '../BriefingForm';
import { projects as projectsApi } from '../../lib/api';
import { rowsToRules, RULES_MAX } from '../briefingRules';

const mocked = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

const wrap = () =>
  render(
    <I18nProvider>
      <BriefingForm
        projectId="p-rules"
        onClose={() => {}}
        onSaved={() => {}}
        agent="ClaudeCode"
        tier="reasoning"
        review={false}
        toast={() => {}}
      />
    </I18nProvider>,
  );

const fillMandatory = () => {
  const textareas = document.querySelectorAll('textarea');
  ['Purpose', 'Team', 'Maturity', 'Deps', 'Traps'].forEach((v, i) =>
    fireEvent.change(textareas[i], { target: { value: v } }),
  );
};

const submit = () => fireEvent.click(screen.getByRole('button', { name: /^(Save|Enregistrer|Guardar)$/i }));
const section = () => screen.getByTestId('briefing-rules');
const add = () => fireEvent.click(screen.getByTestId('briefing-rules-add'));

beforeEach(() => {
  vi.clearAllMocks();
  mocked(projectsApi.briefingDocs).mockResolvedValue([]);
  mocked(projectsApi.saveBriefing).mockResolvedValue(true);
});

describe('BriefingForm rules and bans (KT-1193)', () => {
  it('saves a rule and a ban with its pattern', async () => {
    wrap();
    fillMandatory();
    add();
    add();
    const s = within(section());
    // A new line starts as a ban; the first one becomes a rule.
    fireEvent.click(within(s.getByRole('radiogroup', { name: 'Ligne 1 : toujours ou jamais' }))
      .getByRole('radio', { name: 'Toujours' }));
    fireEvent.change(s.getByRole('textbox', { name: 'Ligne 1' }), { target: { value: ' toujours passer par le service X ' } });
    // A rule offers no pattern.
    expect(s.queryByRole('combobox', { name: 'Motif vérifiable de la ligne 1' })).toBeNull();
    fireEvent.change(s.getByRole('textbox', { name: 'Ligne 2' }), { target: { value: 'ne jamais utiliser lodash' } });
    fireEvent.change(s.getByRole('combobox', { name: 'Motif vérifiable de la ligne 2' }), { target: { value: 'dependency' } });
    fireEvent.change(s.getByRole('textbox', { name: 'Motif de la ligne 2' }), { target: { value: 'lodash' } });
    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalled());
    expect(mocked(projectsApi.saveBriefing).mock.calls[0][1].project_rules).toEqual({
      rules: [
        { kind: 'rule', text: 'toujours passer par le service X', pattern: null },
        { kind: 'ban', text: 'ne jamais utiliser lodash', pattern: { kind: 'dependency', value: 'lodash' } },
      ],
    });
  });

  it('removes a line and sends no rule when none is left', async () => {
    wrap();
    fillMandatory();
    add();
    const s = within(section());
    fireEvent.change(s.getByRole('textbox', { name: 'Ligne 1' }), { target: { value: 'jamais X' } });
    fireEvent.click(s.getByRole('button', { name: 'Retirer la ligne 1' }));
    expect(s.queryByRole('textbox', { name: 'Ligne 1' })).toBeNull();
    submit();
    await waitFor(() => expect(projectsApi.saveBriefing).toHaveBeenCalled());
    expect(mocked(projectsApi.saveBriefing).mock.calls[0][1].project_rules).toEqual({ rules: [] });
  });

  it('stops at the maximum number of lines', () => {
    wrap();
    for (let i = 0; i < RULES_MAX + 2; i++) add();
    expect(within(section()).getAllByRole('radiogroup')).toHaveLength(RULES_MAX);
    expect((screen.getByTestId('briefing-rules-add') as HTMLButtonElement).disabled).toBe(true);
  });

  it('drops empty lines and a pattern left on a rule', () => {
    expect(rowsToRules([
      { id: 1, kind: 'rule', text: '  ', patternKind: null, pattern: '' },
      { id: 2, kind: 'rule', text: 'always', patternKind: 'import', pattern: 'x' },
      { id: 3, kind: 'ban', text: 'never', patternKind: 'file_glob', pattern: '  ' },
    ])).toEqual([
      { kind: 'rule', text: 'always', pattern: null },
      { kind: 'ban', text: 'never', pattern: null },
    ]);
  });
});
