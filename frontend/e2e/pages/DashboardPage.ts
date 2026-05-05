import type { Page, Locator } from '@playwright/test';

/**
 * Dashboard nav (top bar with Projets / Discussions / Plugins / Automatisation
 * / Config). Selectors use `data-tour-id="nav-<page>"` from `Dashboard.tsx`
 * — those IDs were added for the guided tour and double as stable test
 * hooks (survive label changes, i18n, locale).
 */
export class DashboardPage {
  constructor(private readonly page: Page) {}

  /** Open the app at root and wait for the nav to be ready. */
  async goto() {
    await this.page.goto('/');
    await this.navWorkflows.waitFor({ state: 'visible', timeout: 15_000 });
  }

  get navProjects(): Locator { return this.page.locator('[data-tour-id="nav-projects"]'); }
  get navDiscussions(): Locator { return this.page.locator('[data-tour-id="nav-discussions"]'); }
  get navMcps(): Locator { return this.page.locator('[data-tour-id="nav-mcps"]'); }
  get navWorkflows(): Locator { return this.page.locator('[data-tour-id="nav-workflows"]'); }
  get navSettings(): Locator { return this.page.locator('[data-tour-id="nav-settings"]'); }

  async clickWorkflows() { await this.navWorkflows.click(); }
  async clickSettings() { await this.navSettings.click(); }
  async clickProjects() { await this.navProjects.click(); }
}
