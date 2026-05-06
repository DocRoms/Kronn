/**
 * Settings — "Mes contextes" (cross-project user context editor).
 *
 * Sprint 2 graph-memory feature : files in `~/.kronn/user-context/` are
 * auto-injected into every agent's prompt regardless of CLI or project.
 * This spec stubs the four CRUD endpoints and drives the inline editor
 * end-to-end : create → list → expand → edit → save → delete.
 *
 * The spec doesn't talk to the real disk — the backend handlers are
 * unit-tested in `backend/src/api/user_context.rs`. Here we lock in the
 * UI contract so a future refactor of `UserContextEditor.tsx` can't
 * silently break the operator-facing flow.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

interface FileEntry {
  name: string;
  size: number;
  content?: string;
}

/** Install in-memory CRUD stubs for /api/user-context. Using a closure
 *  store lets a single spec exercise the full create→list→update→delete
 *  cycle without re-routing per assertion. */
function installUserContextStubs(page: import('@playwright/test').Page, initial: FileEntry[]) {
  const store = new Map<string, FileEntry>(initial.map(f => [f.name, { ...f }]));

  page.route('**/api/user-context', route => {
    if (route.request().method() !== 'GET') return route.continue();
    const list = [...store.values()].map(({ name, size }) => ({ name, size }));
    list.sort((a, b) => a.name.localeCompare(b.name));
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: list, error: null }),
    });
  });

  page.route('**/api/user-context/*', async route => {
    const url = new URL(route.request().url());
    const name = decodeURIComponent(url.pathname.split('/').pop() ?? '');
    const method = route.request().method();
    if (method === 'GET') {
      const f = store.get(name);
      if (!f) return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: false, data: null, error: 'File not found' }) });
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: { ...f }, error: null }),
      });
    }
    if (method === 'PUT') {
      const body = JSON.parse(route.request().postData() ?? '{}');
      const content = body.content ?? '';
      store.set(name, { name, size: content.length, content });
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: { name, size: content.length, content }, error: null }),
      });
    }
    if (method === 'DELETE') {
      store.delete(name);
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: null, error: null }),
      });
    }
    return route.continue();
  });
}

test.describe('Settings — user context editor', () => {
  test('lists existing files and expands an editor with the file content', async ({ page }) => {
    installUserContextStubs(page, [
      { name: 'about-me.md', size: 22, content: '# About me\n\nFR speaker' },
      { name: 'conventions.md', size: 14, content: '# Conventions' },
    ]);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.locator('#settings-user-context');
    await section.scrollIntoViewIfNeeded();
    await expect(section).toBeVisible();

    await expect(section.getByText('about-me.md')).toBeVisible();
    await expect(section.getByText('conventions.md')).toBeVisible();

    // Expand the first file — its body should land in the textarea.
    await section.getByText('about-me.md').click();
    const textarea = section.locator('textarea').first();
    await expect(textarea).toHaveValue(/# About me/);
  });

  test('creates a new file (auto-appends .md) and surfaces it in the list', async ({ page }) => {
    installUserContextStubs(page, []);
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.locator('#settings-user-context');
    await section.scrollIntoViewIfNeeded();

    await section.locator('input.user-context-name-input').fill('writing-style');
    await section.getByRole('button', { name: /Ajouter/i }).click();

    // Refresh propagates new file into the list — created with .md appended.
    await expect(section.getByText('writing-style.md')).toBeVisible();
  });

  test('edits a file content and Save persists', async ({ page }) => {
    installUserContextStubs(page, [
      { name: 'about-me.md', size: 5, content: 'hello' },
    ]);
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.locator('#settings-user-context');
    await section.scrollIntoViewIfNeeded();

    await section.getByText('about-me.md').click();
    const textarea = section.locator('textarea').first();
    await expect(textarea).toHaveValue('hello');

    // Save is disabled when content === original (no-op detection).
    const saveBtn = section.getByRole('button', { name: /Enregistrer/i });
    await expect(saveBtn).toBeDisabled();

    await textarea.fill('hello, updated');
    await expect(saveBtn).toBeEnabled();
    await saveBtn.click();

    // After save, the file size in the list reflects the new content length.
    await expect(section.getByText('14 B')).toBeVisible();
  });

  test('deletes a file after confirm', async ({ page }) => {
    installUserContextStubs(page, [
      { name: 'about-me.md', size: 5, content: 'hello' },
      { name: 'keep-me.md', size: 4, content: 'keep' },
    ]);
    page.on('dialog', dialog => dialog.accept());

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.locator('#settings-user-context');
    await section.scrollIntoViewIfNeeded();

    const row = section.locator('li.user-context-row', { hasText: 'about-me.md' });
    await row.locator('button.user-context-delete').click();

    await expect(section.getByText('about-me.md')).toBeHidden();
    await expect(section.getByText('keep-me.md')).toBeVisible();
  });

  test('dismisses delete when confirm is cancelled', async ({ page }) => {
    installUserContextStubs(page, [
      { name: 'about-me.md', size: 5, content: 'hello' },
    ]);
    page.on('dialog', dialog => dialog.dismiss());

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = page.locator('#settings-user-context');
    await section.scrollIntoViewIfNeeded();

    const row = section.locator('li.user-context-row', { hasText: 'about-me.md' });
    await row.locator('button.user-context-delete').click();

    // File remains because the dialog was dismissed.
    await expect(section.getByText('about-me.md')).toBeVisible();
  });
});
