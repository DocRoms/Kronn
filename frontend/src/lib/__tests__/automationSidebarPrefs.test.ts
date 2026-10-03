import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  AUTOMATION_GROUP_BY_STORAGE_KEY,
  AUTOMATION_OPENED_LIMIT,
  AUTOMATION_OPENED_STORAGE_KEY,
  readAutomationGroupBy,
  readAutomationLastOpened,
  withAutomationOpened,
  writeAutomationGroupBy,
  writeAutomationLastOpened,
  type AutomationLastOpened,
} from '../automationSidebarPrefs';

afterEach(() => {
  localStorage.removeItem(AUTOMATION_GROUP_BY_STORAGE_KEY);
  localStorage.removeItem(AUTOMATION_OPENED_STORAGE_KEY);
  vi.restoreAllMocks();
});

describe('the "Group by" choice', () => {
  it('starts on Type and remembers what the user picked', () => {
    expect(readAutomationGroupBy()).toBe('kind');
    writeAutomationGroupBy('project');
    expect(localStorage.getItem(AUTOMATION_GROUP_BY_STORAGE_KEY)).toBe('project');
    expect(readAutomationGroupBy()).toBe('project');
    writeAutomationGroupBy('none');
    expect(readAutomationGroupBy()).toBe('none');
  });

  it('falls back to Type on a value it does not know', () => {
    localStorage.setItem(AUTOMATION_GROUP_BY_STORAGE_KEY, 'folder');
    expect(readAutomationGroupBy()).toBe('kind');
  });

  it('survives a localStorage that refuses to read or to write', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('full'); });
    expect(readAutomationGroupBy()).toBe('kind');
    expect(() => writeAutomationGroupBy('none')).not.toThrow();
    expect(readAutomationLastOpened()).toEqual({});
    expect(() => writeAutomationLastOpened({ 'workflows:a': 1 })).not.toThrow();
  });
});

describe('the history of what was opened', () => {
  it('reads nothing from an empty, corrupted or foreign value', () => {
    expect(readAutomationLastOpened()).toEqual({});
    localStorage.setItem(AUTOMATION_OPENED_STORAGE_KEY, '{not json');
    expect(readAutomationLastOpened()).toEqual({});
    localStorage.setItem(AUTOMATION_OPENED_STORAGE_KEY, '[1,2]');
    expect(readAutomationLastOpened()).toEqual({});
    localStorage.setItem(AUTOMATION_OPENED_STORAGE_KEY, '"text"');
    expect(readAutomationLastOpened()).toEqual({});
  });

  it('keeps only the entries that are a real date', () => {
    localStorage.setItem(AUTOMATION_OPENED_STORAGE_KEY, JSON.stringify({
      'workflows:a': 1700000000000, 'skills:b': 'yesterday', 'quickApis:c': null, 'quickExecs:d': -4, 'quickPrompts:e': 0,
    }));
    expect(readAutomationLastOpened()).toEqual({ 'workflows:a': 1700000000000 });
  });

  it('writes what it reads back', () => {
    writeAutomationLastOpened({ 'workflows:a': 10, 'skills:b': 20 });
    expect(readAutomationLastOpened()).toEqual({ 'workflows:a': 10, 'skills:b': 20 });
  });

  it('makes the automation just opened the most recent, without touching the others', () => {
    const history = { 'workflows:a': 100, 'skills:b': 200 };
    expect(withAutomationOpened(history, 'quickApis:c', 500)).toEqual({ 'workflows:a': 100, 'skills:b': 200, 'quickApis:c': 500 });
    // Opening it again only moves its own date.
    expect(withAutomationOpened(history, 'workflows:a', 900)).toEqual({ 'workflows:a': 900, 'skills:b': 200 });
    expect(history).toEqual({ 'workflows:a': 100, 'skills:b': 200 });
  });

  it('keeps two openings in the same millisecond in order', () => {
    const first = withAutomationOpened({}, 'workflows:a', 1000);
    const second = withAutomationOpened(first, 'workflows:b', 1000);
    expect(second['workflows:b']).toBeGreaterThan(second['workflows:a']);
    // A clock that went back never puts the newest opening behind an older one.
    const third = withAutomationOpened(second, 'workflows:c', 10);
    expect(third['workflows:c']).toBeGreaterThan(third['workflows:b']);
  });

  it('forgets the oldest opening once the history is full', () => {
    let history: AutomationLastOpened = {};
    for (let index = 1; index <= AUTOMATION_OPENED_LIMIT + 3; index += 1) {
      history = withAutomationOpened(history, `workflows:${index}`, index * 10);
    }
    const ids = Object.keys(history);
    expect(ids).toHaveLength(AUTOMATION_OPENED_LIMIT);
    expect(ids).not.toContain('workflows:1');
    expect(ids).not.toContain('workflows:3');
    expect(ids).toContain('workflows:4');
    expect(ids).toContain(`workflows:${AUTOMATION_OPENED_LIMIT + 3}`);
  });
});
