import { expect } from 'vitest';
import userEvent from '@testing-library/user-event';

/** Tabs forwards then backwards and asserts focus never leaves the dialog. */
export async function expectFocusKeptIn(dialog: HTMLElement, presses = 10): Promise<void> {
  expect(dialog.contains(document.activeElement)).toBe(true);
  for (let i = 0; i < presses; i++) {
    await userEvent.tab();
    expect(dialog.contains(document.activeElement)).toBe(true);
  }
  for (let i = 0; i < presses; i++) {
    await userEvent.tab({ shift: true });
    expect(dialog.contains(document.activeElement)).toBe(true);
  }
}
