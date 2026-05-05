import type { Page, Locator } from '@playwright/test';

/**
 * Workflow creation wizard. Opens after clicking "Nouveau workflow" on the
 * Workflows page. Modes : `simple` / `advanced` (toggle at top). Steps page
 * (advanced mode) shows the preset cards at top + the step editor below.
 */
export class WorkflowWizardPage {
  constructor(private readonly page: Page) {}

  // ─── Mode toggle (advanced gives access to presets) ─────────────────
  get advancedModeButton(): Locator {
    return this.page.getByRole('button', { name: /Avancé|Advanced/i }).first();
  }

  // ─── Preset cards ───────────────────────────────────────────────────
  /** Preset card by its title (i18n-aware regex). */
  preset(titleRe: RegExp): Locator {
    // Presets render their title inside `.wf-preset-card` — but to keep the
    // selector resilient to class renames, anchor on the preset id which
    // is in the i18n key. Easiest stable handle: clickable element whose
    // accessible name matches the title.
    return this.page.getByRole('button', { name: titleRe });
  }

  get presetAutoDev(): Locator { return this.preset(/Auto-Dev avec tests|Auto-Dev with tests/i); }
  get presetTicketToPr(): Locator { return this.preset(/Ticket Autopilot/i); }
  get presetDailyHostAudit(): Locator { return this.preset(/Audit quotidien|Daily audit/i); }

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

  /** Switch to advanced mode (which exposes the preset cards on the
   *  Steps page). NB: advanced is the default in 0.6+. */
  async selectAdvancedMode() {
    if (await this.advancedModeButton.isVisible().catch(() => false)) {
      await this.advancedModeButton.click();
    }
  }

  /** Walk through the advanced wizard from Infos → Steps :
   *   1. fill the name input (required to enable Next on Infos)
   *   2. click Next (Infos → Trigger)
   *   3. click Next (Trigger → Steps)
   *  After this, preset cards are visible.
   */
  async gotoStepsPage(name: string) {
    await this.selectAdvancedMode();
    await this.nameInput.fill(name);
    await this.nextButton.click();   // Infos → Trigger
    await this.nextButton.click();   // Trigger → Steps
  }
}
