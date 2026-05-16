// 0.8.5 — QuickPromptForm bindings (skills · profiles · directives).
//
// Pins the new binding pickers added by Phase 2:
//   - hidden when no catalogs are provided (graceful degradation)
//   - rendered when at least one catalog is non-empty
//   - accordion: only one section open at a time
//   - chip click toggles selection; submit forwards the picked ids in
//     skill_ids / profile_ids / directive_ids on the save payload
//
// Mocks the api module + uses I18nProvider so the picker labels resolve.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import type { ReactElement } from 'react';
import type { Skill, AgentProfile, Directive } from '../../../types/generated';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

import { QuickPromptForm } from '../QuickPromptForm';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const sampleSkills: Skill[] = [
  { id: 's1', name: 'Security', description: 'Sec audit', icon: '🔒', category: 'Domain', content: '', is_builtin: true, token_estimate: 100, license: null } as Skill,
  { id: 's2', name: 'Perf',     description: 'Perf audit', icon: '⚡', category: 'Domain', content: '', is_builtin: true, token_estimate: 100, license: null } as Skill,
];
const sampleProfiles: AgentProfile[] = [
  { id: 'p1', name: 'coder', persona_name: 'Codeur', role: 'Senior dev', avatar: '🧑‍💻', color: '#c8ff00', category: 'Technical', persona_prompt: '', is_builtin: true, token_estimate: 200 } as AgentProfile,
];
const sampleDirectives: Directive[] = [
  { id: 'd1', name: 'concise', description: 'Be brief', icon: '✂️', category: 'Output', content: '', is_builtin: true, token_estimate: 50 } as Directive,
];

beforeEach(() => { vi.clearAllMocks(); });

describe('QuickPromptForm bindings (0.8.5)', () => {
  it('hides the bindings block when no catalogs are provided', () => {
    wrap(
      <QuickPromptForm
        projects={[]}
        onSave={vi.fn()}
        onCancel={() => {}}
      />
    );
    expect(screen.queryByTestId('qp-bindings')).toBeNull();
  });

  it('renders the bindings block with one toggle per non-empty catalog', () => {
    wrap(
      <QuickPromptForm
        projects={[]}
        skills={sampleSkills}
        profiles={sampleProfiles}
        directives={sampleDirectives}
        onSave={vi.fn()}
        onCancel={() => {}}
      />
    );
    expect(screen.getByTestId('qp-bindings')).toBeInTheDocument();
    expect(screen.getByTestId('qp-bindings-skills-toggle')).toBeInTheDocument();
    expect(screen.getByTestId('qp-bindings-profiles-toggle')).toBeInTheDocument();
    expect(screen.getByTestId('qp-bindings-directives-toggle')).toBeInTheDocument();
  });

  it('opens one section at a time (accordion)', () => {
    wrap(
      <QuickPromptForm
        projects={[]}
        skills={sampleSkills}
        profiles={sampleProfiles}
        directives={sampleDirectives}
        onSave={vi.fn()}
        onCancel={() => {}}
      />
    );
    fireEvent.click(screen.getByTestId('qp-bindings-skills-toggle'));
    expect(screen.getByTestId('qp-bindings-skills-chips')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('qp-bindings-profiles-toggle'));
    // Switching sections closes the previous one.
    expect(screen.queryByTestId('qp-bindings-skills-chips')).toBeNull();
    expect(screen.getByTestId('qp-bindings-profiles-chips')).toBeInTheDocument();
  });

  it('forwards the picked skill / profile / directive ids on save', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);
    wrap(
      <QuickPromptForm
        projects={[]}
        skills={sampleSkills}
        profiles={sampleProfiles}
        directives={sampleDirectives}
        onSave={onSave}
        onCancel={() => {}}
      />
    );
    // Fill required fields (name + prompt template).
    const [iconInput, nameInput] = screen.getAllByRole('textbox');
    void iconInput; // keep the destructure positional
    fireEvent.change(nameInput, { target: { value: 'My QP' } });
    const promptArea = screen.getByPlaceholderText(/Analyse le ticket|Analyse the ticket|Analiza el ticket/i);
    fireEvent.change(promptArea, { target: { value: 'Hello {{name}}' } });

    // Open each section and pick the first chip.
    fireEvent.click(screen.getByTestId('qp-bindings-skills-toggle'));
    fireEvent.click(screen.getByRole('button', { name: /Security/i }));
    fireEvent.click(screen.getByTestId('qp-bindings-profiles-toggle'));
    fireEvent.click(screen.getByRole('button', { name: /Codeur|coder/i }));
    fireEvent.click(screen.getByTestId('qp-bindings-directives-toggle'));
    fireEvent.click(screen.getByRole('button', { name: /concise/i }));

    // Save.
    fireEvent.click(screen.getByRole('button', { name: /qp.save|Save|Enregistrer|Guardar/i }));

    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    const payload = onSave.mock.calls[0][0];
    expect(payload.skill_ids).toEqual(['s1']);
    expect(payload.profile_ids).toEqual(['p1']);
    expect(payload.directive_ids).toEqual(['d1']);
  });

  it('initial state respects editPrompt bindings', () => {
    wrap(
      <QuickPromptForm
        editPrompt={{
          id: 'qp-x', name: 'Edit me', icon: '⚡', prompt_template: 'foo',
          variables: [], agent: 'ClaudeCode',
          skill_ids: ['s1'], profile_ids: ['p1'], directive_ids: ['d1'],
          tier: 'default', description: '',
          created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
        }}
        projects={[]}
        skills={sampleSkills}
        profiles={sampleProfiles}
        directives={sampleDirectives}
        onSave={vi.fn()}
        onCancel={() => {}}
      />
    );
    // Counts on the closed toggle reflect prefilled bindings.
    const skillsToggle = screen.getByTestId('qp-bindings-skills-toggle');
    const profilesToggle = screen.getByTestId('qp-bindings-profiles-toggle');
    const directivesToggle = screen.getByTestId('qp-bindings-directives-toggle');
    expect(skillsToggle.textContent).toMatch(/1/);
    expect(profilesToggle.textContent).toMatch(/1/);
    expect(directivesToggle.textContent).toMatch(/1/);
  });
});
