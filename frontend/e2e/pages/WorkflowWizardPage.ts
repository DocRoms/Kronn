import type { Page, Locator } from '@playwright/test';

/**
 * Workflow creation wizard. Opens after clicking "Nouveau workflow" on the
 * Workflows page.
 *
 * **0.8.5 layout change.** Pre-0.8.5 the wizard had three separate
 * "start from something" surfaces: STARTER_TEMPLATES buttons at the
 * top of step 0, project-suggestions toggle at the top of step 0,
 * and v0.7 preset cards buried in advanced step 2. The page-object
 * exposed `gotoStepsPage()` to walk to step 2 where the preset cards
 * lived. Since 0.8.5 the three sources are unified in a single
 * `WorkflowQuickStartPicker` on step 0 (Infos), right after the
 * name + project inputs. The picker is **collapsed by default** and
 * **disabled until the workflow name is filled**. Tests should
 * therefore use `openQuickStartPicker(name)` (which fills the name
 * + clicks the toggle) and `applyQuickStart(name, titleRe)` (which
 * also clicks the Apply button on the matching row + lets the wizard
 * jump to advanced step 2).
 */
export class WorkflowWizardPage {
  constructor(private readonly page: Page) {}

  // ─── Mode toggle (advanced is the default in 0.6+) ──────────────────
  get advancedModeButton(): Locator {
    return this.page.getByRole('button', { name: /Avancé|Advanced/i }).first();
  }

  // ─── QuickStart picker (0.8.5+, unified entry point on step 0) ──────
  /**
   * The collapsed toggle chip shown when the picker is closed. Matches
   * the i18n key `wiz.quickstart.toggle` (interpolated with the entry
   * count) across FR / EN / ES.
   */
  get quickStartToggle(): Locator {
    return this.page.getByRole('button', {
      name: /modèles disponibles|ready-made templates available|plantillas disponibles/i,
    });
  }

  /** Search input inside the expanded picker panel. */
  get quickStartSearchInput(): Locator {
    return this.page.getByPlaceholder(/Rechercher un modèle|Search templates|Buscar plantilla/i);
  }

  /**
   * Pick the `<li class="wf-quickstart-row">` whose title `<span>`
   * matches `titleRe`. Used as the locator surface for `toBeVisible()`
   * + `toContainText()` assertions across the wizard-presets specs.
   */
  quickStartRow(titleRe: RegExp): Locator {
    return this.page.locator('li.wf-quickstart-row').filter({
      has: this.page.locator('span.wf-quickstart-row-title', { hasText: titleRe }),
    });
  }

  /**
   * The "Use this template" button inside the matching row. Returns the
   * button, not the row, so `.click()` actually triggers `onApply` on
   * the picker.
   */
  quickStartApplyButton(titleRe: RegExp): Locator {
    return this.quickStartRow(titleRe).getByRole('button', {
      name: /Utiliser ce modèle|Use this template|Usar esta plantilla/i,
    });
  }

  // ─── Preset-card backward-compat (0.8.5+) ───────────────────────────
  // Return the picker rows that correspond to the 4 frequently-referenced
  // v0.7 presets. Tests use these for visibility assertions; for clicks,
  // use `quickStartApplyButton(titleRe)` since the row itself is a `<li>`
  // (the apply button is a sibling of the title).
  get presetAutoDev(): Locator { return this.quickStartRow(/Auto-Dev avec tests|Auto-Dev with tests/i); }
  // 0.8.3 emoji anchor (preserved across the 0.8.5 picker refactor) — the
  // generic `/Ticket Autopilot/i` substring also matches `🎯 Big-ticket
  // AutoPilot — feasibility-gated`, so we anchor on the unique `🎫`
  // prefix from the preset's `icon` field. See
  // `frontend/src/lib/workflow-templates/v07-presets.ts::TICKET_TO_PR.icon`.
  get presetTicketToPr(): Locator { return this.quickStartRow(/🎫\s*Ticket Autopilot/i); }
  get presetFeasibilityAutopilot(): Locator { return this.quickStartRow(/🎯\s*Big-ticket AutoPilot/i); }
  get presetDailyHostAudit(): Locator { return this.quickStartRow(/Audit quotidien|Daily audit/i); }

  // ─── Step type buttons (in-step editor) ─────────────────────────────
  /** Step type button inside a step card. Use `data-type` for stable
   *  matching (cf. WorkflowWizard.tsx). */
  stepTypeButton(stepIdx: number, type: 'agent' | 'api' | 'batch-qp' | 'batch-api' | 'notify' | 'gate' | 'exec' | 'json-data'): Locator {
    return this.page.locator(`.wf-step:nth-of-type(${stepIdx + 1}) [data-type="${type}"]`);
  }

  // ─── Wizard global actions ──────────────────────────────────────────
  get nextButton(): Locator { return this.page.getByRole('button', { name: /Suivant|Next/i }); }
  get createButton(): Locator { return this.page.getByRole('button', { name: /Créer$|Create$/i }); }
  get cancelButton(): Locator { return this.page.getByRole('button', { name: /Annuler|Cancel/i }); }

  // ─── Save error banner (0.7+ — surfaced when handleSave throws) ─────
  // The banner reuses `.wf-restricted-warning` (also used for step
  // constraint warnings deep in the editor) — but on the Résumé step
  // those are out of the DOM, so a plain class match resolves to the
  // save banner. We still filter by an error-like word to stay robust
  // if a future restricted-warning gets added on the summary screen.
  get saveErrorBanner(): Locator {
    return this.page.locator('.wf-restricted-warning').filter({
      hasText: /failed|Échec|Erreur|Error/i,
    });
  }

  // ─── Workflow name input (info step) ────────────────────────────────
  /** Workflow name input (placeholder "ex: Auto-fix 5xx errors"). */
  get nameInput(): Locator {
    return this.page.getByPlaceholder(/Auto-fix|Nom du workflow/i).first();
  }

  /** Switch to advanced mode (default in 0.6+, no-op when already there). */
  async selectAdvancedMode() {
    if (await this.advancedModeButton.isVisible().catch(() => false)) {
      await this.advancedModeButton.click();
    }
  }

  /**
   * Fill the name + click the QuickStart toggle to expand the panel.
   * Mandatory before any preset assertion or click since the picker is
   * disabled when the name is empty (gates the "click template before
   * naming the workflow" UX pitfall flagged 2026-05-17).
   */
  async openQuickStartPicker(name: string) {
    await this.selectAdvancedMode();
    await this.nameInput.fill(name);
    await this.quickStartToggle.click();
  }

  /**
   * Apply a preset / starter / suggestion by its visible title regex.
   * The wizard's `applyQuickStart` handler auto-jumps to advanced step 2
   * (Steps), matching the pre-0.8.5 behaviour of clicking a preset card.
   */
  async applyQuickStart(name: string, titleRe: RegExp) {
    await this.openQuickStartPicker(name);
    await this.quickStartApplyButton(titleRe).click();
  }

  /**
   * @deprecated 0.8.5 — preset cards moved from step 2 to step 0 (Infos).
   * Use `openQuickStartPicker(name)` to land on the picker, or
   * `applyQuickStart(name, titleRe)` to one-shot apply + jump to Steps.
   * Kept for any spec that wants to walk through the wizard pages
   * manually without touching the picker.
   */
  async gotoStepsPage(name: string) {
    await this.selectAdvancedMode();
    await this.nameInput.fill(name);
    await this.nextButton.click();   // Infos → Trigger
    await this.nextButton.click();   // Trigger → Steps
  }
}
