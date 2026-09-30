import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { WorkflowsPage } from '../WorkflowsPage';
import type { Project, QuickPrompt, Skill, WorkflowSummary } from '../../types/generated';

// KT-914 — the skills of the catalog, listed on the Automation page.

const mockSkillsApi = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
}));
const mockWorkflowsApi = vi.hoisted(() => ({
  list: vi.fn(),
  get: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
  trigger: vi.fn(),
  listRuns: vi.fn().mockResolvedValue([]),
  countRuns: vi.fn().mockResolvedValue(0),
  getRun: vi.fn(),
  deleteRun: vi.fn(),
  deleteAllRuns: vi.fn(),
  cancelRun: vi.fn(),
  triggerStream: vi.fn(),
  exportWorkflow: vi.fn(),
  importWorkflow: vi.fn(),
}));
const mockQuickPromptsApi = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  setPinned: vi.fn(),
  delete: vi.fn(),
  batchRun: vi.fn(),
  compareAgents: vi.fn(),
  exportQp: vi.fn(),
  importQp: vi.fn(),
}));

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: () => ({ connected: false, connectionState: 'connecting' }),
}));

vi.mock('../../lib/api', () => ({
  workflows: mockWorkflowsApi,
  skills: mockSkillsApi,
  profiles: { list: vi.fn().mockResolvedValue([]), get: vi.fn(), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  directives: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  quickPrompts: mockQuickPromptsApi,
  quickApis: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), setPinned: vi.fn(), delete: vi.fn() },
  quickExecs: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), setPinned: vi.fn(), delete: vi.fn() },
  config: { getUiLanguage: vi.fn().mockResolvedValue('fr'), saveUiLanguage: vi.fn().mockResolvedValue(undefined), getServerConfig: vi.fn().mockResolvedValue({ default_model_tier: 'default' }) },
  mcps: {
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], customized_contexts: [], incompatibilities: [] }),
    registry: vi.fn().mockResolvedValue([]),
  },
  discussions: { create: vi.fn() },
  modelCatalogApi: { list: vi.fn().mockResolvedValue({ targets: [] }) },
  externalApi: { list: vi.fn().mockResolvedValue([]) },
}));

const RAW_HTML = '<script>window.leak = true</script><img src=x onerror="window.leak = true">';
const REVIEW_MD = `---\nname: review\ndescription: Checks pull requests\n---\n\n# Review carefully\n\nRead **every** line.\n\n${RAW_HTML}\n`;

const skill = (over: Partial<Skill> & Pick<Skill, 'id' | 'name'>): Skill => ({
  description: '', icon: '🧩', category: 'Domain', content: '# Skill\n', is_builtin: false, token_estimate: 10, ...over,
});
const SKILLS: Skill[] = [
  skill({ id: 'review', name: 'Review', description: 'Checks pull requests', icon: '🔍', content: REVIEW_MD }),
  skill({ id: 'rust', name: 'Rust', description: 'Idioms for systems code', icon: '🦀', category: 'Language', is_builtin: true, content: '# Rust idioms\n' }),
  skill({ id: 'orphan', name: 'Orphan', description: 'Attached to nothing', icon: '👻', category: 'Business' }),
];
const PROJECTS = [
  { id: 'p-alpha', name: 'Alpha', path: '/work/alpha', repo_url: null, default_skill_ids: ['review', 'rust'] },
  { id: 'p-beta', name: 'Beta', path: '/work/beta', repo_url: null, default_skill_ids: ['review'] },
] as unknown as Project[];
const WORKFLOW = {
  id: 'wf-nightly', name: 'Nightly report', project_id: 'p-alpha', project_name: 'Alpha',
  trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0,
  enabled: true, pinned: false, last_run: null, created_at: '2026-03-01T00:00:00Z',
} as WorkflowSummary;
const PROMPT = {
  id: 'qp-daily', pinned: false, name: 'Daily standup', icon: '📅', prompt_template: 'Summarize.', variables: [],
  agent: 'ClaudeCode', project_id: null, skill_ids: [], profile_ids: [], directive_ids: [], tier: 'default', description: '',
  created_at: '2026-02-01T00:00:00Z', updated_at: '2026-02-01T00:00:00Z',
} as QuickPrompt;

const FAVORITES_KEY = 'kronn:automationSkillFavorites';

beforeEach(() => {
  mockSkillsApi.list.mockResolvedValue(SKILLS);
  mockWorkflowsApi.list.mockResolvedValue([WORKFLOW]);
  mockQuickPromptsApi.list.mockResolvedValue([PROMPT]);
});

afterEach(() => {
  cleanup();
  for (const key of ['kronn:automationNavigation', 'kronn:automationCollapsedSections', 'kronn:automationGroupBy', 'kronn:automationLastOpened', FAVORITES_KEY]) {
    localStorage.removeItem(key);
  }
  mockSkillsApi.delete.mockReset();
  delete (window as unknown as Record<string, unknown>).leak;
});

const wrap = async (ui: React.ReactElement) => {
  await act(async () => { render(<I18nProvider>{ui}</I18nProvider>); });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
};
const page = (props: Partial<React.ComponentProps<typeof WorkflowsPage>> = {}) => (
  <WorkflowsPage projects={PROJECTS} {...props} />
);

const sidebar = () => screen.getByRole('complementary', { name: 'Automatisation' });
const list = () => sidebar().querySelector('.automation-sidebar-items') as HTMLElement;
/** A group of the sidebar list, by key: `kind:skills`, `project:p-alpha`… */
const group = (key: string) => sidebar().querySelector(`[data-group="${key}"]`) as HTMLElement;
const PROJECT_GROUPS: Record<string, string> = { Alpha: 'project:p-alpha', Beta: 'project:p-beta', 'Sans projet': 'project:__global__' };
const projectGroup = (name: string) => group(PROJECT_GROUPS[name]);
const openButtons = (scope: HTMLElement) => (
  within(scope).queryAllByRole('button', { name: /Ouvrir /i }).map(button => button.getAttribute('aria-label'))
);
const headers = () => Array.from(sidebar().querySelectorAll('.automation-group-header')).map(header => header.getAttribute('aria-label'));

const groupBy = async (name: 'Type' | 'Projet' | 'Aucun') => {
  const segmented = within(sidebar()).getByRole('group', { name: 'Grouper par' });
  await act(async () => { fireEvent.click(within(segmented).getByRole('button', { name })); });
};
const typeChip = () => screen.getByRole('button', { name: /^Type d’automatisation : / });
const typeList = () => {
  if (!screen.queryByRole('listbox', { name: 'Filtre par type d’automatisation' })) fireEvent.click(typeChip());
  return screen.getByRole('listbox', { name: 'Filtre par type d’automatisation' });
};
const search = async (text: string) => {
  const input = screen.getByRole('textbox', { name: 'Rechercher une automatisation…' });
  await act(async () => { fireEvent.change(input, { target: { value: text } }); });
};

describe('WorkflowsPage — skills (KT-914)', () => {
  it('offers Skills in the type list with its count, next to the other types', async () => {
    await wrap(page());
    const options = within(typeList()).getAllByRole('option').map(option => option.getAttribute('aria-label'));
    // 1 workflow + 1 Quick Prompt + 3 skills.
    expect(options).toEqual(['Tout (5)', 'Workflows (1)', 'Quick Prompts (1)', 'Quick APIs (0)', 'Quick Execs (CLI) (0)', 'Skills (3)']);
  });

  it('counts the library title with the skills', async () => {
    await wrap(page());
    expect(sidebar().querySelector('.collection-shell-title-count')).toHaveTextContent('5');
  });

  it('groups the skills under their own type, after the dated automations, starred ones first', async () => {
    localStorage.setItem(FAVORITES_KEY, JSON.stringify(['rust']));
    await wrap(page());
    expect(headers()).toEqual(['Workflows 1', 'Quick Prompts 1', 'Skills 3']);
    expect(openButtons(group('kind:skills'))).toEqual(['Ouvrir Rust', 'Ouvrir Orphan', 'Ouvrir Review']);
    expect(openButtons(group('kind:workflows'))).toEqual(['Ouvrir Nightly report']);
    // A row in a type group leaves the type to the group: no "Skills · " in front.
    expect(within(group('kind:skills')).queryByText(/^Skills · /)).toBeNull();
  });

  it('lists each skill once in the flat list, after the dated automations, when sorted by last change', async () => {
    await wrap(page());
    await groupBy('Aucun');
    fireEvent.click(screen.getByLabelText('Autres actions'));
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'Dernière modification' }));
    expect(openButtons(list())).toEqual([
      'Ouvrir Nightly report', 'Ouvrir Daily standup', 'Ouvrir Orphan', 'Ouvrir Review', 'Ouvrir Rust',
    ]);
  });

  it('files a skill under every project that lists it, and an unattached one under "No project"', async () => {
    await wrap(page());
    await groupBy('Projet');
    expect(headers()).toEqual(['Alpha 3', 'Beta 1', 'Sans projet 2']);
    expect(openButtons(projectGroup('Alpha'))).toEqual(['Ouvrir Nightly report', 'Ouvrir Review', 'Ouvrir Rust']);
    expect(openButtons(projectGroup('Beta'))).toEqual(['Ouvrir Review']);
    expect(openButtons(projectGroup('Sans projet'))).toEqual(['Ouvrir Daily standup', 'Ouvrir Orphan']);
    // The title counts each automation once, not once per folder: six rows, five automations.
    expect(openButtons(list())).toHaveLength(6);
    expect(sidebar().querySelector('.collection-shell-title-count')).toHaveTextContent('5');
  });

  it('keeps the same rows under the project chip, by project and by "No project"', async () => {
    await wrap(page());
    fireEvent.click(screen.getByRole('button', { name: 'Filtrer les automatisations par projet' }));
    expect(within(screen.getByRole('listbox', { name: 'Filtrer les automatisations par projet' }))
      .getAllByRole('option').map(option => option.getAttribute('aria-label')))
      .toEqual(['Sans projet (2)', 'Alpha (3)', 'Beta (1)']);
    fireEvent.click(screen.getByRole('option', { name: 'Beta (1)' }));
    expect(openButtons(list())).toEqual(['Ouvrir Review']);
    fireEvent.click(screen.getByRole('button', { name: 'Beta' }));
    fireEvent.click(screen.getByRole('option', { name: 'Sans projet (2)' }));
    expect(openButtons(list())).toEqual(['Ouvrir Daily standup', 'Ouvrir Orphan']);
  });

  it('lists a starred skill from the Épinglés chip, and keeps the star in this browser', async () => {
    localStorage.setItem(FAVORITES_KEY, JSON.stringify(['rust']));
    await wrap(page());
    const pinned = within(sidebar()).getByRole('button', { name: 'Épinglés' });
    fireEvent.click(pinned);
    expect(openButtons(list())).toEqual(['Ouvrir Rust']);
    fireEvent.click(pinned);

    const orphanStar = within(group('kind:skills')).getByRole('button', { name: 'Ajouter aux favoris · Orphan' });
    await act(async () => { fireEvent.click(orphanStar); });
    expect(JSON.parse(localStorage.getItem(FAVORITES_KEY) ?? '[]').sort()).toEqual(['orphan', 'rust']);
    fireEvent.click(pinned);
    expect(openButtons(list())).toEqual(['Ouvrir Orphan', 'Ouvrir Rust']);

    await act(async () => { fireEvent.click(within(list()).getByRole('button', { name: 'Retirer des favoris · Rust' })); });
    expect(JSON.parse(localStorage.getItem(FAVORITES_KEY) ?? '[]')).toEqual(['orphan']);
    expect(openButtons(list())).toEqual(['Ouvrir Orphan']);
  });

  it('opens the sheet of a skill in the main column: name, description, category, projects, SKILL.md', async () => {
    await wrap(page());
    await act(async () => { fireEvent.click(within(group('kind:skills')).getByRole('button', { name: 'Ouvrir Review' })); });

    const sheet = screen.getByTestId('skill-sheet');
    expect(within(sheet).getByRole('heading', { level: 2, name: 'Review' })).toBeInTheDocument();
    expect(within(sheet).getByTestId('skill-description')).toHaveTextContent('Checks pull requests');
    expect(within(sheet).getByTestId('skill-category')).toHaveTextContent('Domain');
    expect(within(within(sheet).getByTestId('skill-projects')).getAllByRole('listitem').map(item => item.textContent))
      .toEqual(['Alpha', 'Beta']);

    // SKILL.md, rendered as Markdown and never as HTML.
    const content = within(sheet).getByTestId('skill-content');
    expect(within(content).getByRole('heading', { level: 1, name: 'Review carefully' })).toBeInTheDocument();
    expect(sheet.querySelector('script, img, [onerror]')).toBeNull();
    expect((window as unknown as Record<string, unknown>).leak).toBeUndefined();

    // The row that was opened is the active one.
    expect(within(group('kind:skills')).getByRole('button', { name: 'Ouvrir Review' })).toHaveAttribute('aria-current', 'true');
    expect(JSON.parse(localStorage.getItem('kronn:automationNavigation') ?? '{}')).toEqual({ tab: 'skills', resourceId: 'review' });
  });

  it('switches the SKILL.md between Rendu and Source', async () => {
    await wrap(page());
    await act(async () => { fireEvent.click(within(group('kind:skills')).getByRole('button', { name: 'Ouvrir Review' })); });
    const content = screen.getByTestId('skill-content');
    expect(within(content).getByRole('button', { name: 'Rendu' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(content).queryByTestId('content-source')).toBeNull();

    fireEvent.click(within(content).getByRole('button', { name: 'Source' }));
    expect(within(content).getByTestId('content-source').textContent).toBe(REVIEW_MD);
    expect(within(content).queryByTestId('content-rendered')).toBeNull();
    expect(content.querySelector('script, img')).toBeNull();

    fireEvent.click(within(content).getByRole('button', { name: 'Rendu' }));
    expect(within(content).getByTestId('content-rendered')).toBeInTheDocument();
  });

  it('shows an unattached skill as used by no project', async () => {
    await wrap(page());
    await act(async () => { fireEvent.click(within(group('kind:skills')).getByRole('button', { name: 'Ouvrir Orphan' })); });
    const used = screen.getByTestId('skill-projects');
    expect(within(used).queryAllByRole('listitem')).toHaveLength(0);
    expect(within(used).getByText(/Aucun projet ne déclare ce skill/)).toBeInTheDocument();
  });

  it('reopens the sheet of the skill that was open before a reload', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'rust' }));
    await wrap(page());
    expect(screen.getByRole('heading', { level: 2, name: 'Rust' })).toBeInTheDocument();
    expect(screen.getByTestId('skill-content')).toHaveTextContent('Rust idioms');
  });

  it('falls back to the list when the remembered skill no longer exists', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'deleted' }));
    await wrap(page());
    expect(screen.queryByTestId('skill-sheet')).toBeNull();
    expect(JSON.parse(localStorage.getItem('kronn:automationNavigation') ?? '{}')).toEqual({ tab: 'skills', resourceId: null });
  });

  it('finds a skill by its name and by its description, like the other types', async () => {
    await wrap(page());
    await search('rust');
    expect(openButtons(list())).toEqual(['Ouvrir Rust']);
    await search('pull requests');
    expect(openButtons(list())).toEqual(['Ouvrir Review']);
    // Present in both of its project folders when grouped by project.
    await groupBy('Projet');
    expect(openButtons(projectGroup('Alpha'))).toEqual(['Ouvrir Review']);
    expect(openButtons(projectGroup('Beta'))).toEqual(['Ouvrir Review']);
    await search('attached to nothing');
    expect(openButtons(projectGroup('Sans projet'))).toEqual(['Ouvrir Orphan']);
    await search('no such skill');
    expect(screen.getByText('Aucune automatisation ne correspond à ces filtres.')).toBeInTheDocument();
  });

  it('lists only the skills, as cards that open their sheet, when the Skills type is picked', async () => {
    await wrap(page());
    // `typeList()` opens the menu with its own click: resolve it outside `act`, which would hold the render back.
    const skillsOption = within(typeList()).getByRole('option', { name: 'Skills (3)' });
    await act(async () => { fireEvent.click(skillsOption); });
    expect(typeChip()).toHaveAttribute('data-value', 'skills');
    expect(openButtons(list())).toEqual(['Ouvrir Orphan', 'Ouvrir Review', 'Ouvrir Rust']);

    const viewer = document.querySelector('.automation-viewer') as HTMLElement;
    expect(within(viewer).queryByTestId('skill-sheet')).toBeNull();
    expect(openButtons(viewer)).toEqual(['Ouvrir Orphan', 'Ouvrir Review', 'Ouvrir Rust']);
    await act(async () => { fireEvent.click(within(viewer).getByRole('button', { name: 'Ouvrir Rust' })); });
    expect(screen.getByRole('heading', { level: 2, name: 'Rust' })).toBeInTheDocument();
  });

  it('sends the reader to Config, the one place a skill is edited', async () => {
    const onNavigateSettings = vi.fn();
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'review' }));
    await wrap(page({ onNavigateSettings }));
    fireEvent.click(screen.getByRole('button', { name: 'Ouvrir dans Config' }));
    expect(onNavigateSettings).toHaveBeenCalledTimes(1);
  });

  it('deletes a skill the user wrote from its sheet, and leaves a built-in one alone', async () => {
    mockSkillsApi.delete.mockResolvedValue(true);
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'rust' }));
    await wrap(page());
    expect(screen.queryByTestId('skill-delete-rust')).toBeNull();

    await act(async () => { fireEvent.click(within(group('kind:skills')).getByRole('button', { name: 'Ouvrir Orphan' })); });
    mockSkillsApi.list.mockResolvedValue(SKILLS.filter(item => item.id !== 'orphan'));
    await act(async () => { fireEvent.click(screen.getByTestId('skill-delete-orphan')); });
    await act(async () => { fireEvent.click(screen.getByTestId('skill-delete-orphan')); });
    expect(mockSkillsApi.delete).toHaveBeenCalledWith('orphan');
    expect(screen.queryByTestId('skill-sheet')).toBeNull();
    expect(openButtons(list())).not.toContain('Ouvrir Orphan');
  });

  it('offers a row menu delete for a skill the user wrote, not for a built-in one', async () => {
    await wrap(page());
    const menu = async (name: string) => {
      const skills = group('kind:skills');
      await act(async () => { fireEvent.click(within(skills).getByRole('button', { name: `Plus d’actions · ${name}` })); });
      return within(skills).getByRole('menu');
    };
    expect(within(await menu('Rust')).queryByRole('menuitem', { name: 'Supprimer' })).toBeNull();
    await act(async () => { fireEvent.keyDown(window, { key: 'Escape' }); });
    expect(within(await menu('Review')).getByRole('menuitem', { name: 'Supprimer' })).toBeInTheDocument();
  });
});
