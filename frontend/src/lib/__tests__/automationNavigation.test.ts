import { afterEach, describe, expect, it, vi } from 'vitest';
import { isAutomationTab, readAutomationLastVisit, writeAutomationLastVisit } from '../automationNavigation';

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe('automation last visit', () => {
  it('starts on the workflows list', () => {
    expect(readAutomationLastVisit()).toEqual({ tab: 'workflows', resourceId: null });
  });

  it('reads back what was written', () => {
    writeAutomationLastVisit({ tab: 'quickPrompts', resourceId: 'qp-1' });
    expect(readAutomationLastVisit()).toEqual({ tab: 'quickPrompts', resourceId: 'qp-1' });

    writeAutomationLastVisit({ tab: 'skills', resourceId: null });
    expect(readAutomationLastVisit()).toEqual({ tab: 'skills', resourceId: null });
  });

  it.each([
    ['not json', { tab: 'workflows', resourceId: null }],
    ['{"tab":"elsewhere","resourceId":"x"}', { tab: 'workflows', resourceId: 'x' }],
    ['{"tab":"quickApis","resourceId":"  "}', { tab: 'quickApis', resourceId: null }],
    ['{"tab":"quickApis","resourceId":42}', { tab: 'quickApis', resourceId: null }],
  ])('falls back field by field on %s', (raw, visit) => {
    localStorage.setItem('kronn:automationNavigation', raw);
    expect(readAutomationLastVisit()).toEqual(visit);
  });

  it('survives a storage that refuses', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => { throw new Error('denied'); },
      setItem: () => { throw new Error('quota'); },
    });
    expect(() => writeAutomationLastVisit({ tab: 'skills', resourceId: 'kronn/a' })).not.toThrow();
    expect(readAutomationLastVisit()).toEqual({ tab: 'workflows', resourceId: null });
    vi.unstubAllGlobals();
  });

  it('knows the tabs', () => {
    for (const tab of ['workflows', 'quickPrompts', 'quickApis', 'quickExecs', 'skills']) expect(isAutomationTab(tab)).toBe(true);
    expect(isAutomationTab('Workflows')).toBe(false);
    expect(isAutomationTab(null)).toBe(false);
  });
});
