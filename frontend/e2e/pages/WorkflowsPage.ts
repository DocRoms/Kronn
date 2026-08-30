import type { Page, Locator } from '@playwright/test';

/**
 * Automation page (= "Workflows" page in the codebase). Resource kinds are
 * exposed from the sidebar and creation goes through the shared
 * "Create or import" dialog opened by the green + button.
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

  // ─── Unified creation dialog ──────────────────────────────────
  /** Green + button in the collection sidebar header. */
  get creationMenuButton(): Locator {
    return this.page.locator('[data-tour-id="automation-actions"]');
  }

  /** Accessible chooser opened by the sidebar + button. */
  get creationDialog(): Locator {
    return this.page.getByRole('dialog', {
      name: /Créer ou importer|Create or import|Crear o importar|创建或导入/i,
    });
  }

  // ─── Creation choices ──────────────────────────────────────
  /** "Nouveau workflow" / "New workflow" choice in the creation dialog. */
  get newWorkflowButton(): Locator {
    return this.creationDialog.getByRole('button', {
      name: /Nouveau workflow|New workflow|Nuevo workflow/i,
    });
  }
  /** "Nouveau prompt" / "New prompt" choice in the creation dialog. */
  get newPromptButton(): Locator {
    return this.creationDialog.getByRole('button', {
      name: /Nouveau prompt|New prompt|Nuevo prompt/i,
    });
  }
  /** "Nouveau Quick API" choice, present when an API plugin is available. */
  get newQuickApiButton(): Locator {
    return this.creationDialog.getByRole('button', {
      name: /Nouveau Quick API|New Quick API|Nueva Quick API/i,
    });
  }
  /** Import choice, always present in the creation dialog. */
  get importButton(): Locator {
    return this.creationDialog.getByRole('button', {
      name: /Importer|Import|Importar/i,
    });
  }

  // ─── Actions ────────────────────────────────────────────────────────
  async clickQuickPromptsTab() { await this.tabQuickPrompts.click(); }
  async clickQuickApisTab() { await this.tabQuickApis.click(); }
  async clickWorkflowsTab() { await this.tabWorkflows.click(); }

  /** Open the shared automation creation chooser. */
  async openCreationDialog() {
    await this.creationMenuButton.click();
    await this.creationDialog.waitFor({ state: 'visible' });
  }

  /** Open the workflow creation wizard. */
  async openNewWorkflowWizard() {
    await this.openCreationDialog();
    await this.newWorkflowButton.click();
  }

  /** Open the Quick Prompt creation form. */
  async openNewQuickPromptForm() {
    await this.openCreationDialog();
    await this.newPromptButton.click();
  }
}
