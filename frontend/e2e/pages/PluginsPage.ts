import type { Locator, Page } from '@playwright/test';

export class PluginsPage {
  constructor(private readonly page: Page) {}

  card(label: string): Locator {
    return this.page.getByRole('button', {
      name: `${label} — Voir les détails`,
      exact: true,
    });
  }

  get panel(): Locator {
    return this.page.locator('[data-testid="mcp-plugin-panel"]');
  }

  get probeButton(): Locator {
    return this.panel.getByTestId('mcp-probe-button');
  }

  get probeStatus(): Locator {
    return this.panel.getByTestId('mcp-probe-status');
  }

  get preferredInterface(): Locator {
    return this.panel.getByTestId('mcp-preferred-interface');
  }

  async open(label: string) {
    await this.card(label).click();
    await this.panel.waitFor({ state: 'visible' });
  }
}
