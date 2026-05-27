/**
 * 0.8.7 — P1-7b of the QA roadmap.
 *
 * ProfilesSection CRUDs agent personas. Pre-test : zero coverage. Risk
 * vector flagged by the audit : **delete cascade** — an active discussion
 * still references a deleted profile. These tests pin :
 *   - mount loads the list
 *   - delete button is HIDDEN for builtin profiles (regression : an
 *     accidental delete of a builtin breaks every project that defaults
 *     to that profile)
 *   - delete fires `confirm()` + `profiles.delete()` + toast on success
 *   - delete cancelled at `confirm()` → no API call
 *   - inline persona-name edit on blur calls `updatePersonaName`
 *   - create form toggle + validation (disabled while name+persona empty)
 *   - listen to `kronn:profiles-changed` event for refetch (secret-code
 *     unlock case)
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor } from '@testing-library/react';

const { profiles } = vi.hoisted(() => ({
  profiles: {
    list: vi.fn(),
    create: vi.fn(),
    delete: vi.fn(),
    update: vi.fn(),
    updatePersonaName: vi.fn(),
  },
}));

vi.mock('../../../lib/api', () => ({ profiles }));

import { ProfilesSection } from '../ProfilesSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

const SAMPLE = [
  {
    id: 'p-builtin', name: 'Architect', persona_name: 'Leo',
    role: 'Software Architect', avatar: '🧱', color: '#c8a0ff',
    category: 'Technical', persona_prompt: 'You are…',
    is_builtin: true, token_estimate: 120, default_engine: null,
  },
  {
    id: 'p-custom-1', name: 'QA Lead', persona_name: 'Mia',
    role: 'QA Engineer', avatar: '🛡️', color: '#88ddaa',
    category: 'Technical', persona_prompt: 'You audit…',
    is_builtin: false, token_estimate: 90, default_engine: null,
  },
];

beforeEach(() => {
  profiles.list.mockResolvedValue(SAMPLE);
  profiles.delete.mockResolvedValue(undefined);
  profiles.create.mockImplementation(async input => ({
    id: 'p-new', ...input, is_builtin: false, token_estimate: 50,
    default_engine: null,
  }));
  profiles.updatePersonaName.mockImplementation(async (id, persona_name) => {
    const found = SAMPLE.find(p => p.id === id)!;
    return { ...found, persona_name };
  });
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });

async function mountProfiles(toast = vi.fn()) {
  let r: ReturnType<typeof render>;
  await act(async () => {
    r = render(<ProfilesSection toast={toast as never} t={t} />);
  });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return r!;
}

describe('ProfilesSection — mount', () => {
  it('lists every profile returned by the API', async () => {
    await mountProfiles();
    expect(screen.getByText('Architect')).toBeTruthy();
    expect(screen.getByText('Leo')).toBeTruthy();
    expect(screen.getByText('QA Lead')).toBeTruthy();
    expect(screen.getByText('Mia')).toBeTruthy();
  });

  it('survives a failed list fetch without crashing the card', async () => {
    profiles.list.mockRejectedValue(new Error('500'));
    await mountProfiles();
    // Just the "create custom" CTA visible, no rows, no throw.
    expect(screen.getByText('profiles.createCustom')).toBeTruthy();
  });

  it('refetches when the kronn:profiles-changed event fires', async () => {
    await mountProfiles();
    expect(profiles.list).toHaveBeenCalledTimes(1);
    await act(async () => {
      window.dispatchEvent(new Event('kronn:profiles-changed'));
    });
    expect(profiles.list).toHaveBeenCalledTimes(2);
  });
});

describe('ProfilesSection — delete (cascade-safe)', () => {
  it('delete button is HIDDEN for builtin profiles', async () => {
    await mountProfiles();
    // The Trash2 button has aria-label="common.delete" via the identity
    // translator. Builtin profile (id p-builtin) must NOT expose one.
    const deletes = screen.getAllByLabelText('common.delete');
    // Only the ONE custom profile should be deletable.
    expect(deletes).toHaveLength(1);
  });

  it('clicking delete fires confirm() then profiles.delete + toast', async () => {
    // JSDOM doesn't ship a `window.confirm` ; assign one directly
    // (cleared in afterEach via the cleanup of the global stub).
    const toast = vi.fn();
    const confirmFn = vi.fn().mockReturnValue(true);
    window.confirm = confirmFn as never;
    await mountProfiles(toast);
    const delBtn = screen.getByLabelText('common.delete');
    await act(async () => { fireEvent.click(delBtn); });
    expect(confirmFn).toHaveBeenCalledWith('profiles.deleteConfirm');
    await waitFor(() => expect(profiles.delete).toHaveBeenCalledWith('p-custom-1'));
    expect(toast).toHaveBeenCalledWith('common.delete', 'success');
  });

  it('cancelling the confirm() dialog skips the API call', async () => {
    const toast = vi.fn();
    const confirmFn = vi.fn().mockReturnValue(false);
    window.confirm = confirmFn as never;
    await mountProfiles(toast);
    fireEvent.click(screen.getByLabelText('common.delete'));
    expect(confirmFn).toHaveBeenCalled();
    expect(profiles.delete).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
  });

  it('removes the deleted profile from the rendered list', async () => {
    window.confirm = vi.fn().mockReturnValue(true) as never;
    await mountProfiles();
    expect(screen.getByText('QA Lead')).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByLabelText('common.delete')); });
    await waitFor(() => expect(screen.queryByText('QA Lead')).toBeNull());
  });
});

describe('ProfilesSection — inline persona-name edit', () => {
  it('clicking the persona name opens an input and saves onBlur', async () => {
    await mountProfiles();
    // Mia is the editable persona of the custom profile (the builtin
    // also has a persona name but the API call is the same path).
    fireEvent.click(screen.getByText('Mia'));
    const editor = document.querySelector('input.set-persona-input') as HTMLInputElement | null;
    expect(editor).not.toBeNull();
    fireEvent.change(editor!, { target: { value: 'Maya' } });
    await act(async () => { fireEvent.blur(editor!); });
    await waitFor(() =>
      expect(profiles.updatePersonaName).toHaveBeenCalledWith('p-custom-1', 'Maya')
    );
  });

  it('onBlur does NOT fire updatePersonaName when the name is unchanged', async () => {
    await mountProfiles();
    fireEvent.click(screen.getByText('Mia'));
    const editor = document.querySelector('input.set-persona-input') as HTMLInputElement | null;
    expect(editor).not.toBeNull();
    // Don't change the value — just blur.
    await act(async () => { fireEvent.blur(editor!); });
    expect(profiles.updatePersonaName).not.toHaveBeenCalled();
  });

  it('Escape exits edit mode without saving', async () => {
    await mountProfiles();
    fireEvent.click(screen.getByText('Mia'));
    const editor = document.querySelector('input.set-persona-input') as HTMLInputElement | null;
    expect(editor).not.toBeNull();
    fireEvent.change(editor!, { target: { value: 'WontSave' } });
    fireEvent.keyDown(editor!, { key: 'Escape' });
    expect(profiles.updatePersonaName).not.toHaveBeenCalled();
  });
});

describe('ProfilesSection — create form', () => {
  it('CTA toggles the form open ; submit button stays disabled until required fields are filled', async () => {
    await mountProfiles();
    fireEvent.click(screen.getByText('profiles.createCustom'));
    // Two "createCustom" labels now exist : the CTA-now-hidden one + the
    // submit inside the form. Find the submit by its disabled state.
    const buttons = screen.getAllByText('profiles.createCustom')
      .map(el => el.closest('button'))
      .filter((b): b is HTMLButtonElement => !!b);
    const submit = buttons.find(b => b.hasAttribute('disabled'))!;
    expect(submit).toBeTruthy();
    expect(submit.hasAttribute('disabled')).toBe(true);

    fireEvent.change(screen.getByPlaceholderText('Architect, QA Lead...'), { target: { value: 'New Profile' } });
    fireEvent.change(screen.getByPlaceholderText('You are an expert in... Always prioritize...'),
      { target: { value: 'You are a focused tester.' } });

    // Re-query : the submit button is now enabled.
    const updated = screen.getAllByText('profiles.createCustom')
      .map(el => el.closest('button'))
      .filter((b): b is HTMLButtonElement => !!b)
      .find(b => !b.hasAttribute('disabled'));
    expect(updated).toBeTruthy();
  });

  it('submitting a valid form posts to profiles.create + adds the result to the list', async () => {
    const toast = vi.fn();
    await mountProfiles(toast);
    fireEvent.click(screen.getByText('profiles.createCustom'));
    fireEvent.change(screen.getByPlaceholderText('Architect, QA Lead...'), { target: { value: 'PeerAlpha' } });
    fireEvent.change(screen.getByPlaceholderText('Leo, Mia, Sam...'), { target: { value: 'Alpha' } });
    fireEvent.change(screen.getByPlaceholderText('You are an expert in... Always prioritize...'),
      { target: { value: 'Sharp reviewer.' } });

    // Click the still-enabled "createCustom" submit (the form one).
    const enabledSubmit = screen.getAllByText('profiles.createCustom')
      .map(el => el.closest('button'))
      .filter((b): b is HTMLButtonElement => !!b)
      .find(b => !b.hasAttribute('disabled'))!;
    await act(async () => { fireEvent.click(enabledSubmit); });
    await waitFor(() => expect(profiles.create).toHaveBeenCalledWith(expect.objectContaining({
      name: 'PeerAlpha', persona_name: 'Alpha', persona_prompt: 'Sharp reviewer.',
    })));
    expect(toast).toHaveBeenCalledWith('profiles.createCustom', 'success');
  });
});
