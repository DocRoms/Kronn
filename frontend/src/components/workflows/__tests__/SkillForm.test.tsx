import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import { SkillForm } from '../SkillForm';
import { skillBody } from '../../../lib/automationSkills';
import type { Project, Skill } from '../../../types/generated';

const PROJECTS = [
  { id: 'p-front', name: 'front_euronews' },
  { id: 'p-api', name: 'api' },
] as unknown as Project[];

const renderForm = (props: Partial<React.ComponentProps<typeof SkillForm>> = {}) => {
  const onSave = props.onSave ?? vi.fn();
  render(
    <I18nProvider>
      <SkillForm projects={PROJECTS} onSave={onSave} onCancel={vi.fn()} {...props} />
    </I18nProvider>,
  );
  return onSave;
};

afterEach(cleanup);

describe('SkillForm', () => {
  it('starts on the project picked by the page and says where the skill will be offered', () => {
    renderForm({ initialProjectId: 'p-front' });
    expect(screen.getByRole('combobox', { name: /^Projet/ })).toHaveValue('p-front');
    expect(screen.getByText(/uniquement dans ce projet/)).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: /^Projet/ }), { target: { value: '' } });
    expect(screen.getByText('Proposé dans tous les projets.')).toBeInTheDocument();
  });

  it('shows a refusal in place and keeps what was typed', async () => {
    const onSave = vi.fn().mockRejectedValue(new Error('Project not found'));
    renderForm({ onSave });
    fireEvent.change(screen.getByRole('textbox', { name: 'Nom *' }), { target: { value: 'Review' } });
    fireEvent.change(screen.getByRole('textbox', { name: 'Contenu (system prompt) *' }), { target: { value: 'Body.' } });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Ajouter un skill' })); });
    expect(screen.getByRole('alert')).toHaveTextContent('Project not found');
    expect(screen.getByRole('textbox', { name: 'Nom *' })).toHaveValue('Review');
  });

  it('sends one save for two synchronous clicks, and can save again after a refusal', async () => {
    let refuse: (error: Error) => void = () => {};
    const onSave = vi.fn()
      .mockImplementationOnce(() => new Promise<void>((_, reject) => { refuse = reject; }))
      .mockResolvedValueOnce(undefined);
    renderForm({ onSave });
    fireEvent.change(screen.getByRole('textbox', { name: 'Nom *' }), { target: { value: 'Review' } });
    fireEvent.change(screen.getByRole('textbox', { name: 'Contenu (system prompt) *' }), { target: { value: 'Body.' } });
    const button = screen.getByRole('button', { name: 'Ajouter un skill' });
    act(() => { button.click(); button.click(); });
    expect(onSave).toHaveBeenCalledTimes(1);

    await act(async () => { refuse(new Error('Project not found')); });
    expect(screen.getByRole('alert')).toHaveTextContent('Project not found');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Ajouter un skill' })); });
    expect(onSave).toHaveBeenCalledTimes(2);
  });

  it('edits the body without its frontmatter, which the backend writes from the fields', () => {
    const skill: Skill = {
      id: 'custom-review', name: 'Review', description: '', icon: '🔎', category: 'Domain',
      content: '---\nname: Review\n---\nRead every line.', is_builtin: false, token_estimate: 1,
    };
    renderForm({ editSkill: skill });
    expect(screen.getByRole('textbox', { name: 'Contenu (system prompt) *' })).toHaveValue('Read every line.');
    expect(screen.getByRole('combobox', { name: /^Projet/ })).toHaveValue('');
    expect(skillBody('No header.')).toBe('No header.');
  });
});
