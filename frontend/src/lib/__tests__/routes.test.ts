import { describe, expect, it } from 'vitest';
import {
  DEFAULT_PAGE, DESIGN_SYSTEM_PATH, DESIGN_SYSTEM_TABS, PAGE_PATHS, STANDALONE_PATHS, automationEditorFromPath,
  designSystemAnchorTab, designSystemPath, designSystemTabFromSegment, legacyDesignSystemLocation, automationPath, automationTabFromPath, discussionPath, isAppPath, pathToPage,
  livePagePath, planningTaskPath, pluginPath, projectLocation, projectPath, sameAutomationSelection, settingsSectionPath, standalonePagePath, workflowPath, workflowShadowSettingsPath, type AutomationSelection, type DashboardPage,
  DEFAULT_SETTINGS_SUB_PAGE, EMBED_SETTINGS_PATH, SETTINGS_SECTIONS, SETTINGS_SUB_PAGES, embedSettingsPath, settingsFoldOf,
  settingsRedirectPath, settingsSectionOf, settingsSectionsOf, settingsSubPageFromSegment, settingsSubPagePath,
  type SettingsSubPage,
} from '../routes';

describe('routes', () => {
  it('gives every page its own absolute path', () => {
    const paths = Object.values(PAGE_PATHS);
    expect(new Set(paths).size).toBe(paths.length);
    for (const path of paths) expect(path).toMatch(/^\/[a-z]+$/);
  });

  it('round-trips every page through its path', () => {
    for (const page of Object.keys(PAGE_PATHS) as DashboardPage[]) {
      expect(pathToPage(PAGE_PATHS[page])).toBe(page);
    }
  });

  it('keeps the default page addressable', () => {
    expect(pathToPage(PAGE_PATHS[DEFAULT_PAGE])).toBe(DEFAULT_PAGE);
  });

  it('assigns a whole subtree to its page', () => {
    expect(pathToPage('/projects/abc')).toBe('projects');
    expect(pathToPage('/workflows/wf-1/runs/run-2')).toBe('workflows');
    expect(pathToPage('/plugins/cfg-1')).toBe('mcps');
    expect(pathToPage('/config')).toBe('settings');
  });

  it('ignores trailing slashes', () => {
    expect(pathToPage('/discussions/')).toBe('discussions');
    expect(pathToPage('/planning//')).toBe('planning');
  });

  it('does not match a path that merely starts like a page', () => {
    expect(pathToPage('/projectsfoo')).toBeNull();
    expect(pathToPage('/configuration')).toBeNull();
  });

  it('returns null for anything that is not a dashboard address', () => {
    expect(pathToPage('/')).toBeNull();
    expect(pathToPage('')).toBeNull();
    expect(pathToPage('/api/health')).toBeNull();
    expect(pathToPage('/Projects')).toBeNull();
    expect(pathToPage('/discussions-été')).toBeNull();
  });
});

describe('project addresses', () => {
  it('encodes the project id under the Projects page', () => {
    expect(projectPath('proj-1')).toBe('/projects/proj-1');
    expect(projectPath('a/b c')).toBe('/projects/a%2Fb%20c');
    expect(pathToPage(projectPath('proj-1'))).toBe('projects');
  });

  it('names a view of the project, and what the code or docs view shows', () => {
    expect(projectPath('proj-1', { view: 'git' })).toBe('/projects/proj-1/git');
    expect(projectPath('proj-1', { view: 'code', file: 'src/a b.ts', line: 12 })).toBe('/projects/proj-1/code?file=src%2Fa+b.ts&line=12');
    expect(projectPath('proj-1', { view: 'docs', folder: 'docs/tech-debt' })).toBe('/projects/proj-1/docs?folder=docs%2Ftech-debt');
    // A file only means something to the code view, a folder to the docs view.
    expect(projectPath('proj-1', { view: 'git', file: 'x', folder: 'y' })).toBe('/projects/proj-1/git');
    expect(pathToPage(projectPath('proj-1', { view: 'code' }))).toBe('projects');
  });

  it('reads back what a project address names, and ignores what it cannot', () => {
    const at = (path: string) => {
      const url = new URL(path, 'http://localhost');
      return projectLocation(url.pathname.split('/')[3], url.searchParams);
    };
    expect(at(projectPath('p', { view: 'code', file: 'src/a b.ts', line: 12 })))
      .toEqual({ view: 'code', file: 'src/a b.ts', line: 12, folder: null });
    expect(at(projectPath('p', { view: 'docs', folder: 'docs/tech-debt' })))
      .toEqual({ view: 'docs', file: null, line: null, folder: 'docs/tech-debt' });
    expect(at('/projects/p/code?file=a.ts&line=-3')).toEqual({ view: 'code', file: 'a.ts', line: null, folder: null });
    expect(at('/projects/p/nope')).toBeNull();
    expect(at('/projects/p')).toBeNull();
  });
});

describe('planning addresses', () => {
  it('encodes the task id under the Planning page', () => {
    expect(planningTaskPath('task-4')).toBe('/planning/task-4');
    expect(planningTaskPath('KT 4/é')).toBe('/planning/KT%204%2F%C3%A9');
    expect(pathToPage(planningTaskPath('task-4'))).toBe('planning');
  });
});

describe('Artifact addresses', () => {
  it('encodes the Page id under the Artifacts page, apart from its standalone view', () => {
    expect(livePagePath('page-7')).toBe('/pages/page-7');
    expect(livePagePath('page/7')).toBe('/pages/page%2F7');
    expect(pathToPage(livePagePath('page-7'))).toBe('pages');
    expect(livePagePath('page-7')).not.toBe(standalonePagePath('page-7'));
  });
});

describe('plugin addresses', () => {
  it('encodes the config id under the Plugins page', () => {
    expect(pluginPath('cfg-9')).toBe('/plugins/cfg-9');
    expect(pluginPath('cfg/9 é')).toBe('/plugins/cfg%2F9%20%C3%A9');
    expect(pathToPage(pluginPath('cfg-9'))).toBe('mcps');
  });
});

describe('automation addresses', () => {
  const cases: [AutomationSelection, string][] = [
    [{ tab: 'workflows', resourceId: null }, '/workflows'],
    [{ tab: 'workflows', resourceId: 'wf/1' }, '/workflows/wf%2F1'],
    [{ tab: 'workflows', resourceId: 'wf-1', runId: 'run 2' }, '/workflows/wf-1/runs/run%202'],
    [{ tab: 'quickPrompts', resourceId: null }, '/workflows/qp'],
    [{ tab: 'quickPrompts', resourceId: 'qp-1' }, '/workflows/qp/qp-1'],
    [{ tab: 'quickApis', resourceId: 'qa-1' }, '/workflows/qa/qa-1'],
    [{ tab: 'quickExecs', resourceId: 'qe-1' }, '/workflows/qe/qe-1'],
    [{ tab: 'skills', resourceId: null }, '/workflows/skills'],
    [{ tab: 'skills', resourceId: 'kronn/skill' }, '/workflows/skills/kronn%2Fskill'],
  ];

  it.each(cases)('writes %o as %s', (selection, path) => {
    expect(automationPath(selection)).toBe(path);
    expect(pathToPage(path)).toBe('workflows');
    expect(automationTabFromPath(path)).toBe(selection.tab);
  });

  it('ignores a run outside the workflows tab, and without a workflow', () => {
    expect(automationPath({ tab: 'quickPrompts', resourceId: 'qp-1', runId: 'run-2' })).toBe('/workflows/qp/qp-1');
    expect(automationPath({ tab: 'workflows', resourceId: null, runId: 'run-2' })).toBe('/workflows');
    expect(workflowPath('wf-1')).toBe('/workflows/wf-1');
    expect(workflowPath('wf-1', 'run-2')).toBe('/workflows/wf-1/runs/run-2');
  });

  it('reads an unknown second segment as a workflow id', () => {
    expect(automationTabFromPath('/workflows/anything')).toBe('workflows');
    expect(automationTabFromPath('/workflows/')).toBe('workflows');
    expect(automationTabFromPath('/workflows/qpx')).toBe('workflows');
  });

  it('compares selections by what they show', () => {
    expect(sameAutomationSelection({ tab: 'workflows', resourceId: 'a' }, { tab: 'workflows', resourceId: 'a', runId: null })).toBe(true);
    expect(sameAutomationSelection({ tab: 'workflows', resourceId: 'a' }, { tab: 'workflows', resourceId: 'a', runId: 'r' })).toBe(false);
    expect(sameAutomationSelection({ tab: 'quickApis', resourceId: 'a' }, { tab: 'quickPrompts', resourceId: 'a' })).toBe(false);
  });
});

describe('discussion addresses', () => {
  it('encodes the discussion id under the Discussions page', () => {
    expect(discussionPath('disc-1')).toBe('/discussions/disc-1');
    expect(discussionPath('disc/é 42')).toBe('/discussions/disc%2F%C3%A9%2042');
    expect(pathToPage(discussionPath('disc-1'))).toBe('discussions');
  });

  it('names a message in the query, and only when there is one', () => {
    expect(discussionPath('disc-1', 'msg/🦀')).toBe('/discussions/disc-1?message=msg%2F%F0%9F%A6%80');
    expect(discussionPath('disc-1', null)).toBe('/discussions/disc-1');
    expect(discussionPath('disc-1', '')).toBe('/discussions/disc-1');
  });
});

describe('standalone addresses', () => {
  it('encodes the page id into its path', () => {
    expect(standalonePagePath('page-1')).toBe('/standalone/pages/page-1');
    expect(standalonePagePath('page/équipe')).toBe('/standalone/pages/page%2F%C3%A9quipe');
    expect(standalonePagePath('a b?c')).toBe('/standalone/pages/a%20b%3Fc');
  });

  it('keeps the mosaics out of the page id space', () => {
    expect(STANDALONE_PATHS.pagesMosaic.startsWith(`${STANDALONE_PATHS.page}/`)).toBe(true);
    expect(STANDALONE_PATHS.discussionsMosaic.startsWith(STANDALONE_PATHS.page)).toBe(false);
  });

  it('recognises every address of the app, and nothing else', () => {
    for (const path of ['/', ...Object.values(PAGE_PATHS), '/discussions/abc', standalonePagePath('p'), STANDALONE_PATHS.pagesMosaic, STANDALONE_PATHS.discussionsMosaic, '/standalone']) {
      expect(isAppPath(path), path).toBe(true);
    }
    for (const path of ['', '/api/health', '/assets/index.js', '/standalonefoo', '/index.html', '/Projects']) {
      expect(isAppPath(path), path).toBe(false);
    }
  });
});

describe('workflow wizard addresses', () => {
  it('names the wizard, creating or editing', () => {
    expect(automationPath({ tab: 'workflows', resourceId: null, editor: 'create' })).toBe('/workflows/new');
    expect(automationPath({ tab: 'workflows', resourceId: 'wf 1', editor: 'edit' })).toBe('/workflows/wf%201/edit');
    // Editing needs a workflow; without one it is the list.
    expect(automationPath({ tab: 'workflows', resourceId: null, editor: 'edit' })).toBe('/workflows');
    expect(automationTabFromPath('/workflows/new')).toBe('workflows');
  });

  it('names the wizard opened on the shadow settings', () => {
    expect(workflowShadowSettingsPath('wf 1')).toBe('/workflows/wf%201/edit#shadow');
    expect(automationEditorFromPath('/workflows/wf%201/edit')).toBe('edit');
  });

  it('reads the wizard back from the address, and nothing else as one', () => {
    expect(automationEditorFromPath('/workflows/new')).toBe('create');
    expect(automationEditorFromPath('/workflows/wf-1/edit')).toBe('edit');
    expect(automationEditorFromPath('/workflows/wf-1')).toBeNull();
    expect(automationEditorFromPath('/workflows/wf-1/runs/edit')).toBeNull();
    expect(automationEditorFromPath('/workflows/qp/new')).toBeNull();
  });

  it('tells two selections apart by the wizard too', () => {
    const list = { tab: 'workflows' as const, resourceId: null, runId: null };
    expect(sameAutomationSelection(list, { ...list, editor: 'create' })).toBe(false);
    expect(sameAutomationSelection(list, { ...list, editor: null })).toBe(true);
  });
});

describe('Configuration sections (KT-1923: five sub-pages)', () => {
  /** Every anchor the single page had (its section index, `credentials`,
   *  `artifacts`, the groups and fields linked from elsewhere), and the
   *  sub-page the owner's grouping of 2026-11-21 puts it on. */
  const FORMER_ANCHORS: readonly (readonly [string, SettingsSubPage])[] = [
    ['settings-identity', 'you'],
    ['settings-appearance', 'you'],
    ['settings-languages', 'you'],
    ['settings-voice', 'you'],
    ['settings-user-context', 'you'],
    ['settings-agent-config', 'agents'],
    ['settings-agent-handoffs', 'agents'],
    ['settings-usage', 'agents'],
    ['settings-capabilities', 'agents'],
    ['settings-skills', 'agents'],
    ['settings-profiles', 'agents'],
    ['settings-directives', 'agents'],
    ['settings-host-mcps', 'agents'],
    ['settings-scan', 'work'],
    ['settings-workers', 'work'],
    ['settings-worktrees', 'work'],
    ['settings-sourcing', 'work'],
    ['settings-continual-learning', 'work'],
    ['settings-artifacts', 'work'],
    ['settings-recovery', 'security'],
    ['settings-authenticator', 'security'],
    ['settings-trust', 'security'],
    ['settings-server', 'security'],
    ['credentials', 'security'],
    ['settings-database', 'system'],
    ['run-payload-retention', 'system'],
    ['settings-disk', 'system'],
    ['settings-api-audit', 'system'],
    ['settings-debug', 'system'],
    ['settings-general', 'system'],
  ];
  /** The sections that were nested inside Agents, listed now with their own anchor. */
  const NEW_SECTIONS = ['settings-local-models', 'settings-media', 'settings-compression', 'settings-catalogue'];

  it('has one address per sub-page under /config, with language-neutral segments', () => {
    expect(SETTINGS_SUB_PAGES.map(page => settingsSubPagePath(page))).toEqual([
      '/config/you', '/config/agents', '/config/work', '/config/security', '/config/system',
    ]);
    expect(settingsSubPagePath('agents', 'settings-usage')).toBe('/config/agents#settings-usage');
    for (const page of SETTINGS_SUB_PAGES) {
      expect(pathToPage(settingsSubPagePath(page))).toBe('settings');
      expect(settingsSubPageFromSegment(page)).toBe(page);
    }
    expect(settingsSubPageFromSegment(undefined)).toBeNull();
    expect(settingsSubPageFromSegment('design-system')).toBeNull();
    expect(DEFAULT_SETTINGS_SUB_PAGE).toBe('you');
  });

  it.each(FORMER_ANCHORS)('sends the anchor %s to its sub-page (%s), with the same signature for the callers', (anchor, page) => {
    expect(settingsSectionPath(anchor)).toBe(`/config/${page}#${anchor}`);
    expect(settingsSectionOf(anchor)?.page).toBe(page);
    expect(settingsRedirectPath('/config', `#${anchor}`, undefined)).toBe(`/config/${page}#${anchor}`);
    // Already there: nothing to redirect.
    expect(settingsRedirectPath(`/config/${page}`, `#${anchor}`, page)).toBeNull();
  });

  it('groups the sections in the order of the validated mockup', () => {
    const ids = (page: SettingsSubPage) => settingsSectionsOf(page).map(section => section.id);
    expect(ids('you')).toEqual(['settings-identity', 'settings-appearance', 'settings-voice', 'settings-user-context']);
    expect(ids('agents')).toEqual([
      'settings-agent-config', 'settings-local-models', 'settings-media', 'settings-compression',
      'settings-catalogue', 'settings-usage', 'settings-capabilities',
    ]);
    expect(ids('work')).toEqual([
      'settings-scan', 'settings-workers', 'settings-worktrees', 'settings-sourcing', 'settings-continual-learning',
      'settings-artifacts',
    ]);
    expect(ids('security')).toEqual([
      'settings-recovery', 'settings-authenticator', 'settings-trust', 'settings-server', 'credentials',
    ]);
    expect(ids('system')).toEqual([
      'settings-database', 'settings-disk', 'settings-api-audit', 'settings-debug', 'settings-general',
    ]);
    for (const id of NEW_SECTIONS) expect(settingsSectionPath(id)).toBe(`/config/agents#${id}`);
    // Every section is on exactly one sub-page.
    expect(new Set(SETTINGS_SECTIONS.map(section => section.id)).size).toBe(SETTINGS_SECTIONS.length);
  });

  it('opens a folded section when an anchor lands on it', () => {
    expect(settingsFoldOf('settings-catalogue')).toBe('settings-catalogue');
    expect(settingsFoldOf('settings-usage')).toBe('settings-usage');
    expect(settingsFoldOf('settings-api-audit')).toBe('settings-api-audit');
    expect(settingsFoldOf('settings-debug-logs')).toBe('settings-debug-logs');
    expect(settingsSectionPath('settings-debug-logs')).toBe('/config/system#settings-debug-logs');
    expect(settingsFoldOf('settings-debug')).toBeNull();
    expect(settingsFoldOf('settings-server')).toBeNull();
    expect(settingsFoldOf('nope')).toBeNull();
    expect(settingsFoldOf(null)).toBeNull();
  });

  it('keeps an old alias, the rows drawn per agent, and an unknown anchor without an error', () => {
    expect(settingsSectionPath('artifacts')).toBe('/config/work#settings-artifacts');
    expect(settingsSectionPath('agent-config-ClaudeCode')).toBe('/config/agents#agent-config-ClaudeCode');
    expect(settingsSectionOf('agent-config-')).toBeNull();
    // No section names it: Configuration itself, which opens its first sub-page.
    expect(settingsSectionPath('nope')).toBe('/config#nope');
    expect(settingsRedirectPath('/config', '#nope', undefined)).toBe('/config/you');
    expect(settingsRedirectPath('/config', '', undefined)).toBe('/config/you');
    expect(settingsRedirectPath('/config/', '', undefined)).toBe('/config/you');
    expect(settingsRedirectPath('/config/elsewhere', '', 'elsewhere')).toBe('/config/you');
    expect(settingsRedirectPath('/config/work', '#nope', 'work')).toBeNull();
    expect(settingsRedirectPath('/config/work', '#%E0%A4%A', 'work')).toBeNull();
    // An anchor asked on another sub-page's address moves to its own.
    expect(settingsRedirectPath('/config/system', '#settings-usage', 'system')).toBe('/config/agents#settings-usage');
    expect(settingsRedirectPath('/config/work', '#artifacts', 'work')).toBe('/config/work#settings-artifacts');
  });

  it('leaves the embedded Settings address and the design system addresses as they were', () => {
    expect(EMBED_SETTINGS_PATH).toBe('/config/artifacts');
    expect(embedSettingsPath('https://vimeo.com')).toBe('/config/artifacts?origin=https%3A%2F%2Fvimeo.com');
    expect(DESIGN_SYSTEM_PATH).toBe('/config/design-system');
    expect(settingsSubPageFromSegment('artifacts')).toBeNull();
    expect(legacyDesignSystemLocation('settings-appearance')).toBeNull();
  });
});

describe('design system sub-page (KT-1987)', () => {
  it('lives under Configuration: the tokens at its root, the components on their segment', () => {
    expect(DESIGN_SYSTEM_PATH).toBe('/config/design-system');
    expect(designSystemPath()).toBe('/config/design-system');
    expect(designSystemPath({ tab: 'components' })).toBe('/config/design-system/components');
    expect(designSystemPath({ tab: 'components', anchor: 'ds-c-menu' })).toBe('/config/design-system/components#ds-c-menu');
    expect(designSystemPath({ tab: 'tokens', anchor: 'ds-text' })).toBe('/config/design-system#ds-text');
    expect(pathToPage(designSystemPath({ tab: 'components' }))).toBe('settings');
  });

  it('reads the tab back from its segment, and nothing from an unknown one', () => {
    for (const tab of DESIGN_SYSTEM_TABS) {
      const segment = designSystemPath({ tab }).slice(DESIGN_SYSTEM_PATH.length + 1) || undefined;
      expect(designSystemTabFromSegment(segment)).toBe(tab);
    }
    expect(designSystemTabFromSegment('tokens-and-more')).toBeNull();
  });

  it.each([
    ['design-system', { tab: 'tokens', anchor: null }],
    ['ds-tokens', { tab: 'tokens', anchor: 'ds-tokens' }],
    ['ds-colour', { tab: 'tokens', anchor: 'ds-colour' }],
    ['ds-components', { tab: 'components', anchor: 'ds-components' }],
    ['ds-c-button', { tab: 'components', anchor: 'ds-c-button' }],
    ['ds-c-confirmDialog', { tab: 'components', anchor: 'ds-c-confirmDialog' }],
  ] as const)('lands the old anchor /config#%s on its tab and entry', (anchor, location) => {
    expect(legacyDesignSystemLocation(anchor)).toEqual(location);
  });

  it.each([null, '', 'settings-server', 'settings-appearance', 'run-payload-retention', 'design-systems'])(
    'leaves the anchor %s to Configuration',
    anchor => {
      expect(legacyDesignSystemLocation(anchor)).toBeNull();
    },
  );

  it('names the tab of an entry anchor', () => {
    expect(designSystemAnchorTab('ds-c-menu')).toBe('components');
    expect(designSystemAnchorTab('ds-space')).toBe('tokens');
    expect(designSystemAnchorTab('settings-server')).toBeNull();
  });
});
