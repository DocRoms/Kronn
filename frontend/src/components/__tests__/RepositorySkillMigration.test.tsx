import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import en from '../../lib/i18n/locales/en';
import fr from '../../lib/i18n/locales/fr';
import es from '../../lib/i18n/locales/es';
import zh from '../../lib/i18n/locales/zh';
import type { SkillMigrationPlan, SkillMigrationResult } from '../../types/generated';
import { listing } from './repositoryResourceFixtures';

const repositoryResources = vi.hoisted(() => vi.fn());
const skillMigrationPlan = vi.hoisted(() => vi.fn());
const migrateSkills = vi.hoisted(() => vi.fn());

vi.mock('../../lib/api', () => ({
  projects: {
    repositoryResources,
    skillMigrationPlan,
    migrateSkills,
    repositoryResourceComparison: vi.fn(),
    publishRepositoryResource: vi.fn(),
    importRepositoryResource: vi.fn(),
    approveRepositoryResource: vi.fn(),
    useNativeSkill: vi.fn(),
    copyNativeSkill: vi.fn(),
    setDefaultSkills: vi.fn(),
  },
  quickExecs: { list: vi.fn(), delete: vi.fn() },
  quickApis: { list: vi.fn(), delete: vi.fn() },
  quickPrompts: { list: vi.fn(), delete: vi.fn() },
  workflows: { get: vi.fn(), delete: vi.fn() },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) => (args.length > 0 ? `${key}:${args.join('|')}` : key),
  }),
}));

import { ProjectRepositoryResourcesPanel } from '../ProjectRepositoryResourcesPanel';
import { RepositorySkillMigration } from '../RepositorySkillMigration';

const M = 'projects.repositoryResources.skillMigration.';

const plan = (overrides: Partial<SkillMigrationPlan> = {}): SkillMigrationPlan => ({
  target_root: '.agents/skills',
  moves: [
    { slug: 'lint', source: '.gemini/skills/lint', target: '.agents/skills/lint', action: 'move', converted: false, kronn_managed: false },
    { slug: 'rust', source: 'kronn/skills/rust', target: '.agents/skills/rust', action: 'move', converted: true, kronn_managed: true },
    { slug: 'triage', source: '.cursor/skills/triage', target: '.agents/skills/triage', action: 'duplicate', converted: false, kronn_managed: false },
  ],
  conflicts: [{
    slug: 'review',
    target: '.agents/skills/review',
    versions: [
      { fingerprint: 'aaaaaaaa', paths: ['.agents/skills/review'], at_target: true },
      { fingerprint: 'bbbbbbbb', paths: ['.claude/skills/review'], at_target: false },
    ],
  }],
  blocked: [{ path: '.claude/skills/linked', reason: 'symlink' }],
  ...overrides,
});

const result = (overrides: Partial<SkillMigrationResult> = {}): SkillMigrationResult => ({
  moved: plan().moves,
  unresolved: [],
  kept: [],
  blocked: [],
  ...overrides,
});

describe('RepositorySkillMigration', () => {
  beforeEach(() => {
    [repositoryResources, skillMigrationPlan, migrateSkills].forEach(mock => mock.mockReset());
  });

  const open = async (props: Partial<Parameters<typeof RepositorySkillMigration>[0]> = {}) => {
    const onClose = vi.fn();
    render(<RepositorySkillMigration projectId="project-1" canWrite onClose={onClose} {...props} />);
    await screen.findByTestId('skill-migration-moves');
    return onClose;
  };

  it('lists every move as source → target, and every conflict, before anything is written', async () => {
    skillMigrationPlan.mockResolvedValue(plan());
    await open();

    const moves = within(screen.getByTestId('skill-migration-moves'));
    for (const [source, target] of [
      ['.gemini/skills/lint/', '.agents/skills/lint/'],
      ['kronn/skills/rust/', '.agents/skills/rust/'],
      ['.cursor/skills/triage/', '.agents/skills/triage/'],
    ]) {
      const line = moves.getByText(source).closest('li') as HTMLElement;
      expect(within(line).getByText(target)).toBeInTheDocument();
    }
    const rust = moves.getByText('kronn/skills/rust/').closest('li') as HTMLElement;
    expect(within(rust).getByText(`${M}converted`)).toBeInTheDocument();
    expect(within(rust).getByText(`${M}managed`)).toBeInTheDocument();
    const triage = moves.getByText('.cursor/skills/triage/').closest('li') as HTMLElement;
    expect(within(triage).getByText(`${M}duplicate`)).toBeInTheDocument();

    const conflicts = within(screen.getByTestId('skill-migration-conflicts'));
    expect(conflicts.getByText('review')).toBeInTheDocument();
    expect(conflicts.getByText('.claude/skills/review/')).toBeInTheDocument();
    // The target is named in the heading and again as the version already there.
    expect(conflicts.getAllByText('.agents/skills/review/')).toHaveLength(2);
    expect(within(screen.getByTestId('skill-migration-blocked')).getByText(`${M}reason.symlink`)).toBeInTheDocument();
    expect(screen.getByText(`${M}noCommit`)).toBeInTheDocument();
    expect(migrateSkills).not.toHaveBeenCalled();
  });

  it('picks no version for a conflict: skipped, and the request carries no resolution', async () => {
    skillMigrationPlan.mockResolvedValue(plan());
    migrateSkills.mockResolvedValue(result({ unresolved: ['review'] }));
    await open();

    expect(screen.getByTestId('skill-migration-skipped')).toHaveTextContent(`${M}skipped:1`);
    const skip = within(screen.getByTestId('skill-migration-conflicts')).getByRole('radio', { name: `${M}skip` });
    expect(skip).toBeChecked();
    fireEvent.click(screen.getByRole('button', { name: `${M}confirm:3` }));

    await waitFor(() => expect(migrateSkills).toHaveBeenCalledWith('project-1', { resolutions: [] }));
    expect(await screen.findByTestId('skill-migration-done')).toHaveTextContent(`${M}done.unresolved:1`);
  });

  it('sends the version the user chose for a conflict, and only that', async () => {
    skillMigrationPlan.mockResolvedValue(plan());
    migrateSkills.mockResolvedValue(result());
    await open();

    const conflicts = within(screen.getByTestId('skill-migration-conflicts'));
    const claude = conflicts.getByText('.claude/skills/review/').closest('label') as HTMLElement;
    fireEvent.click(within(claude).getByRole('radio'));
    expect(screen.queryByTestId('skill-migration-skipped')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: `${M}confirm:4` }));

    await waitFor(() => expect(migrateSkills).toHaveBeenCalledWith('project-1', {
      resolutions: [{ slug: 'review', keep: '.claude/skills/review' }],
    }));
    expect(await screen.findByTestId('skill-migration-done')).toHaveTextContent(`${M}done.moved:3`);
    expect(screen.getByText(`${M}done.notCommitted`)).toBeInTheDocument();
  });

  it('a choice can be taken back: the conflict is skipped again', async () => {
    skillMigrationPlan.mockResolvedValue(plan());
    await open();

    const conflicts = within(screen.getByTestId('skill-migration-conflicts'));
    const claude = conflicts.getByText('.claude/skills/review/').closest('label') as HTMLElement;
    fireEvent.click(within(claude).getByRole('radio'));
    fireEvent.click(conflicts.getByRole('radio', { name: `${M}skip` }));
    expect(screen.getByTestId('skill-migration-skipped')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: `${M}confirm:3` })).toBeEnabled();
  });

  it('cannot confirm where the repository cannot be written, nor with nothing to do', async () => {
    skillMigrationPlan.mockResolvedValue(plan());
    await open({ canWrite: false });
    expect(screen.getByRole('button', { name: `${M}confirm:3` })).toBeDisabled();
  });

  it('says so when there is nothing to migrate', async () => {
    skillMigrationPlan.mockResolvedValue(plan({ moves: [], conflicts: [], blocked: [] }));
    render(<RepositorySkillMigration projectId="project-1" canWrite onClose={vi.fn()} />);
    expect(await screen.findByText(`${M}empty`)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: `${M}confirm:0` })).toBeDisabled();
  });

  it('shows a failed plan or a failed move, and closing tells whether files moved', async () => {
    skillMigrationPlan.mockRejectedValue(new Error('boom'));
    const onClose = vi.fn();
    const { unmount } = render(<RepositorySkillMigration projectId="project-1" canWrite onClose={onClose} />);
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'common.cancel' }));
    expect(onClose).toHaveBeenCalledWith(false);
    unmount();

    skillMigrationPlan.mockResolvedValue(plan());
    migrateSkills.mockRejectedValue(new Error('cannot write'));
    const closed = await open();
    fireEvent.click(screen.getByRole('button', { name: `${M}confirm:3` }));
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(screen.queryByTestId('skill-migration-done')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'common.cancel' }));
    expect(closed).toHaveBeenCalledWith(false);
  });
});

describe('the "Migrate everything to .agents/skills" offer', () => {
  beforeEach(() => {
    [repositoryResources, skillMigrationPlan, migrateSkills].forEach(mock => mock.mockReset());
    localStorage.removeItem('kronn:projectRepositoryResourcesTab');
  });

  const show = async (data = listing()) => {
    repositoryResources.mockResolvedValue(data);
    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);
    await screen.findByRole('tablist');
    fireEvent.click(screen.getByRole('tab', { name: /tab\.skills/ }));
  };

  it('appears when skills sit in .claude/skills, .gemini/skills or kronn/skills', async () => {
    await show(listing({
      skill_roots: [
        { path: '.agents/skills', skill_count: 3 },
        { path: '.claude/skills', skill_count: 2 },
        { path: 'kronn/skills', skill_count: 1 },
      ],
    }));
    const offer = screen.getByTestId('skill-migration-offer');
    expect(within(offer).getByRole('button', { name: `${M}button:3` })).toBeEnabled();
  });

  it('is not offered when every skill is already in .agents/skills', async () => {
    await show(listing({ skill_roots: [{ path: '.agents/skills', skill_count: 3 }] }));
    expect(screen.queryByTestId('skill-migration-offer')).not.toBeInTheDocument();
  });

  it('is disabled where the repository cannot be written', async () => {
    await show(listing({
      can_write_repository: false,
      can_write_repository_reason: 'repository_read_only',
      skill_roots: [{ path: '.claude/skills', skill_count: 2 }],
    }));
    expect(within(screen.getByTestId('skill-migration-offer')).getByRole('button')).toBeDisabled();
  });

  it('opens the recap, then reads the listing again once files moved, and the banner counts them', async () => {
    const before = listing({ skill_roots: [{ path: '.claude/skills', skill_count: 2 }] });
    const after = listing({
      skill_roots: [{ path: '.agents/skills', skill_count: 2 }],
      uncommitted_managed_paths: ['.agents/skills/a/SKILL.md', '.agents/skills/b/SKILL.md'],
    });
    skillMigrationPlan.mockResolvedValue(plan({ conflicts: [], blocked: [] }));
    migrateSkills.mockResolvedValue(result());
    await show(before);

    fireEvent.click(screen.getByRole('button', { name: `${M}button:2` }));
    await screen.findByTestId('skill-migration-moves');
    expect(screen.queryByText(new RegExp('banner\\.uncommitted'))).not.toBeInTheDocument();
    repositoryResources.mockResolvedValue(after);
    fireEvent.click(screen.getByRole('button', { name: `${M}confirm:3` }));
    await screen.findByTestId('skill-migration-done');
    fireEvent.click(screen.getAllByRole('button', { name: 'common.close' }).at(-1) as HTMLElement);

    await waitFor(() => expect(screen.queryByTestId('skill-migration')).not.toBeInTheDocument());
    expect(await screen.findByText('projects.repositoryResources.banner.uncommitted:2')).toBeInTheDocument();
    expect(screen.queryByTestId('skill-migration-offer')).not.toBeInTheDocument();
  });
});

describe('the migration wording', () => {
  const keys = Object.keys(en).filter(key => key.startsWith(M));

  it('is worded in the four languages, with the same placeholders', () => {
    expect(keys.length).toBeGreaterThan(20);
    for (const [name, dictionary] of Object.entries({ fr, es, zh })) {
      for (const key of keys) {
        const text = (dictionary as Record<string, string>)[key];
        expect(text, `${name} lacks ${key}`).toBeTruthy();
        expect(text.match(/\{\d\}/g)?.sort() ?? [], `${name} ${key}`).toEqual(en[key].match(/\{\d\}/g)?.sort() ?? []);
      }
    }
  });

  it('names the target folder in the button, whatever the language', () => {
    for (const dictionary of [en, fr, es, zh] as Array<Record<string, string>>) {
      expect(dictionary[`${M}button`]).toContain('.agents/skills');
    }
  });
});
