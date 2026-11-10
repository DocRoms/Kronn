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
  /** The one permanent activity bell (KT-1658); it replaced the audits and runs triggers. */
  get activityBell(): Locator { return this.page.getByTestId('activity-bell'); }
  get activityPanel(): Locator { return this.page.getByTestId('activity-panel'); }

  async clickWorkflows() { await this.openWorkflows(); }
  async clickSettings() { await this.navSettings.click(); }
  async clickProjects() { await this.navProjects.click(); }

  async openWorkflows() {
    await this.navWorkflows.click();
    const automationKinds = this.page.locator('[data-tour-id="automation-kinds"]');
    await automationKinds.waitFor({ state: 'visible', timeout: 10_000 });
  }

  /** Select one concrete discussion without assuming it appears only once.
   *  Smart shortcuts and the canonical project tree intentionally render the
   *  same discussion; the durable id is the stable identity. */
  async openDiscussion(discId: string) {
    await this.navDiscussions.click();
    await this.page.locator('.disc-sidebar').waitFor({ state: 'visible', timeout: 10_000 });
    await this.page
      .locator(`[data-tour-disc-id="${discId}"] .disc-item-open`)
      .first()
      .click();
  }
}
