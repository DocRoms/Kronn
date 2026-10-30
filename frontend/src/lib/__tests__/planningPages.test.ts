import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ list: vi.fn() }));

vi.mock('../api', () => ({ planning: { list: mocks.list } }));

import { listAllPlanningTasks, MAX_PLANNING_PAGES } from '../planningPages';
import type { PlanningTaskListResponse, PlanningTaskSummary } from '../../types/generated';

const task = (id: string) => ({ id }) as PlanningTaskSummary;

function page(overrides: Partial<PlanningTaskListResponse>): PlanningTaskListResponse {
  return {
    items: [task('t')],
    next_cursor: null,
    total: 1,
    project_totals: {},
    unassigned_total: 0,
    ...overrides,
  };
}

describe('listAllPlanningTasks', () => {
  beforeEach(() => mocks.list.mockReset());

  it('takes the grouped counts from the first page, the only one that carries them', async () => {
    mocks.list
      .mockResolvedValueOnce(page({ next_cursor: 100, total: 2, project_totals: { a: 2 }, unassigned_total: 0 }))
      .mockResolvedValueOnce(page({ total: 2, project_totals: {}, unassigned_total: 0 }));

    const result = await listAllPlanningTasks({});

    expect(result).toMatchObject({ total: 2, project_totals: { a: 2 }, truncated: false });
    expect(result?.items).toHaveLength(2);
  });

  it('stops at the page ceiling and says the list is truncated', async () => {
    mocks.list.mockImplementation(async (filters?: { cursor?: number }) => {
      const start = filters?.cursor ?? 0;
      return page({ next_cursor: start + 100, total: 999_999 });
    });

    const result = await listAllPlanningTasks({});

    expect(mocks.list).toHaveBeenCalledTimes(MAX_PLANNING_PAGES);
    expect(result).toMatchObject({ total: 999_999, truncated: true });
    expect(result?.items).toHaveLength(MAX_PLANNING_PAGES);
  });

  it.each([
    ['repeats', 100],
    ['goes back to', 50],
  ])('stops, without saying it truncated, when the next cursor %s the one just read', async (_, next) => {
    mocks.list.mockImplementation(async (filters?: { cursor?: number }) => (
      page({ next_cursor: filters?.cursor === undefined ? 100 : next, total: 2 })
    ));

    const result = await listAllPlanningTasks({});

    expect(mocks.list).toHaveBeenCalledTimes(2);
    expect(result).toMatchObject({ total: 2, truncated: false });
    expect(result?.items).toHaveLength(2);
  });

  it('stops paging once superseded', async () => {
    mocks.list.mockResolvedValue(page({ next_cursor: 100 }));
    expect(await listAllPlanningTasks({}, () => true)).toBeNull();
    expect(mocks.list).toHaveBeenCalledTimes(1);
  });
});
