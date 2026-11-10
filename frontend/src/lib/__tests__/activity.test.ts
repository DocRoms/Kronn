import { describe, expect, it, vi } from 'vitest';
import type { ActivityOverview, ActivityRow } from '../../types/generated';
import {
  ACTIVITY_COMPARTMENTS,
  actionLabelKey,
  activityLinkPath,
  formatDuration,
  humanizeType,
  isAnythingRunning,
  openActivityLink,
  rowsOfCompartment,
  sameActivity,
  translateOr,
  waitingOnOwnerCount,
} from '../activity';

vi.mock('../api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

const row = (overrides: Partial<ActivityRow> & Pick<ActivityRow, 'id' | 'type'>): ActivityRow => ({
  compartment: 'running', state: 'running', source: 'test', title: overrides.id,
  started_at: '2026-11-10T11:56:00Z', stalled: false, actions: [], ...overrides,
});
const overview = (rows: ActivityRow[], generated_at = '2026-11-10T12:00:00Z'): ActivityOverview => ({
  rows, stall_after_secs: 900, generated_at, unavailable_sources: [],
});

describe('activity compartments', () => {
  it('reserves the KT-334 compartments around "running", in reading order', () => {
    expect(ACTIVITY_COMPARTMENTS.map(c => c.id)).toEqual(['needs_decision', 'running', 'unseen_done']);
    expect(ACTIVITY_COMPARTMENTS.filter(c => c.alwaysShown).map(c => c.id)).toEqual(['running']);
  });

  it('keeps the rows of one compartment: working, then queued, then paused, oldest first', () => {
    const rows = [
      row({ id: 'p', type: 't', state: 'paused', started_at: '2026-11-10T10:00:00Z' }),
      row({ id: 'q', type: 't', state: 'queued', started_at: '2026-11-10T10:30:00Z' }),
      row({ id: 'new', type: 't', started_at: '2026-11-10T11:50:00Z' }),
      row({ id: 'old', type: 't', started_at: '2026-11-10T11:00:00Z' }),
      row({ id: 'other', type: 't', compartment: 'unseen_done' }),
    ];
    expect(rowsOfCompartment(rows, 'running').map(r => r.id)).toEqual(['old', 'new', 'q', 'p']);
    expect(rowsOfCompartment(rows, 'unseen_done').map(r => r.id)).toEqual(['other']);
  });
});

describe('what the bell reads', () => {
  it('moves for working or queued rows only, never for a row that is merely paused', () => {
    expect(isAnythingRunning(null)).toBe(false);
    expect(isAnythingRunning(overview([]))).toBe(false);
    expect(isAnythingRunning(overview([row({ id: 'p', type: 't', state: 'paused' })]))).toBe(false);
    expect(isAnythingRunning(overview([row({ id: 'q', type: 't', state: 'queued' })]))).toBe(true);
    expect(isAnythingRunning(overview([row({ id: 'r', type: 't' })]))).toBe(true);
  });

  it('counts what waits for the owner from the decision compartment only', () => {
    expect(waitingOnOwnerCount(null)).toBe(0);
    expect(waitingOnOwnerCount(overview([
      row({ id: 'a', type: 't', compartment: 'needs_decision' }),
      row({ id: 'b', type: 't' }),
      row({ id: 'c', type: 't', compartment: 'unseen_done' }),
    ]))).toBe(1);
  });

  it('treats a poll that found nothing new as the same view, whatever its clock says', () => {
    const a = overview([row({ id: 'a', type: 't' })], '2026-11-10T12:00:00Z');
    expect(sameActivity(a, overview([row({ id: 'a', type: 't' })], '2026-11-10T12:00:03Z'))).toBe(true);
    expect(sameActivity(a, overview([row({ id: 'a', type: 't', stalled: true })]))).toBe(false);
    expect(sameActivity(null, a)).toBe(false);
    expect(sameActivity(null, null)).toBe(true);
  });
});

describe('formatting', () => {
  it.each([
    [0, '0s'], [59_000, '59s'], [60_000, '1m 00s'], [4 * 60_000 + 12_000, '4m 12s'],
    [63 * 60_000, '1h 03m'], [49 * 3_600_000, '2d 01h'], [-5_000, '0s'],
  ])('reads %i ms as %s', (ms, text) => { expect(formatDuration(ms)).toBe(text); });

  it('names a row type the locales do not know', () => {
    expect(humanizeType('media_job')).toBe('Media job');
    expect(humanizeType('')).toBe('');
    const t = (key: string) => (key === 'known' ? 'Connu' : key);
    expect(translateOr(t, 'known', 'x')).toBe('Connu');
    expect(translateOr(t, 'unknown', 'Fallback')).toBe('Fallback');
  });

  it('labels an action by what it does, not by the row it sits on', () => {
    expect(actionLabelKey({ id: 'anything', kind: 'resume_now', args: {} })).toBe('activity.action.resume_now');
  });
});

describe('deep links', () => {
  it.each([
    [{ target: 'discussion', id: 'd 1' }, '/discussions/d%201'],
    [{ target: 'project', id: 'p1' }, '/projects/p1'],
    [{ target: 'planning_task', id: 't1' }, '/planning/t1'],
    [{ target: 'workflow', id: 'wf-1' }, '/workflows/wf-1'],
  ] as const)('resolves %j to its address from the route table', (link, path) => {
    expect(activityLinkPath(link)).toBe(path);
  });

  it('reveals the run of a workflow link', () => {
    expect(activityLinkPath({ target: 'workflow', id: 'wf-1', run_id: 'r1' })).toContain('r1');
  });

  it('opens each target through the typed navigation', () => {
    const nav = {
      toDiscussion: vi.fn(), toWorkflow: vi.fn(), toProject: vi.fn(), toPlanningTask: vi.fn(),
    } as unknown as Parameters<typeof openActivityLink>[0];
    openActivityLink(nav, { target: 'discussion', id: 'd1' });
    openActivityLink(nav, { target: 'workflow', id: 'wf', run_id: 'r1' });
    openActivityLink(nav, { target: 'project', id: 'p1' });
    openActivityLink(nav, { target: 'planning_task', id: 't1' });
    expect(nav.toDiscussion).toHaveBeenCalledWith('d1');
    expect(nav.toWorkflow).toHaveBeenCalledWith('wf', 'r1');
    expect(nav.toProject).toHaveBeenCalledWith('p1');
    expect(nav.toPlanningTask).toHaveBeenCalledWith('t1');
  });
});
