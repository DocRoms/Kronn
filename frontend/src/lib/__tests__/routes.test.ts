import { describe, expect, it } from 'vitest';
import {
  DEFAULT_PAGE, PAGE_PATHS, STANDALONE_PATHS, automationPath, automationTabFromPath, discussionPath, isAppPath, pathToPage,
  livePagePath, planningTaskPath, pluginPath, projectPath, sameAutomationSelection, standalonePagePath, workflowPath, type AutomationSelection, type DashboardPage,
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
