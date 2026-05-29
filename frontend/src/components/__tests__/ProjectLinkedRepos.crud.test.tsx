import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { LinkedRepo } from '../../types/generated';

// 0.8.6 — CRUD coverage for ProjectLinkedRepos. The picker-overflow
// behaviour is pinned in ProjectLinkedRepos.picker.test.tsx; this file
// targets the *uncovered* paths: rendering existing repos, the add form
// (validation + setLinkedRepos payload + onUpdate), removal (confirm +
// cancel), editing the kind dropdown, the save-error catch branch, the
// max-entries guard, and the empty state.
//
// The I18nProvider defaults to locale 'fr', so visible strings asserted
// below are the French i18n values for the `linkedRepos.*` keys.

const mockState = vi.hoisted(() => ({
  // No candidates by default — keeps the picker out of the way so the
  // manual add-form is the thing under test. Picker ranking has its own
  // suite.
  candidates: [] as Array<{ id: string; name: string; path: string; proximity_hint: string }>,
}));

const { setLinkedRepos, linkedReposCandidates } = vi.hoisted(() => ({
  setLinkedRepos: vi.fn(),
  linkedReposCandidates: vi.fn(),
}));

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock({
    projects: {
      setLinkedRepos,
      linkedReposCandidates,
    },
  });
});

import { ProjectLinkedRepos } from '../ProjectLinkedRepos';

type Props = React.ComponentProps<typeof ProjectLinkedRepos>;

function wrap(overrides: Partial<Props> = {}) {
  const onUpdate = vi.fn();
  const props: Props = {
    projectId: 'proj-current',
    currentRepos: [],
    onUpdate,
    ...overrides,
  };
  const utils = render(
    <I18nProvider>
      <ProjectLinkedRepos {...props} />
    </I18nProvider>,
  );
  return { onUpdate, ...utils };
}

const REPO_API: LinkedRepo = {
  id: 'r-api',
  name: 'backend-api',
  kind: 'api',
  location: 'https://github.com/org/backend-api',
  description: 'GraphQL schema',
};
const REPO_IAC: LinkedRepo = {
  id: 'r-iac',
  name: 'infra',
  kind: 'iac',
  location: '/home/user/Repos/infra',
};

async function openDraft() {
  const addBtn = await screen.findByRole('button', { name: /Ajouter un dépôt lié/i });
  fireEvent.click(addBtn);
  // candidates fetch resolves on draft entry; wait a tick for state.
  await waitFor(() => expect(linkedReposCandidates).toHaveBeenCalled());
}

beforeEach(() => {
  setLinkedRepos.mockReset().mockResolvedValue(undefined);
  linkedReposCandidates.mockReset().mockImplementation(() => Promise.resolve(mockState.candidates));
  mockState.candidates = [];
});

afterEach(() => {
  vi.clearAllMocks();
  cleanup();
});

describe('ProjectLinkedRepos — rendering', () => {
  it('shows the empty-state hint when there are no repos', () => {
    wrap({ currentRepos: [] });
    expect(screen.getByText(/Aucun dépôt lié/i)).toBeDefined();
  });

  it('renders existing repos with name, kind and description', () => {
    wrap({ currentRepos: [REPO_API, REPO_IAC] });
    expect(screen.getByText('backend-api')).toBeDefined();
    expect(screen.getByText('infra')).toBeDefined();
    expect(screen.getByText('GraphQL schema')).toBeDefined();
    // Empty-state hint must be gone when repos exist.
    expect(screen.queryByText(/Aucun dépôt lié/i)).toBeNull();
  });

  it('renders a URL location as an anchor and a filesystem path as code', () => {
    const { container } = wrap({ currentRepos: [REPO_API, REPO_IAC] });
    const link = screen.getByRole('link', { name: /backend-api/i });
    expect(link.getAttribute('href')).toBe('https://github.com/org/backend-api');
    expect(link.getAttribute('target')).toBe('_blank');
    // Filesystem path renders inside a <code>, not a link.
    const code = container.querySelector('code');
    expect(code?.textContent).toContain('/home/user/Repos/infra');
  });
});

describe('ProjectLinkedRepos — add flow', () => {
  it('blocks the save when name or location is empty (no API call)', async () => {
    wrap({ currentRepos: [] });
    await openDraft();
    // Click "Ajouter" (the submit button inside the draft form) with empty fields.
    const submit = screen.getByRole('button', { name: /^Ajouter$/i });
    fireEvent.click(submit);
    await waitFor(() => expect(screen.getByText(/obligatoires/i)).toBeDefined());
    expect(setLinkedRepos).not.toHaveBeenCalled();
  });

  it('appends a new repo and POSTs the full list with a trimmed payload', async () => {
    const { onUpdate } = wrap({ currentRepos: [REPO_API] });
    await openDraft();

    const nameInput = screen.getByPlaceholderText(/^Nom/i);
    const locationInput = screen.getByPlaceholderText(/Chemin ou URL/i);
    const descInput = screen.getByPlaceholderText(/À quoi sert/i);
    fireEvent.change(nameInput, { target: { value: '  shared-types  ' } });
    fireEvent.change(locationInput, { target: { value: '  /home/user/Repos/types  ' } });
    fireEvent.change(descInput, { target: { value: '  shared TS types  ' } });

    fireEvent.click(screen.getByRole('button', { name: /^Ajouter$/i }));

    await waitFor(() => expect(setLinkedRepos).toHaveBeenCalledTimes(1));
    const [pid, payload] = setLinkedRepos.mock.calls[0] as [string, LinkedRepo[]];
    expect(pid).toBe('proj-current');
    expect(payload).toHaveLength(2);
    // Pre-existing repo is preserved at the head.
    expect(payload[0]).toMatchObject({ id: 'r-api', name: 'backend-api' });
    // New repo: trimmed name/location/description, default kind 'api', fresh id.
    expect(payload[1]).toMatchObject({
      name: 'shared-types',
      location: '/home/user/Repos/types',
      description: 'shared TS types',
      kind: 'api',
    });
    expect(payload[1].id).toBeTruthy();
    await waitFor(() => expect(onUpdate).toHaveBeenCalledTimes(1));
  });

  it('uses the kind selected in the dropdown for the new repo', async () => {
    wrap({ currentRepos: [] });
    await openDraft();

    // Open the kind dropdown and pick "IaC". The trigger button already
    // shows the current label, so scope the option lookup to the open
    // listbox to avoid matching the trigger's own text.
    const trigger = screen.getByTestId('linked-repos-kind-picker');
    fireEvent.click(trigger);
    const listbox = await screen.findByRole('listbox');
    fireEvent.click(within(listbox).getByText(/IaC/i));

    fireEvent.change(screen.getByPlaceholderText(/^Nom/i), { target: { value: 'infra' } });
    fireEvent.change(screen.getByPlaceholderText(/Chemin ou URL/i), { target: { value: '/repos/infra' } });
    fireEvent.click(screen.getByRole('button', { name: /^Ajouter$/i }));

    await waitFor(() => expect(setLinkedRepos).toHaveBeenCalledTimes(1));
    const payload = setLinkedRepos.mock.calls[0][1] as LinkedRepo[];
    expect(payload[0].kind).toBe('iac');
  });

  it('cancel closes the draft form without calling the API', async () => {
    wrap({ currentRepos: [] });
    await openDraft();
    expect(screen.getByPlaceholderText(/^Nom/i)).toBeDefined();
    fireEvent.click(screen.getByRole('button', { name: /Annuler/i }));
    await waitFor(() => expect(screen.queryByPlaceholderText(/^Nom/i)).toBeNull());
    expect(setLinkedRepos).not.toHaveBeenCalled();
  });

  it('surfaces a save-error message when setLinkedRepos rejects', async () => {
    setLinkedRepos.mockRejectedValueOnce(new Error('network down'));
    wrap({ currentRepos: [] });
    await openDraft();
    fireEvent.change(screen.getByPlaceholderText(/^Nom/i), { target: { value: 'x' } });
    fireEvent.change(screen.getByPlaceholderText(/Chemin ou URL/i), { target: { value: '/x' } });
    fireEvent.click(screen.getByRole('button', { name: /^Ajouter$/i }));
    await waitFor(() => expect(screen.getByText(/network down/i)).toBeDefined());
  });
});

describe('ProjectLinkedRepos — remove flow', () => {
  it('removes a repo after confirm and POSTs the shortened list', async () => {
    // happy-dom doesn't define window.confirm, so stub it rather than spy.
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    const { onUpdate } = wrap({ currentRepos: [REPO_API, REPO_IAC] });

    const removeBtns = screen.getAllByRole('button', { name: /Retirer/i });
    fireEvent.click(removeBtns[0]);

    await waitFor(() => expect(setLinkedRepos).toHaveBeenCalledTimes(1));
    const payload = setLinkedRepos.mock.calls[0][1] as LinkedRepo[];
    expect(payload).toHaveLength(1);
    expect(payload[0].id).toBe('r-iac');
    await waitFor(() => expect(onUpdate).toHaveBeenCalled());
    vi.unstubAllGlobals();
  });

  it('does nothing when the remove confirm is dismissed', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(false));
    wrap({ currentRepos: [REPO_API] });
    fireEvent.click(screen.getByRole('button', { name: /Retirer/i }));
    expect(setLinkedRepos).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});

describe('ProjectLinkedRepos — max entries guard', () => {
  it('disables the add button at 20 linked repos', () => {
    const many: LinkedRepo[] = Array.from({ length: 20 }, (_, i) => ({
      id: `r-${i}`,
      name: `repo-${i}`,
      kind: 'other',
      location: `/repos/r-${i}`,
    }));
    wrap({ currentRepos: many });
    const addBtn = screen.getByRole('button', { name: /Ajouter un dépôt lié/i });
    expect((addBtn as HTMLButtonElement).disabled).toBe(true);
  });

  it('keeps the add button enabled below the limit', () => {
    wrap({ currentRepos: [REPO_API] });
    const addBtn = screen.getByRole('button', { name: /Ajouter un dépôt lié/i });
    expect((addBtn as HTMLButtonElement).disabled).toBe(false);
  });
});

describe('ProjectLinkedRepos — picker prefill', () => {
  it('clicking a candidate prefills name + location into the draft', async () => {
    mockState.candidates = [
      { id: 'c1', name: 'companion-lib', path: '/repos/companion-lib', proximity_hint: 'same-parent' },
    ];
    wrap({ currentRepos: [] });
    await openDraft();
    const picker = await screen.findByTestId('linked-repos-picker');
    fireEvent.click(within(picker).getByRole('button', { name: /companion-lib/i }));

    expect((screen.getByPlaceholderText(/^Nom/i) as HTMLInputElement).value).toBe('companion-lib');
    expect((screen.getByPlaceholderText(/Chemin ou URL/i) as HTMLInputElement).value).toBe('/repos/companion-lib');
  });
});
