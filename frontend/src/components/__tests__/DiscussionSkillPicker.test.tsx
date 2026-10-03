// The shared skill picker of the new-discussion form and the discussion
// settings (KT-923): grouped, searchable, dynamic.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { Project, ProjectUsedSkill, Skill, SkillCategory } from '../../types/generated';

const { skillsList, usedSkills } = vi.hoisted(() => ({
  skillsList: vi.fn(),
  usedSkills: vi.fn(),
}));
vi.mock('../../lib/api', () => ({
  skills: { list: skillsList },
  projects: { usedSkills },
}));

import { DiscussionSkillPicker } from '../DiscussionSkillPicker';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);

const skill = (id: string, name: string, category: SkillCategory = 'Domain', description = ''): Skill => ({
  id,
  name,
  description,
  icon: '🔧',
  category,
  content: '',
  is_builtin: true,
  token_estimate: 0,
});

const CATALOG = [
  skill('rust', 'Rust', 'Language', 'Idiomatic Rust'),
  skill('security', 'Security', 'Domain', 'Threat modelling'),
  skill('seo', 'SEO', 'Business'),
  skill('devops', 'DevOps', 'Domain'),
];

const used = (over: Partial<ProjectUsedSkill> = {}): ProjectUsedSkill => ({
  project_id: 'p1',
  slug: 'block-migration',
  name: 'Block migration',
  root: '.agents/skills',
  relative_path: '.agents/skills/block-migration/SKILL.md',
  referenced: true,
  published: false,
  ...over,
});

const project = (id: string, name: string, defaults: string[] = []) => ({
  id, name, default_skill_ids: defaults,
}) as Project;

const PROJECTS = [project('p1', 'front_euronews', ['devops']), project('p2', 'other')];

interface Overrides {
  projectId?: string | null;
  selectedIds?: string[];
  onToggle?: (id: string) => void;
}

const renderPicker = (over: Overrides = {}) => {
  const props = {
    projectId: 'p1' as string | null,
    projects: PROJECTS,
    catalog: CATALOG,
    selectedIds: [] as string[],
    onToggle: vi.fn(),
    t,
    ...over,
  };
  const view = render(<DiscussionSkillPicker {...props} />);
  return { ...view, props, rerenderWith: (next: Overrides) => view.rerender(<DiscussionSkillPicker {...props} {...next} />) };
};

const settled = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
const section = (name: string) => document.querySelector(`[data-section="${name}"]`) as HTMLElement;

beforeEach(() => {
  skillsList.mockReset().mockResolvedValue(CATALOG);
  usedSkills.mockReset().mockResolvedValue([used()]);
});

describe('DiscussionSkillPicker — with a project', () => {
  it('lists what the project uses first, with where a repository skill lives', async () => {
    renderPicker();
    await settled();

    const usedSection = section('used');
    expect(within(usedSection).getByText('disc.skillsUsedByProject')).toBeInTheDocument();
    const chip = within(usedSection).getByRole('button', { name: /Block migration/ });
    expect(chip).toHaveTextContent('automation.skill.originRepository:.agents/skills');
    // The project's default skills are used as well.
    expect(within(usedSection).getByRole('button', { name: /DevOps/ })).toBeInTheDocument();
    // The used section comes before everything else.
    expect(usedSection.compareDocumentPosition(section('available')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('folds the rest of the catalog behind "Voir les skills disponibles (N)", by category', async () => {
    renderPicker();
    await settled();

    const toggle = screen.getByRole('button', { name: 'automation.skill.availableToggle:3' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText('Rust')).not.toBeInTheDocument();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const groups = [...document.querySelectorAll('.skill-picker-group')].map(group => group.getAttribute('data-category'));
    expect(groups).toEqual(['Language', 'Domain', 'Business']);
    expect(screen.getByRole('button', { name: /Rust/ })).toBeInTheDocument();
  });

  it('shows the ticked skills between the project\'s and the catalog', async () => {
    renderPicker({ selectedIds: ['rust', 'repository:p1:block-migration'] });
    await settled();

    const ticked = section('ticked');
    expect(within(ticked).getByRole('button', { name: /Rust/ })).toHaveAttribute('aria-pressed', 'true');
    // The repository skill is ticked in its own place, not listed twice.
    expect(within(ticked).queryByText(/Block migration/)).not.toBeInTheDocument();
    expect(within(section('used')).getByRole('button', { name: /Block migration/ })).toHaveAttribute('aria-pressed', 'true');
    expect(section('used').compareDocumentPosition(ticked) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(ticked.compareDocumentPosition(section('available')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('selects a repository skill under its stable id', async () => {
    const { props } = renderPicker();
    await settled();

    fireEvent.click(screen.getByRole('button', { name: /Block migration/ }));
    expect(props.onToggle).toHaveBeenCalledWith('repository:p1:block-migration');
  });

  it('a search on the name or the description opens what matches, folded skills included', async () => {
    renderPicker();
    await settled();
    const search = screen.getByRole('searchbox', { name: 'disc.skillSearch' });

    fireEvent.change(search, { target: { value: 'threat' } });
    expect(screen.getByRole('button', { name: /Security/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Block migration/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Rust/ })).not.toBeInTheDocument();

    fireEvent.change(search, { target: { value: 'block' } });
    expect(screen.getByRole('button', { name: /Block migration/ })).toBeInTheDocument();

    fireEvent.change(search, { target: { value: 'nothing-like-this' } });
    expect(screen.getByRole('status')).toHaveTextContent('disc.skillNoMatch:nothing-like-this');
  });

  it('follows the project the discussion moves to, without a new read being needed', async () => {
    usedSkills.mockResolvedValue([used(), used({ project_id: 'p2', slug: 'hero', name: 'Hero banner' })]);
    const { rerenderWith } = renderPicker();
    await settled();
    expect(screen.getByRole('button', { name: /Block migration/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Hero banner/ })).not.toBeInTheDocument();

    rerenderWith({ projectId: 'p2' });
    // Immediately: the lists in hand already answer for the new project.
    expect(screen.getByRole('button', { name: /Hero banner/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Block migration/ })).not.toBeInTheDocument();
  });

  it('reads the lists again each time it opens, so a fresh or a removed skill shows up', async () => {
    const first = renderPicker();
    await settled();
    expect(usedSkills).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: /Hero banner/ })).not.toBeInTheDocument();
    first.unmount();

    usedSkills.mockResolvedValue([used({ slug: 'hero', name: 'Hero banner' })]);
    renderPicker();
    await settled();
    expect(usedSkills).toHaveBeenCalledTimes(2);
    expect(skillsList).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('button', { name: /Hero banner/ })).toBeInTheDocument();
    // block-migration left the repository: it is gone from the list.
    expect(screen.queryByRole('button', { name: /Block migration/ })).not.toBeInTheDocument();
  });

  it('keeps a ticked skill that vanished visible, flagged, and lets it be removed', async () => {
    usedSkills.mockResolvedValue([]);
    const { props } = renderPicker({ selectedIds: ['repository:p1:block-migration'] });
    await settled();

    const gone = within(section('ticked')).getByRole('button', { name: /block-migration/ });
    expect(gone).toHaveAttribute('data-missing', 'true');
    expect(gone).toHaveAttribute('title', 'disc.skillUnavailable');
    fireEvent.click(gone);
    expect(props.onToggle).toHaveBeenCalledWith('repository:p1:block-migration');
  });

  it('keeps the lists in hand when a read fails', async () => {
    usedSkills.mockRejectedValue(new Error('offline'));
    skillsList.mockRejectedValue(new Error('offline'));
    renderPicker();
    await settled();

    // The catalog the caller passed stays; the project's default skill too.
    expect(screen.getByRole('button', { name: /DevOps/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'automation.skill.availableToggle:3' }));
    expect(screen.getByRole('button', { name: /Rust/ })).toBeInTheDocument();
  });
});

describe('DiscussionSkillPicker — without a project', () => {
  it('shows the catalog by category, unfolded', async () => {
    renderPicker({ projectId: null });
    await settled();

    expect(screen.queryByText('disc.skillsUsedByProject')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /availableToggle/ })).not.toBeInTheDocument();
    const groups = [...document.querySelectorAll('.skill-picker-group')].map(group => group.getAttribute('data-category'));
    expect(groups).toEqual(['Language', 'Domain', 'Business']);
    await waitFor(() => expect(screen.getByRole('button', { name: /Rust/ })).toBeInTheDocument());
    // No project, so no repository skill either.
    expect(screen.queryByRole('button', { name: /Block migration/ })).not.toBeInTheDocument();
  });
});
