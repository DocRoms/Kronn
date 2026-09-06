import type { Page } from '@playwright/test';

export class BootPage {
  constructor(readonly page: Page) {}

  get status() { return this.page.getByRole('status'); }
  get mark() { return this.status.locator('svg'); }

  async goto() {
    await this.page.goto('/', { waitUntil: 'domcontentloaded' });
    await this.mark.waitFor({ state: 'visible' });
  }
}
