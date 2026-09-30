import type { Page, Locator } from '@playwright/test';

/** The automation types of the sidebar's type chip, as `data-kind-option` and
 *  `data-value` spell them (`AUTOMATION_KIND_FILTERS` in the app). `all` is
 *  the chip's resting value. */
export type AutomationKind = 'workflows' | 'quickPrompts' | 'quickApis' | 'quickExecs' | 'skills';
export type AutomationKindFilter = AutomationKind | 'all';

/**
 * Automation page (= "Workflows" page in the codebase). The resource types are
 * picked on the type chip of the sidebar (`AutomationSidebarControls`, "All"
 * until one is chosen) and creation goes through the shared "Create or
 * import" dialog opened by the green + button.
 */
export class WorkflowsPage {
  constructor(private readonly page: Page) {}

  // ─── Type chip ──────────────────────────────────────────────────────
  /** Type chip under the sidebar search. `data-value` holds the chosen type
   *  (`all` by default) and `data-active` is `true` once one is chosen. */
  get kindChip(): Locator {
    return this.page.locator('[data-tour-id="automation-filter-type"]');
  }
  /** Listbox the type chip opens (one option per type, plus "All"). */
  get kindMenu(): Locator {
    return this.page.getByRole('listbox', {
      name: /Filtre par type d.automatisation|Automation type filter|Filtro por tipo de automatización|自动化类型筛选/i,
    });
  }
  /** One option of the type listbox, addressed by type rather than by its
   *  translated, counted label. `aria-selected` tells whether it is chosen. */
  kindOption(kind: AutomationKindFilter): Locator {
    return this.kindMenu.locator(`[data-kind-option="${kind}"]`);
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
  /** Open the type listbox (no-op when it already is). */
  async openKindMenu() {
    if (!(await this.kindMenu.isVisible())) await this.kindChip.click();
    await this.kindMenu.waitFor({ state: 'visible' });
  }

  /** Choose a type on the chip: the sidebar then lists that type only. */
  async selectKind(kind: AutomationKindFilter) {
    await this.openKindMenu();
    await this.kindOption(kind).click();
    await this.kindMenu.waitFor({ state: 'hidden' });
  }

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
