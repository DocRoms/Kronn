import { describe, it, expect } from 'vitest';
import { PAGE_PATHS, pathToPage } from '../routeConstants';
import type { Page } from '../routeConstants';

describe('PAGE_PATHS', () => {
  it('maps every page to a path starting with /', () => {
    for (const [page, path] of Object.entries(PAGE_PATHS)) {
      expect(path).toMatch(/^\//);
      expect(page).toBeTruthy();
    }
  });

  it('covers all 5 pages', () => {
    const pages: Page[] = ['projects', 'mcps', 'workflows', 'discussions', 'settings'];
    for (const p of pages) {
      expect(PAGE_PATHS[p]).toBeDefined();
    }
  });
});

describe('pathToPage', () => {
  it('returns the correct page for exact paths', () => {
    expect(pathToPage('/projects')).toBe('projects');
    expect(pathToPage('/discussions')).toBe('discussions');
    expect(pathToPage('/plugins')).toBe('mcps');
    expect(pathToPage('/workflows')).toBe('workflows');
    expect(pathToPage('/config')).toBe('settings');
  });

  it('returns the correct page for sub-paths', () => {
    expect(pathToPage('/projects/abc-123')).toBe('projects');
    expect(pathToPage('/discussions/def-456')).toBe('discussions');
    expect(pathToPage('/plugins/cfg-789')).toBe('mcps');
    expect(pathToPage('/workflows/wf-1/runs/run-2')).toBe('workflows');
  });

  it('returns workflows for QP and QA sub-routes', () => {
    expect(pathToPage('/workflows/qp')).toBe('workflows');
    expect(pathToPage('/workflows/qa')).toBe('workflows');
    expect(pathToPage('/workflows/qp/qp-123')).toBe('workflows');
    expect(pathToPage('/workflows/qa/qa-456')).toBe('workflows');
  });

  it('returns projects for the root path', () => {
    expect(pathToPage('/')).toBe('projects');
  });

  it('returns null for unknown paths', () => {
    expect(pathToPage('/unknown')).toBeNull();
    expect(pathToPage('/settings')).toBeNull();
    expect(pathToPage('/api/health')).toBeNull();
  });
});
