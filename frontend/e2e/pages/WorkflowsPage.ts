import type { Page, Locator } from '@playwright/test';

/**
 * Automation page (= "Workflows" page in the codebase). Three sub-tabs:
 * Workflows / Quick Prompts / Quick APIs. Each tab has its own header
 * actions (create button + import button when applicable).
 */
export class WorkflowsPage {
  constructor(private readonly page: Page) {}

  // ─── Sub-tab buttons ────────────────────────────────────────────────
  /** "Workflows" sub-tab button. */
  get tabWorkflows(): Locator {
    return this.page.getByRole('button', { name: /^Workflows\b/i }).first();
  }
  /** "Quick Prompts" sub-tab button. */
  get tabQuickPrompts(): Locator {
    return this.page.getByRole('button', { name: /Quick Prompts/i });
  }
  /** "Quick APIs" sub-tab button. */
  get tabQuickApis(): Locator {
    return this.page.getByRole('button', { name: /Quick APIs/i });
  }

  // ─── Header (the flex-between block at the top of the page) ─────────
  /** Top-of-page header that holds the create + import + "create with AI" buttons.
   *  Used by regression specs to assert what action buttons are exposed
   *  on each sub-tab. */
  get header(): Locator {
    return this.page.locator('div.flex-between').first();
  }

  // ─── Header action buttons (sub-tab-dependent) ──────────────────────
  /** "Nouveau workflow" / "New workflow" — only visible when on Workflows tab. */
  get newWorkflowButton(): Locator {
    return this.header.getByRole('button', { name: /Nouveau workflow|New workflow/i });
  }
  /** "Nouveau prompt" — only visible when on Quick Prompts tab. */
  get newPromptButton(): Locator {
    return this.header.getByRole('button', { name: /Nouveau prompt|New prompt/i });
  }
  /** "Nouveau Quick API" — only visible when on Quick APIs tab AND a plugin is wired. */
  get newQuickApiButton(): Locator {
    return this.header.getByRole('button', { name: /Nouveau Quick API|New Quick API/i });
  }

  // ─── Actions ────────────────────────────────────────────────────────
  async clickQuickPromptsTab() { await this.tabQuickPrompts.click(); }
  async clickQuickApisTab() { await this.tabQuickApis.click(); }
  async clickWorkflowsTab() { await this.tabWorkflows.click(); }

  /** Open the workflow creation wizard. */
  async openNewWorkflowWizard() { await this.newWorkflowButton.click(); }
}
