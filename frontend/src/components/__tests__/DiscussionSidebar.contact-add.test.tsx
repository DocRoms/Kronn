// DiscussionSidebar — contact add double-click race regression.
//
// `handleContactAdd` calls the parent-supplied `onContactAdd` which POSTs
// to /api/contacts. Pre-fix two synchronous Enter presses on the input
// (or two clicks on the submit button) would both call the prop before
// React closed the form, creating duplicate contacts. The ref guard
// short-circuits the second call.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, act } from '@testing-library/react';
import { DiscussionSidebar } from '../DiscussionSidebar';

const noop = () => {};

const baseProps = {
  discussions: [],
  projects: [],
  activeId: null,
  sendingMap: {},
  lastSeenMsgCount: {},
  contacts: [],
  contactsOnline: {},
  wsConnected: true,
  isMobile: false,
  onSelect: noop,
  onArchive: noop,
  onUnarchive: noop,
  onDelete: noop,
  onTogglePin: noop,
  onNewDiscussion: noop,
  onClose: noop,
  onContactDelete: vi.fn().mockResolvedValue(undefined),
  toast: vi.fn(),
  t: (key: string) => key,
  collapsedGroups: new Set<string>(),
  onToggleGroup: noop,
};

describe('DiscussionSidebar — contact add ref guard', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('two synchronous Enter presses fire onContactAdd exactly once', async () => {
    let resolveAdd: (() => void) | undefined;
    const onContactAdd = vi.fn(() => new Promise<void>(r => { resolveAdd = r; }));
    render(<DiscussionSidebar {...baseProps} onContactAdd={onContactAdd} />);

    // Open the add-contact form by clicking the "+" toggle. The toggle
    // sets `showAddContact=true` which mounts the input + submit button.
    const toggleBtns = document.querySelectorAll('button[title*="contacts"], button[aria-label*="add" i]');
    // Fallback: just find the form by structure — there's a single
    // `disc-contact-add-input` after toggle.
    // The "Add contact" button uses `contacts.add` translation, which
    // our mock t() returns as the key. Search for it directly.
    const buttons = Array.from(document.querySelectorAll('button'));
    const addToggle = buttons.find(b => b.textContent?.includes('contacts.add')
      && b.parentElement?.className?.includes('contacts'));
    // Toggle button isn't trivial to locate — fall through to the addInitScript.
    if (toggleBtns.length === 0 && !addToggle) {
      // Force-open via querying the chevron — but in this minimal test
      // the toggle button is visible on hover only. Instead, look for
      // the button that opens the add-contact form: the one with title
      // "Add contact" or the icon-only one. The simplest reliable hook
      // is the data attribute. None exists, so we set the state via a
      // direct keyDown on a synthetic input rendered after toggle.
    }

    // Click any toggle that flips the form open. The "+" icon button
    // sits inside `.disc-contacts-section` near the contacts header.
    // We locate it by class.
    const plusToggle = document.querySelector('.disc-contacts-add-toggle, button[title*="add" i]');
    if (plusToggle) {
      act(() => { fireEvent.click(plusToggle); });
    } else {
      // Direct DOM fallback — the toggle is a button rendered inline.
      // If we can't find it, this test's premise is wrong; skip.
      expect.fail('Add-contact toggle button not found');
    }

    const input = document.querySelector('.disc-contact-add-input') as HTMLInputElement;
    expect(input).not.toBeNull();
    act(() => { fireEvent.change(input, { target: { value: 'TestContact' } }); });

    // Two fast Enter presses on the input.
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter' });
      fireEvent.keyDown(input, { key: 'Enter' });
    });

    // Without the ref guard the second Enter would fire a second
    // onContactAdd call. With the guard, only the first invocation wins.
    expect(onContactAdd).toHaveBeenCalledTimes(1);
    expect(onContactAdd).toHaveBeenCalledWith('TestContact');

    // Resolve the in-flight promise so the cleanup is clean.
    await act(async () => { resolveAdd?.(); });
  });

  it('two synchronous submit clicks fire onContactAdd exactly once', async () => {
    let resolveAdd: (() => void) | undefined;
    const onContactAdd = vi.fn(() => new Promise<void>(r => { resolveAdd = r; }));
    render(<DiscussionSidebar {...baseProps} onContactAdd={onContactAdd} />);

    const plusToggle = document.querySelector('.disc-contacts-add-toggle, button[title*="add" i]');
    expect(plusToggle).not.toBeNull();
    act(() => { fireEvent.click(plusToggle!); });

    const input = document.querySelector('.disc-contact-add-input') as HTMLInputElement;
    act(() => { fireEvent.change(input, { target: { value: 'PeerAlpha' } }); });

    const submit = document.querySelector('.disc-contact-add-submit') as HTMLButtonElement;
    expect(submit).not.toBeNull();

    await act(async () => {
      fireEvent.click(submit);
      fireEvent.click(submit);
    });

    expect(onContactAdd).toHaveBeenCalledTimes(1);
    await act(async () => { resolveAdd?.(); });
  });
});
