import type { Page } from '@playwright/test';

/** The human form, independent from the ordinary discussion composer. */
export class ImportantMessagePage {
  constructor(private readonly page: Page) {}
  get region() { return this.page.getByRole('region', { name: 'Message important', exact: true }); }
  get trigger() { return this.region.getByRole('button', { name: 'Message important', exact: true }); }
  get content() { return this.region.getByLabel('Message', { exact: true }); }
  get credential() { return this.region.getByLabel('Clé de publication'); }
  get category() { return this.region.getByLabel('Catégorie'); }
  get task() { return this.region.getByLabel('Tâche liée (facultatif)'); }
  get publish() { return this.region.getByRole('button', { name: 'Publier le message important' }); }
  get ordinaryComposer() { return this.page.locator('.disc-composer-textarea'); }
  async open() {
    if (await this.trigger.getAttribute('aria-expanded') !== 'true') await this.trigger.click();
  }
  async authorize(grant: string) { await this.open(); await this.credential.fill(grant); }
  async fill(content: string, grant: string, category = 'information') {
    await this.authorize(grant);
    await this.content.fill(content);
    await this.category.selectOption(category);
  }
}
