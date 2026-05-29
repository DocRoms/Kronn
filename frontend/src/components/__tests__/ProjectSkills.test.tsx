/**
 * ProjectSkills coverage (97 LOC, presentational + 1 toggle handler).
 *
 * Renders one chip per skill, highlights the active ones (membership in
 * `currentSkillIds`), shows category badge + icon, an "External" badge when
 * `skill.external`, and an empty-state line when `allSkills` is empty.
 * Clicking a chip toggles membership via projectsApi.setDefaultSkills then
 * calls onUpdate. A synchronous re-entry guard blocks a second click while
 * the first round-trip is in flight.
 *
 * Pins :
 *  - empty state when allSkills is empty (no chips, calls noSkills label)
 *  - renders a chip per skill with icon + name + category badge
 *  - external badge only on skills flagged external
 *  - clicking an inactive chip ADDS it then calls onUpdate
 *  - clicking an active chip REMOVES it then calls onUpdate
 *  - unknown category falls back without crashing
 *  - re-entry guard: a second click during the in-flight POST is dropped
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, act } from '@testing-library/react';
import type { Skill } from '../../types/generated';

const { projects } = vi.hoisted(() => ({
  projects: { setDefaultSkills: vi.fn() },
}));

vi.mock('../../lib/api', () => ({ projects }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (k: string, ...args: (string | number)[]) =>
      args.length ? `${k}(${args.join('|')})` : k,
    locale: 'en-US',
  }),
}));

import { ProjectSkills } from '../ProjectSkills';

function makeSkill(over: Partial<Skill> & Pick<Skill, 'id' | 'name'>): Skill {
  return {
    description: '',
    icon: '🧩',
    category: 'Language',
    content: '',
    is_builtin: false,
    token_estimate: 0,
    license: null,
    allowed_tools: null,
    external: false,
    source_url: null,
    ...over,
  } as Skill;
}

const SKILL_ALPHA = makeSkill({ id: 's-alpha', name: 'SkillAlpha', icon: '🅰️', category: 'Language' });
const SKILL_BETA = makeSkill({ id: 's-beta', name: 'SkillBeta', icon: '🅱️', category: 'Business' });
const SKILL_GAMMA = makeSkill({ id: 's-gamma', name: 'SkillGamma', icon: '🌐', category: 'Domain', external: true, source_url: 'https://example.test/repo' });

beforeEach(() => {
  projects.setDefaultSkills.mockReset();
  projects.setDefaultSkills.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
});

describe('ProjectSkills — empty state', () => {
  it('shows the noSkills label and no chips when allSkills is empty', () => {
    render(
      <ProjectSkills projectId="p-1" currentSkillIds={[]} allSkills={[]} onUpdate={vi.fn()} />,
    );
    expect(screen.getByText('projects.noSkills')).toBeDefined();
    expect(screen.queryAllByRole('button')).toHaveLength(0);
  });
});

describe('ProjectSkills — rendering', () => {
  it('renders one chip per skill with name, icon and category badge', () => {
    render(
      <ProjectSkills
        projectId="p-1"
        currentSkillIds={[]}
        allSkills={[SKILL_ALPHA, SKILL_BETA, SKILL_GAMMA]}
        onUpdate={vi.fn()}
      />,
    );
    const buttons = screen.getAllByRole('button');
    expect(buttons).toHaveLength(3);

    expect(screen.getByText('SkillAlpha')).toBeDefined();
    expect(screen.getByText('SkillBeta')).toBeDefined();
    expect(screen.getByText('SkillGamma')).toBeDefined();

    // Icons rendered.
    expect(screen.getByText('🅰️')).toBeDefined();
    expect(screen.getByText('🌐')).toBeDefined();

    // Category badges.
    expect(screen.getByText('Language')).toBeDefined();
    expect(screen.getByText('Business')).toBeDefined();
    expect(screen.getByText('Domain')).toBeDefined();
  });

  it('shows the External badge only on skills flagged external', () => {
    render(
      <ProjectSkills
        projectId="p-1"
        currentSkillIds={[]}
        allSkills={[SKILL_ALPHA, SKILL_GAMMA]}
        onUpdate={vi.fn()}
      />,
    );
    const externalBadges = screen.getAllByText(/External/);
    expect(externalBadges).toHaveLength(1);
    expect(externalBadges[0].getAttribute('title')).toContain('https://example.test/repo');
  });

  it('uses the generic external title when no source_url is provided', () => {
    const noUrl = makeSkill({ id: 's-x', name: 'SkillNoUrl', external: true, source_url: null });
    render(
      <ProjectSkills projectId="p-1" currentSkillIds={[]} allSkills={[noUrl]} onUpdate={vi.fn()} />,
    );
    const badge = screen.getByText(/External/);
    expect(badge.getAttribute('title')).toBe('Vendored from a third-party project');
  });

  it('applies active styling (bold name) to skills in currentSkillIds', () => {
    render(
      <ProjectSkills
        projectId="p-1"
        currentSkillIds={[SKILL_ALPHA.id]}
        allSkills={[SKILL_ALPHA, SKILL_BETA]}
        onUpdate={vi.fn()}
      />,
    );
    const activeName = screen.getByText('SkillAlpha');
    const inactiveName = screen.getByText('SkillBeta');
    expect(activeName.style.fontWeight).toBe('600');
    expect(inactiveName.style.fontWeight).toBe('400');
  });

  it('renders an unknown category without crashing (fallback color path)', () => {
    const weird = makeSkill({ id: 's-weird', name: 'SkillWeird', category: 'Mystery' as Skill['category'] });
    render(
      <ProjectSkills projectId="p-1" currentSkillIds={[]} allSkills={[weird]} onUpdate={vi.fn()} />,
    );
    expect(screen.getByText('SkillWeird')).toBeDefined();
    expect(screen.getByText('Mystery')).toBeDefined();
  });
});

describe('ProjectSkills — toggle handler', () => {
  it('adds an inactive skill then calls onUpdate', async () => {
    const onUpdate = vi.fn();
    render(
      <ProjectSkills
        projectId="p-1"
        currentSkillIds={[SKILL_BETA.id]}
        allSkills={[SKILL_ALPHA, SKILL_BETA]}
        onUpdate={onUpdate}
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByText('SkillAlpha'));
    });
    await waitFor(() =>
      expect(projects.setDefaultSkills).toHaveBeenCalledWith('p-1', [SKILL_BETA.id, SKILL_ALPHA.id]),
    );
    await waitFor(() => expect(onUpdate).toHaveBeenCalledTimes(1));
  });

  it('removes an active skill then calls onUpdate', async () => {
    const onUpdate = vi.fn();
    render(
      <ProjectSkills
        projectId="p-1"
        currentSkillIds={[SKILL_ALPHA.id, SKILL_BETA.id]}
        allSkills={[SKILL_ALPHA, SKILL_BETA]}
        onUpdate={onUpdate}
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByText('SkillAlpha'));
    });
    await waitFor(() =>
      expect(projects.setDefaultSkills).toHaveBeenCalledWith('p-1', [SKILL_BETA.id]),
    );
    await waitFor(() => expect(onUpdate).toHaveBeenCalledTimes(1));
  });

  it('drops a second click while the first round-trip is in flight (re-entry guard)', async () => {
    let resolveFirst: (() => void) | undefined;
    projects.setDefaultSkills.mockImplementationOnce(
      () => new Promise<void>(res => { resolveFirst = () => res(); }),
    );
    const onUpdate = vi.fn();
    render(
      <ProjectSkills
        projectId="p-1"
        currentSkillIds={[]}
        allSkills={[SKILL_ALPHA, SKILL_BETA]}
        onUpdate={onUpdate}
      />,
    );

    // First click starts an in-flight (unresolved) POST.
    fireEvent.click(screen.getByText('SkillAlpha'));
    // Second click should be swallowed by togglingRef.
    fireEvent.click(screen.getByText('SkillBeta'));

    expect(projects.setDefaultSkills).toHaveBeenCalledTimes(1);

    // Resolve the first call → guard releases, onUpdate fires once.
    await act(async () => {
      resolveFirst?.();
    });
    await waitFor(() => expect(onUpdate).toHaveBeenCalledTimes(1));

    // A subsequent click now goes through (guard released).
    await act(async () => {
      fireEvent.click(screen.getByText('SkillBeta'));
    });
    await waitFor(() => expect(projects.setDefaultSkills).toHaveBeenCalledTimes(2));
  });
});
