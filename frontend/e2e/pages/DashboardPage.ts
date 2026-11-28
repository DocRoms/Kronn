import type { Page, Locator } from '@playwright/test';
import { SETTINGS_SUB_PAGES, settingsSectionOf, type SettingsSubPage } from '../../src/lib/routes';

export { SETTINGS_SUB_PAGES, type SettingsSubPage };

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

  /** KT-1923 — the visible Settings sub-page (the others stay drawn, hidden). */
  get settingsSubPage(): Locator { return this.page.locator('.set-subpage:not([hidden])'); }

  /**
   * KT-1923 — Settings is five sub-pages (`/config/you` … `/config/system`)
   * and its tab opens the first. Opens Settings from its tab, then the
   * sub-page from the list on the left, and waits for its heading.
   */
  async openSettings(subPage: SettingsSubPage = 'you') {
    await this.navSettings.click();
    await this.selectSettingsSubPage(subPage);
  }

  /** KT-1923 — Settings already open: shows one of its sub-pages from the list. */
  async selectSettingsSubPage(subPage: SettingsSubPage) {
    const entry = this.page.getByTestId(`settings-subpage-${subPage}`);
    await entry.waitFor({ state: 'visible', timeout: 15_000 });
    if ((await entry.getAttribute('aria-current')) !== 'page') await entry.click();
    await this.page.locator(`#settings-subpage-${subPage}-title`).waitFor({ state: 'visible', timeout: 15_000 });
  }

  /**
   * KT-1923 — opens the sub-page that holds a section (or an anchor inside
   * one: `settings-skills`, `settings-debug-logs`), opens every fold of that
   * sub-page, and brings the anchor into view.
   */
  async openSettingsSection(anchor: string): Promise<Locator> {
    const section = settingsSectionOf(anchor);
    if (!section) throw new Error(`No Settings section holds #${anchor}`);
    await this.openSettings(section.page);
    await this.openSettingsFolds();
    const target = this.page.locator(`[id="${anchor}"]`);
    await target.scrollIntoViewIfNeeded();
    return target;
  }

  /** KT-1923 — opens every fold of the visible sub-page (catalogue, usage,
   *  API call log, debug log viewer); a fold can hold another. */
  async openSettingsFolds() {
    const closed = this.settingsSubPage.locator('[data-testid^="fold-"][aria-expanded="false"]');
    for (let guard = 0; guard < 10 && (await closed.count()) > 0; guard += 1) {
      await closed.first().click();
    }
  }
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
