import { beforeEach, describe, expect, it } from 'vitest';
import type { ActivityRow } from '../../types/generated';
import { categoryOfRow, countByCategory, filterStorageKey, readFilter, writeFilter } from '../activityFilters';

const row = (type: string, link?: ActivityRow['link']): ActivityRow => ({
  id: type, type, compartment: 'running', state: 'running', source: 't', title: type,
  started_at: '2026-11-10T11:56:00Z', stalled: false, actions: [], link,
});

describe('categories', () => {
  it('sorts the known row types, and a type it does not know by where it points', () => {
    expect(categoryOfRow(row('agent_turn'))).toBe('discussions');
    expect(categoryOfRow(row('workflow_run'))).toBe('automations');
    expect(categoryOfRow(row('audit'))).toBe('kronn');
    expect(categoryOfRow(row('task_worker'))).toBe('kronn');
    expect(categoryOfRow(row('media_job', { target: 'discussion', id: 'd' }))).toBe('discussions');
    expect(categoryOfRow(row('media_job', { target: 'workflow', id: 'w' }))).toBe('automations');
    expect(categoryOfRow(row('media_job'))).toBe('kronn');
  });

  it('counts per chip, with All as the total', () => {
    const rows = [row('agent_turn'), row('agent_turn'), row('workflow_run'), row('audit')];
    expect(countByCategory(rows, categoryOfRow)).toEqual({ all: 4, discussions: 2, automations: 1, kronn: 1 });
  });
});

describe('the remembered filter', () => {
  beforeEach(() => { localStorage.clear(); });

  it('is All until a choice is made, and each tab remembers its own', () => {
    expect(readFilter('progress')).toBe('all');
    writeFilter('progress', 'automations');
    writeFilter('notifications', 'kronn');
    expect(readFilter('progress')).toBe('automations');
    expect(readFilter('notifications')).toBe('kronn');
  });

  it('falls back to All on a value it does not know', () => {
    localStorage.setItem(filterStorageKey('progress'), 'nonsense');
    expect(readFilter('progress')).toBe('all');
  });
});
