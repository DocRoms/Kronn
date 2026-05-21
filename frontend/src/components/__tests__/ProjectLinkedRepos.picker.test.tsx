import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// 0.8.6 (#27 fix 2026-05-21) — the "+ N autres" overflow label was a
// static <span> until the user pointed out that they couldn't click it
// to reveal the hidden candidates. These tests pin the new behaviour:
// the cap is 12 by default, an explicit button flips a `showAll` state
// that expands the list, and a "Voir moins" button collapses back.

// vi.mock is hoisted so the factory cannot reference module-level
// values. We use vi.hoisted to share a mutable holder between the
// factory + test bodies.
const mockState = vi.hoisted(() => {
  const MANY = Array.from({ length: 19 }, (_, i) => ({
    id: `p-${i}`,
    name: `Project${String(i).padStart(2, '0')}`,
    path: `/repos/proj-${i}`,
    proximity_hint: i < 5 ? 'same-parent' : 'other',
  }));
  return {
    MANY_CANDIDATES: MANY,
    FEW_CANDIDATES: MANY.slice(0, 8),
    // Mutable — beforeEach swaps it between MANY and FEW per test.
    currentCandidates: MANY as typeof MANY,
  };
});

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock({
    projects: {
      linkedReposCandidates: vi.fn().mockImplementation(() => Promise.resolve(mockState.currentCandidates)),
      setLinkedRepos: vi.fn().mockResolvedValue(undefined),
    },
  });
});

import { ProjectLinkedRepos } from '../ProjectLinkedRepos';

function wrap(ui: React.ReactElement) {
  return render(<I18nProvider>{ui}</I18nProvider>);
}

async function openDraft() {
  // The "Add" button at the bottom of the list opens the draft form.
  // It's the only button with the linkedRepos.add label visible by default.
  const addBtn = await screen.findByRole('button', { name: /linkedRepos\.add|ajouter/i });
  fireEvent.click(addBtn);
  // Wait for the candidates fetch to resolve + the picker to render.
  await waitFor(() => {
    expect(screen.getByTestId('linked-repos-picker')).toBeDefined();
  });
}

describe('ProjectLinkedRepos picker overflow (#27 fix)', () => {
  beforeEach(() => {
    mockState.currentCandidates = mockState.MANY_CANDIDATES;
  });

  afterEach(() => cleanup());

  it('shows the "show more" button when more than 12 candidates exist', async () => {
    wrap(<ProjectLinkedRepos projectId="proj-current" currentRepos={[]} onUpdate={() => {}} />);
    await openDraft();
    // Initially: only the first 12 candidates appear inside the picker.
    const picker = screen.getByTestId('linked-repos-picker');
    const scoped = within(picker);
    // Project00 .. Project11 visible
    expect(scoped.getByText(/Project00/)).toBeDefined();
    expect(scoped.getByText(/Project11/)).toBeDefined();
    // Project12 .. Project18 hidden until expanded
    expect(scoped.queryByText(/Project12/)).toBeNull();
    expect(scoped.queryByText(/Project18/)).toBeNull();
    // The overflow control is rendered as a real button (was the bug).
    const showMore = scoped.getByTestId('linked-repos-picker-show-more');
    expect(showMore.tagName.toLowerCase()).toBe('button');
    expect(showMore.textContent).toMatch(/7/); // 19 - 12 = 7 hidden
  });

  it('clicking the "show more" button reveals the remaining candidates', async () => {
    wrap(<ProjectLinkedRepos projectId="proj-current" currentRepos={[]} onUpdate={() => {}} />);
    await openDraft();
    fireEvent.click(screen.getByTestId('linked-repos-picker-show-more'));
    const picker = screen.getByTestId('linked-repos-picker');
    const scoped = within(picker);
    // The previously-hidden ones now appear.
    expect(scoped.getByText(/Project12/)).toBeDefined();
    expect(scoped.getByText(/Project18/)).toBeDefined();
    // The "show more" disappears, "show less" takes its place.
    expect(scoped.queryByTestId('linked-repos-picker-show-more')).toBeNull();
    expect(scoped.getByTestId('linked-repos-picker-show-less')).toBeDefined();
  });

  it('clicking the "show less" button re-collapses to 12 items', async () => {
    wrap(<ProjectLinkedRepos projectId="proj-current" currentRepos={[]} onUpdate={() => {}} />);
    await openDraft();
    fireEvent.click(screen.getByTestId('linked-repos-picker-show-more'));
    fireEvent.click(screen.getByTestId('linked-repos-picker-show-less'));
    const picker = screen.getByTestId('linked-repos-picker');
    const scoped = within(picker);
    expect(scoped.queryByText(/Project18/)).toBeNull();
    expect(scoped.getByTestId('linked-repos-picker-show-more')).toBeDefined();
  });

  it('no overflow button when 12 or fewer candidates exist', async () => {
    mockState.currentCandidates = mockState.FEW_CANDIDATES;
    wrap(<ProjectLinkedRepos projectId="proj-current" currentRepos={[]} onUpdate={() => {}} />);
    await openDraft();
    const picker = screen.getByTestId('linked-repos-picker');
    const scoped = within(picker);
    expect(scoped.queryByTestId('linked-repos-picker-show-more')).toBeNull();
    expect(scoped.queryByTestId('linked-repos-picker-show-less')).toBeNull();
  });
});
