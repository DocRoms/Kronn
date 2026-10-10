import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { WorkflowsPage } from '../WorkflowsPage';
import type { Project, ProjectUsedSkill, QuickPrompt, Skill, WorkflowSummary } from '../../types/generated';

// KT-914 — the skills of the catalog, listed on the Automation page.
// KT-921 — only the skills a project uses are listed; the others wait behind
// « Voir les skills disponibles », and a skill only a repository holds is listed
// under its project.

const mockSkillsApi = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
}));
const mockProjectsApi = vi.hoisted(() => ({
  usedSkills: vi.fn(),
  usedSkillFile: vi.fn(),
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
  autoDisabled: vi.fn().mockResolvedValue([]),
  reenable: vi.fn().mockResolvedValue([]),
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
  projects: mockProjectsApi,
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
// `review` and `rust` are listed by a project; `orphan` and `spare` by none.
const SKILLS: Skill[] = [
  skill({ id: 'review', name: 'Review', description: 'Checks pull requests', icon: '🔍', content: REVIEW_MD }),
  skill({ id: 'rust', name: 'Rust', description: 'Idioms for systems code', icon: '🦀', category: 'Language', is_builtin: true, content: '# Rust idioms\n' }),
  skill({ id: 'orphan', name: 'Orphan', description: 'Attached to nothing', icon: '👻', category: 'Business' }),
  skill({ id: 'spare', name: 'Spare', description: 'Kept in reserve', icon: '🧰' }),
];
const PROJECTS = [
  { id: 'p-alpha', name: 'Alpha', path: '/work/alpha', repo_url: null, default_skill_ids: ['review', 'rust'] },
  { id: 'p-beta', name: 'Beta', path: '/work/beta', repo_url: null, default_skill_ids: ['review'] },
] as unknown as Project[];
const WORKFLOW = {
  id: 'wf-nightly', name: 'Nightly report', project_id: 'p-alpha', project_name: 'Alpha',
  trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0, unsafe_step_count: 0,
  enabled: true, pinned: false, last_run: null, created_at: '2026-03-01T00:00:00Z',
} as WorkflowSummary;
const PROMPT = {
  id: 'qp-daily', pinned: false, name: 'Daily standup', icon: '📅', prompt_template: 'Summarize.', variables: [],
  agent: 'ClaudeCode', project_id: null, skill_ids: [], profile_ids: [], directive_ids: [], tier: 'default', description: '',
  created_at: '2026-02-01T00:00:00Z', updated_at: '2026-02-01T00:00:00Z',
} as QuickPrompt;

const FAVORITES_KEY = 'kronn:automationSkillFavorites';
const AUTOMATION_ACTIONS = 'Créer ou importer';

beforeEach(() => {
  mockSkillsApi.list.mockResolvedValue(SKILLS);
  mockProjectsApi.usedSkills.mockResolvedValue([]);
  mockWorkflowsApi.list.mockResolvedValue([WORKFLOW]);
  mockQuickPromptsApi.list.mockResolvedValue([PROMPT]);
});

afterEach(() => {
  cleanup();
  for (const key of ['kronn:automationNavigation', 'kronn:automationCollapsedSections', 'kronn:automationGroupBy', 'kronn:automationLastOpened', FAVORITES_KEY]) {
    localStorage.removeItem(key);
  }
  mockSkillsApi.delete.mockReset();
  mockProjectsApi.usedSkills.mockReset();
  mockProjectsApi.usedSkillFile.mockReset();
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

/** The fold under the list: « Voir les skills disponibles (N) ». */
const AVAILABLE_GROUP = 'skills:available';
const availableToggle = (count: number, scope: HTMLElement = sidebar()) => (
  within(scope).getByRole('button', { name: `Voir les skills disponibles (${count})` })
);
const openAvailable = async (count: number) => {
  await act(async () => { fireEvent.click(availableToggle(count)); });
};

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
    // 1 workflow + 1 Quick Prompt + the 2 skills a project uses: the 2 others wait behind « Voir les skills disponibles ».
    expect(options).toEqual(['Tout (4)', 'Workflows (1)', 'Quick Prompts (1)', 'Quick APIs (0)', 'Quick Execs (CLI) (0)', 'Skills (2)']);
  });

  it('counts the library title with the skills a project uses', async () => {
    await wrap(page());
    expect(sidebar().querySelector('.collection-shell-title-count')).toHaveTextContent('4');
  });

  it('groups the skills under their own type, after the dated automations, starred ones first', async () => {
    localStorage.setItem(FAVORITES_KEY, JSON.stringify(['rust']));
    await wrap(page());
    expect(headers()).toEqual(['Workflows 1', 'Quick Prompts 1', 'Skills 2', 'Voir les skills disponibles (2)']);
    expect(openButtons(group('kind:skills'))).toEqual(['Ouvrir Rust', 'Ouvrir Review']);
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
      'Ouvrir Nightly report', 'Ouvrir Daily standup', 'Ouvrir Review', 'Ouvrir Rust',
    ]);
    // The skills no project uses come last of all, once asked for.
    await openAvailable(2);
    expect(openButtons(list())).toEqual([
      'Ouvrir Nightly report', 'Ouvrir Daily standup', 'Ouvrir Review', 'Ouvrir Rust', 'Ouvrir Orphan', 'Ouvrir Spare',
    ]);
  });

  it('files a skill under every project that uses it, and none of the unused ones under "No project"', async () => {
    await wrap(page());
    await groupBy('Projet');
    expect(headers()).toEqual(['Alpha 3', 'Beta 1', 'Sans projet 1', 'Voir les skills disponibles (2)']);
    expect(openButtons(projectGroup('Alpha'))).toEqual(['Ouvrir Nightly report', 'Ouvrir Review', 'Ouvrir Rust']);
    expect(openButtons(projectGroup('Beta'))).toEqual(['Ouvrir Review']);
    expect(openButtons(projectGroup('Sans projet'))).toEqual(['Ouvrir Daily standup']);
    // The title counts each automation once, not once per folder: five rows, four automations.
    expect(openButtons(list())).toHaveLength(5);
    expect(sidebar().querySelector('.collection-shell-title-count')).toHaveTextContent('4');
  });

  it('keeps the same rows under the project chip, by project and by "No project"', async () => {
    await wrap(page());
    fireEvent.click(screen.getByRole('button', { name: 'Filtrer les automatisations par projet' }));
    expect(within(screen.getByRole('listbox', { name: 'Filtrer les automatisations par projet' }))
      .getAllByRole('option').map(option => option.getAttribute('aria-label')))
      .toEqual(['Sans projet (1)', 'Alpha (3)', 'Beta (1)']);
    fireEvent.click(screen.getByRole('option', { name: 'Beta (1)' }));
    expect(openButtons(list())).toEqual(['Ouvrir Review']);
    fireEvent.click(screen.getByRole('button', { name: 'Beta' }));
    fireEvent.click(screen.getByRole('option', { name: 'Sans projet (1)' }));
    // "No project" is where the unused skills are, folded away like anywhere else.
    expect(openButtons(list())).toEqual(['Ouvrir Daily standup']);
    await openAvailable(2);
    expect(openButtons(list())).toEqual(['Ouvrir Daily standup', 'Ouvrir Orphan', 'Ouvrir Spare']);
  });

  it('lists a starred skill from the Épinglés chip, and keeps the star in this browser', async () => {
    localStorage.setItem(FAVORITES_KEY, JSON.stringify(['rust']));
    await wrap(page());
    const pinned = within(sidebar()).getByRole('button', { name: 'Épinglés' });
    fireEvent.click(pinned);
    expect(openButtons(list())).toEqual(['Ouvrir Rust']);
    fireEvent.click(pinned);

    await openAvailable(2);
    const orphanStar = within(group(AVAILABLE_GROUP)).getByRole('button', { name: 'Ajouter aux favoris · Orphan' });
    await act(async () => { fireEvent.click(orphanStar); });
    expect(JSON.parse(localStorage.getItem(FAVORITES_KEY) ?? '[]').sort()).toEqual(['orphan', 'rust']);
    fireEvent.click(pinned);
    expect(openButtons(list())).toEqual(['Ouvrir Rust', 'Ouvrir Orphan']);

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

  it('shows a skill no project uses as used by none, once its fold is opened', async () => {
    await wrap(page());
    await openAvailable(2);
    await act(async () => { fireEvent.click(within(group(AVAILABLE_GROUP)).getByRole('button', { name: 'Ouvrir Orphan' })); });
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
    // A skill no project uses is found too, in its fold, which the search lays open.
    await search('attached to nothing');
    expect(openButtons(group(AVAILABLE_GROUP))).toEqual(['Ouvrir Orphan']);
    await search('no such skill');
    expect(screen.getByText('Aucune automatisation ne correspond à ces filtres.')).toBeInTheDocument();
  });

  it('lists only the skills, as cards that open their sheet, when the Skills type is picked', async () => {
    await wrap(page());
    // `typeList()` opens the menu with its own click: resolve it outside `act`, which would hold the render back.
    const skillsOption = within(typeList()).getByRole('option', { name: 'Skills (2)' });
    await act(async () => { fireEvent.click(skillsOption); });
    expect(typeChip()).toHaveAttribute('data-value', 'skills');
    expect(openButtons(list())).toEqual(['Ouvrir Review', 'Ouvrir Rust']);

    const viewer = document.querySelector('.automation-viewer') as HTMLElement;
    expect(within(viewer).queryByTestId('skill-sheet')).toBeNull();
    // Kronn's skills first, then the user's (KT-1140).
    expect(openButtons(viewer)).toEqual(['Ouvrir Rust', 'Ouvrir Review']);
    // The cards fold the unused skills away as the list does.
    const toggle = availableToggle(2, viewer);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await act(async () => { fireEvent.click(toggle); });
    expect(openButtons(viewer)).toEqual(['Ouvrir Rust', 'Ouvrir Review', 'Ouvrir Orphan', 'Ouvrir Spare']);
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

  it('creates a skill from the Automation menu, for the project picked or for every project', async () => {
    const created = skill({ id: 'custom-pr-review', name: 'PR review', project_id: 'p-beta' });
    mockSkillsApi.create.mockResolvedValue(created);
    await wrap(page());
    await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: AUTOMATION_ACTIONS })[0]); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Nouveau skill' })); });
    const form = screen.getByRole('region', { name: 'Nouveau skill' });
    const save = within(form).getByRole('button', { name: 'Ajouter un skill' });
    expect(save).toBeDisabled();

    fireEvent.change(within(form).getByRole('textbox', { name: 'Nom *' }), { target: { value: ' PR review ' } });
    fireEvent.change(within(form).getByRole('textbox', { name: 'Contenu (system prompt) *' }), { target: { value: 'Review it.' } });
    const project = within(form).getByRole('combobox', { name: /^Projet/ });
    expect(project).toHaveValue('');
    expect(within(form).getByText('Proposé dans tous les projets.')).toBeInTheDocument();
    fireEvent.change(project, { target: { value: 'p-beta' } });
    mockSkillsApi.list.mockResolvedValue([...SKILLS, created]);
    await act(async () => { fireEvent.click(save); });

    expect(mockSkillsApi.create).toHaveBeenCalledWith(expect.objectContaining({
      name: 'PR review', content: 'Review it.', project_id: 'p-beta',
    }));
    expect(screen.queryByRole('region', { name: 'Nouveau skill' })).toBeNull();
    expect(screen.getByTestId('skill-project')).toHaveTextContent('Projet : Beta');
  });

  it('edits a skill the user wrote from its sheet and can make it global', async () => {
    const scoped = skill({ id: 'custom-pr-review', name: 'PR review', project_id: 'p-beta', license: 'MIT' });
    mockSkillsApi.list.mockResolvedValue([...SKILLS, scoped]);
    mockSkillsApi.update.mockResolvedValue({ ...scoped, project_id: undefined });
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'custom-pr-review' }));
    await wrap(page());
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Modifier' })); });
    const form = screen.getByRole('region', { name: 'Modifier le skill' });
    expect(within(form).getByRole('combobox', { name: /^Projet/ })).toHaveValue('p-beta');
    fireEvent.change(within(form).getByRole('combobox', { name: /^Projet/ }), { target: { value: '' } });
    await act(async () => { fireEvent.click(within(form).getByRole('button', { name: 'Enregistrer' })); });
    expect(mockSkillsApi.update).toHaveBeenCalledWith('custom-pr-review', expect.objectContaining({
      project_id: null, license: 'MIT',
    }));
  });

  it('offers no edit for a built-in skill', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'rust' }));
    await wrap(page());
    expect(screen.queryByRole('button', { name: 'Modifier' })).toBeNull();
  });

  it('deletes a skill the user wrote from its sheet, and leaves a built-in one alone', async () => {
    mockSkillsApi.delete.mockResolvedValue(true);
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'rust' }));
    await wrap(page());
    expect(screen.queryByTestId('skill-delete-rust')).toBeNull();

    await openAvailable(2);
    await act(async () => { fireEvent.click(within(group(AVAILABLE_GROUP)).getByRole('button', { name: 'Ouvrir Orphan' })); });
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

describe('WorkflowsPage — skills no project uses (KT-921)', () => {
  it.each([
    ['Type', 'kind:skills'],
    ['Projet', 'project:p-alpha'],
    ['Aucun', 'none:all'],
  ] as const)('keeps them behind « Voir les skills disponibles (N) », closed and last, when grouped by %s', async (by, someGroup) => {
    await wrap(page());
    await groupBy(by);
    const toggle = availableToggle(2);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    // Neither in a group, nor anywhere else in the list, until asked for.
    expect(group(someGroup)).not.toBeNull();
    expect(openButtons(list())).not.toContain('Ouvrir Orphan');
    expect(openButtons(list())).not.toContain('Ouvrir Spare');
    expect(list().lastElementChild).toBe(group(AVAILABLE_GROUP));

    await act(async () => { fireEvent.click(toggle); });
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(openButtons(group(AVAILABLE_GROUP))).toEqual(['Ouvrir Orphan', 'Ouvrir Spare']);
    // Only skills are in there: like a type group, the rows do not repeat the type.
    expect(within(group(AVAILABLE_GROUP)).queryByText(/^Skills · /)).toBeNull();

    await act(async () => { fireEvent.click(toggle); });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(openButtons(list())).not.toContain('Ouvrir Orphan');
  });

  it.each(['Type', 'Projet', 'Aucun'] as const)('opens that section when a search finds one of them, and folds it back with the search, grouped by %s', async by => {
    await wrap(page());
    await groupBy(by);
    await search('reserve');
    // Only what the search found, in the section the search laid open.
    const toggle = availableToggle(1);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(openButtons(group(AVAILABLE_GROUP))).toEqual(['Ouvrir Spare']);
    // The search decides: the section cannot be folded while it looks.
    await act(async () => { fireEvent.click(toggle); });
    expect(toggle).toHaveAttribute('aria-expanded', 'true');

    await search('');
    const closed = availableToggle(2);
    expect(closed).toHaveAttribute('aria-expanded', 'false');
    expect(openButtons(list())).not.toContain('Ouvrir Spare');
  });

  it('shows no section when every skill is used', async () => {
    mockSkillsApi.list.mockResolvedValue(SKILLS.filter(item => item.id === 'review' || item.id === 'rust'));
    await wrap(page());
    expect(screen.queryByRole('button', { name: /^Voir les skills disponibles/ })).toBeNull();
  });

  it('counts a skill Kronn published into a project\'s repository as used by that project', async () => {
    mockProjectsApi.usedSkills.mockResolvedValue([{
      project_id: 'p-beta', skill_id: 'orphan', slug: 'orphan', name: 'Orphan',
      root: '.agents/skills', relative_path: '.agents/skills/orphan/SKILL.md', referenced: false, published: true,
    } satisfies ProjectUsedSkill]);
    await wrap(page());
    await groupBy('Projet');
    expect(openButtons(projectGroup('Beta'))).toEqual(['Ouvrir Orphan', 'Ouvrir Review']);
    // It left the fold: only the skill nobody uses is still in it.
    expect(availableToggle(1)).toBeInTheDocument();

    await act(async () => { fireEvent.click(within(projectGroup('Beta')).getByRole('button', { name: 'Ouvrir Orphan' })); });
    expect(within(within(screen.getByTestId('skill-sheet')).getByTestId('skill-projects')).getAllByRole('listitem').map(item => item.textContent))
      .toEqual(['Beta']);
  });
});

describe('WorkflowsPage — skills used from a repository (KT-921)', () => {
  const SKILL_PATH = '.agents/skills/block-migration/SKILL.md';
  const REPOSITORY_SKILL: ProjectUsedSkill = {
    project_id: 'p-alpha', slug: 'block-migration', name: 'block-migration',
    root: '.agents/skills', relative_path: SKILL_PATH, referenced: true, published: false,
  };
  const BLOCK_MD = `---\nname: block-migration\ndescription: Moves a block\n---\n\n# Move the block\n\nRun the **codemod**.\n\n${RAW_HTML}\n`;
  const REPOSITORY_SKILL_ID = 'repository:p-alpha:block-migration';

  beforeEach(() => {
    mockProjectsApi.usedSkills.mockResolvedValue([REPOSITORY_SKILL]);
    mockProjectsApi.usedSkillFile.mockResolvedValue({ relative_path: SKILL_PATH, content: BLOCK_MD, truncated: false });
  });

  it('lists a native skill of a repository under its project, with where it comes from', async () => {
    await wrap(page());
    // By type: it is one of the skills, and its row says where it lives.
    expect(headers()).toEqual(['Workflows 1', 'Quick Prompts 1', 'Skills 3', 'Voir les skills disponibles (2)']);
    expect(openButtons(group('kind:skills'))).toEqual(['Ouvrir block-migration', 'Ouvrir Review', 'Ouvrir Rust']);
    expect(within(group('kind:skills')).getByText('Dépôt · .agents/skills')).toBeInTheDocument();

    // By project: under the project whose repository holds it, and no other.
    await groupBy('Projet');
    expect(openButtons(projectGroup('Alpha'))).toContain('Ouvrir block-migration');
    expect(openButtons(projectGroup('Beta'))).not.toContain('Ouvrir block-migration');
    expect(within(projectGroup('Alpha')).getByText('Skills · Dépôt · .agents/skills')).toBeInTheDocument();
  });

  it('opens its sheet with the SKILL.md read from the repository, rendered without raw HTML', async () => {
    await wrap(page());
    await act(async () => { fireEvent.click(within(group('kind:skills')).getByRole('button', { name: 'Ouvrir block-migration' })); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });

    expect(mockProjectsApi.usedSkillFile).toHaveBeenCalledWith('p-alpha', SKILL_PATH);
    const sheet = screen.getByTestId('skill-sheet');
    expect(within(sheet).getByRole('heading', { level: 2, name: 'block-migration' })).toBeInTheDocument();
    expect(within(sheet).getByTestId('skill-origin')).toHaveTextContent('Dépôt · .agents/skills');
    expect(within(within(sheet).getByTestId('skill-projects')).getAllByRole('listitem').map(item => item.textContent))
      .toEqual(['Alpha']);
    const content = within(sheet).getByTestId('skill-content');
    expect(within(content).getByRole('heading', { level: 1, name: 'Move the block' })).toBeInTheDocument();
    expect(within(content).getByText('codemod').tagName).toBe('STRONG');
    expect(sheet.querySelector('script, img, [onerror]')).toBeNull();
    expect((window as unknown as Record<string, unknown>).leak).toBeUndefined();

    fireEvent.click(within(content).getByRole('button', { name: 'Source' }));
    expect(within(content).getByTestId('content-source').textContent).toBe(BLOCK_MD);

    // Its source is the repository: nothing of it to edit in Config or to delete in Kronn.
    expect(within(sheet).getByText(/Ce skill vit dans le dépôt du projet/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Ouvrir dans Config' })).toBeNull();
    expect(sheet.querySelector('[data-testid^="skill-delete-"]')).toBeNull();
    expect(JSON.parse(localStorage.getItem('kronn:automationNavigation') ?? '{}')).toEqual({ tab: 'skills', resourceId: REPOSITORY_SKILL_ID });
  });

  it('offers no delete on its row, and no box to select it for a bulk delete', async () => {
    await wrap(page());
    const skills = group('kind:skills');
    await act(async () => { fireEvent.click(within(skills).getByRole('button', { name: 'Plus d’actions · block-migration' })); });
    expect(within(skills).queryByRole('menuitem', { name: 'Supprimer' })).toBeNull();
  });

  it('keeps the remembered repository skill while the list of used skills is on its way, then reopens it', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: REPOSITORY_SKILL_ID }));
    let answer: (skills: ProjectUsedSkill[]) => void = () => {};
    mockProjectsApi.usedSkills.mockReturnValue(new Promise<ProjectUsedSkill[]>(resolve => { answer = resolve; }));
    await wrap(page());
    // Not known yet is not gone: the memory is left alone.
    expect(JSON.parse(localStorage.getItem('kronn:automationNavigation') ?? '{}')).toEqual({ tab: 'skills', resourceId: REPOSITORY_SKILL_ID });
    expect(screen.queryByTestId('skill-sheet')).toBeNull();

    await act(async () => { answer([REPOSITORY_SKILL]); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    expect(screen.getByRole('heading', { level: 2, name: 'block-migration' })).toBeInTheDocument();
    expect(screen.getByTestId('skill-content')).toHaveTextContent('Move the block');
  });

  it('forgets a remembered repository skill once the list is in and no longer holds it', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'skills', resourceId: 'repository:p-alpha:gone' }));
    await wrap(page());
    expect(screen.queryByTestId('skill-sheet')).toBeNull();
    expect(JSON.parse(localStorage.getItem('kronn:automationNavigation') ?? '{}')).toEqual({ tab: 'skills', resourceId: null });
  });

  it('says so when its SKILL.md cannot be read from the repository', async () => {
    mockProjectsApi.usedSkillFile.mockRejectedValue(new Error('cannot read .agents/skills/block-migration/SKILL.md'));
    await wrap(page());
    await act(async () => { fireEvent.click(within(group('kind:skills')).getByRole('button', { name: 'Ouvrir block-migration' })); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    expect(within(screen.getByTestId('skill-content')).getByRole('alert'))
      .toHaveTextContent('Impossible de lire le SKILL.md dans le dépôt : cannot read .agents/skills/block-migration/SKILL.md');
  });

  it('keeps the same skill of two repositories apart', async () => {
    mockProjectsApi.usedSkills.mockResolvedValue([
      REPOSITORY_SKILL,
      { ...REPOSITORY_SKILL, project_id: 'p-beta' },
    ]);
    await wrap(page());
    await groupBy('Projet');
    expect(openButtons(projectGroup('Alpha'))).toContain('Ouvrir block-migration');
    expect(openButtons(projectGroup('Beta'))).toContain('Ouvrir block-migration');

    await act(async () => { fireEvent.click(within(projectGroup('Beta')).getByRole('button', { name: 'Ouvrir block-migration' })); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    expect(mockProjectsApi.usedSkillFile).toHaveBeenLastCalledWith('p-beta', SKILL_PATH);
    expect(within(within(screen.getByTestId('skill-sheet')).getByTestId('skill-projects')).getAllByRole('listitem').map(item => item.textContent))
      .toEqual(['Beta']);
  });

  it('still lists the skills a project attaches when the used-skills list cannot be read', async () => {
    mockProjectsApi.usedSkills.mockRejectedValue(new Error('offline'));
    await wrap(page());
    expect(openButtons(group('kind:skills'))).toEqual(['Ouvrir Review', 'Ouvrir Rust']);
  });
});

// KT-1140 — the cards split Kronn's built-in skills from the user's, with the
// project card's groups, labels and badges.
describe('WorkflowsPage — Kronn skills apart from the user\'s (KT-1140)', () => {
  const PROJECT_SKILL = skill({ id: 'custom-beta-only', name: 'Beta only', project_id: 'p-beta' });
  const REPOSITORY_SKILL: ProjectUsedSkill = {
    project_id: 'p-alpha', slug: 'block-migration', name: 'block-migration',
    root: '.agents/skills', relative_path: '.agents/skills/block-migration/SKILL.md', referenced: true, published: false,
  };

  const showCards = async () => {
    await wrap(page());
    const skillsOption = within(typeList()).getByRole('option', { name: /^Skills \(/ });
    await act(async () => { fireEvent.click(skillsOption); });
    return document.querySelector('.automation-viewer') as HTMLElement;
  };
  const skillGroup = (viewer: HTMLElement, name: string) => within(viewer).getByRole('region', { name });
  const cardOf = (scope: HTMLElement, name: string) => (
    within(scope).getByRole('button', { name: `Ouvrir ${name}` }).closest('.skill-card') as HTMLElement
  );

  beforeEach(() => {
    mockSkillsApi.list.mockResolvedValue([...SKILLS, PROJECT_SKILL]);
    mockProjectsApi.usedSkills.mockResolvedValue([REPOSITORY_SKILL]);
  });

  it('lists the built-in skills under « Skills Kronn » and the custom, project and repository ones under « Mes skills »', async () => {
    const viewer = await showCards();
    const groups = Array.from(viewer.querySelectorAll('.skill-group-title')).map(title => title.textContent);
    expect(groups).toEqual(['Skills Kronn 1', 'Mes skills 3']);
    expect(openButtons(skillGroup(viewer, 'Skills Kronn'))).toEqual(['Ouvrir Rust']);
    expect(openButtons(skillGroup(viewer, 'Mes skills'))).toEqual(['Ouvrir Beta only', 'Ouvrir block-migration', 'Ouvrir Review']);
  });

  it('badges a project skill « Projet » and a repository skill « Dépôt », and a built-in one with neither', async () => {
    const viewer = await showCards();
    const mine = skillGroup(viewer, 'Mes skills');
    expect(within(cardOf(mine, 'Beta only')).getByTestId('skill-group-badge-project')).toHaveTextContent('Projet');
    expect(within(cardOf(mine, 'block-migration')).getByTestId('skill-group-badge-repository')).toHaveTextContent('Dépôt');
    expect(within(cardOf(mine, 'Review')).queryByTestId(/^skill-group-badge-/)).toBeNull();
    expect(within(cardOf(skillGroup(viewer, 'Skills Kronn'), 'Rust')).queryByTestId(/^skill-group-badge-/)).toBeNull();
  });

  it('badges « Pas synchro » a skill a project reports in conflict or moved in the repository', async () => {
    mockProjectsApi.usedSkills.mockResolvedValue([
      REPOSITORY_SKILL,
      { project_id: 'p-alpha', skill_id: 'rust', slug: 'rust', name: 'Rust', root: '.agents/skills',
        relative_path: '.agents/skills/rust/SKILL.md', referenced: false, published: false, sync_status: 'repository_newer' },
      { project_id: 'p-alpha', skill_id: 'review', slug: 'review', name: 'Review', root: '.agents/skills',
        relative_path: '.agents/skills/review/SKILL.md', referenced: false, published: true, sync_status: 'up_to_date' },
      { project_id: 'p-beta', skill_id: 'review', slug: 'review', name: 'Review', root: '.agents/skills',
        relative_path: '.agents/skills/review/SKILL.md', referenced: false, published: true, sync_status: 'conflict' },
    ] satisfies ProjectUsedSkill[]);
    const viewer = await showCards();
    expect(within(cardOf(skillGroup(viewer, 'Skills Kronn'), 'Rust')).getByTestId('skill-group-badge-unsynced')).toHaveTextContent('Pas synchro');
    expect(within(cardOf(skillGroup(viewer, 'Mes skills'), 'Review')).getByTestId('skill-group-badge-unsynced')).toHaveTextContent('Pas synchro');
    expect(within(cardOf(skillGroup(viewer, 'Mes skills'), 'block-migration')).queryByTestId('skill-group-badge-unsynced')).toBeNull();
  });

  it('keeps the unused skills folded below both groups, and opens a card from either group', async () => {
    const viewer = await showCards();
    expect(availableToggle(2, viewer)).toHaveAttribute('aria-expanded', 'false');
    await act(async () => { fireEvent.click(within(skillGroup(viewer, 'Mes skills')).getByRole('button', { name: 'Ouvrir Review' })); });
    expect(screen.getByRole('heading', { level: 2, name: 'Review' })).toBeInTheDocument();
  });
});
