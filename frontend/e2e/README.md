# Kronn E2E tests (Playwright)

Tests end-to-end qui pilotent un vrai navigateur (Chromium headless) contre Vite + le backend Rust.

## Quick start

```bash
# Pré-requis : backend Kronn running (Docker `./kronn start` ou `make dev-backend`)
make test-e2e          # ou: cd frontend && pnpm test:e2e
make test-e2e-ui       # UI Playwright (debug visuel, headed)
```

Vite dev server est auto-spawné par Playwright (cf. `playwright.config.ts::webServer`). Le backend doit tourner séparément.

### Navigateurs Playwright (⚠ après chaque bump de version)

`pnpm test:e2e` installe automatiquement le binaire requis (via
`e2e/ensure-browser.mjs` — skippé en CI où les navigateurs sont pré-cuits dans
l'image conteneur). Si tu lances `npx playwright test`
directement après un bump de `@playwright/test`, chaque spec échoue en 0ms avec
`Executable doesn't exist … chromium_headless_shell-<rev>` — ce n'est PAS un bug
de l'app : chaque version de Playwright épingle sa propre révision de navigateur
(incident 2026-07-09 : deux suites entières perdues là-dessus).

Deux pièges connus sur ce poste :
- **Download qui rampe** : l'installeur Node télécharge parfois à quelques KB/s
  alors que le CDN répond à ~4 MB/s en curl. Plan B qui marche : télécharger le
  zip directement (`https://cdn.playwright.dev/builds/cft/<browserVersion>/mac-arm64/chrome-headless-shell-mac-arm64.zip`,
  `browserVersion` dans `playwright-core/browsers.json`), le dézipper dans
  `~/Library/Caches/ms-playwright/chromium_headless_shell-<rev>/` et `touch
  INSTALLATION_COMPLETE DEPENDENCIES_VALIDATED`.
- **Dossier partiel** : un download interrompu laisse un dossier sans le binaire
  dedans — `rm -rf` le dossier de la révision avant de réinstaller.

La CI n'est pas concernée : elle utilise l'image `mcr.microsoft.com/playwright`
avec les navigateurs pré-installés (le tag doit matcher la version de
`@playwright/test` — voir le commentaire dans `ci-test.yml`).

## Architecture

```
e2e/
├── fixtures/
│   ├── api-stubs.ts          # stubBootEndpoints() — bypass /api/setup/status hang
│   └── kronn-fixture.ts      # `test` étendu : auto stubs + tour skip
├── pages/
│   ├── DashboardPage.ts      # nav top
│   ├── WorkflowsPage.ts      # tabs + headers Workflows / QP / QA
│   ├── WorkflowWizardPage.ts # wizard de création (modes, présets, steps)
│   └── SettingsPage.ts       # cards skills/directives, see-more, badges
└── specs/
    └── *.spec.ts             # tests utilisateur
```

## Conventions

### 1. Importer `test` depuis le fixture, pas depuis Playwright direct

```ts
// ✓ bon — auto-stubs + tour skip
import { test, expect } from '../fixtures/kronn-fixture';

// ✗ mauvais — l'app reste bloquée sur le splash
import { test, expect } from '@playwright/test';
```

Sauf si tu testes **explicitement** le boot flow (setup wizard, splash, retry logic) — auquel cas tu importes vanilla et tu gères toi-même.

### 2. Selectors stables : `data-tour-id` > role+name > class

| Niveau | Quand l'utiliser | Exemple |
|--------|------------------|---------|
| `data-tour-id="X"` | Best — survit aux changements de label, i18n, locale | `[data-tour-id="nav-workflows"]` |
| `getByRole(role, { name: /regex/i })` | Quand pas de testid + accessible name stable | `getByRole('button', { name: /Quick APIs/i })` |
| `.class-name` | Last resort — fragile si refactor CSS | `.set-skill-card` |

Les regex i18n (`/Workflows|Automatisation/i`) marchent pour les rares cas où on doit matcher le label visible quel que soit la locale.

### 3. Page objects > selectors inline

Si une spec a besoin d'un selector qui n'est pas dans les page objects, **ajoute-le au page object**. Les specs doivent rester déclaratives (1-2 lignes par action).

### 4. Pas de `waitForTimeout` arbitraire

Préfère `expect(...).toBeVisible({ timeout })` ou `waitFor({ state: 'visible' })`. Les `waitForTimeout` masquent les race conditions.

## Configuration notable (`playwright.config.ts`)

- **`workers: 1`** — le backend Rust hang sous charge concurrente browser. Test serial obligatoire.
- **`fullyParallel: false`** — idem.
- **`retries: 1`** — flake mitigation. Le retry réutilise le même worker pour éviter la divergence d'état.
- **`locale: 'fr-FR'`** — Kronn tourne en FR par défaut, les regex de selectors assument FR. Si tu change la locale, ajuste les regex.

## Stubs / vrais backends

Les fixtures stubbent **2 endpoints** uniquement (boot) : `/api/setup/status` + `/api/config/ui-language`. Tout le reste passe au backend réel.

Pourquoi pas tout stubber : ça forcerait à maintenir un mirror de tous les endpoints API qui drift dès qu'un dev ajoute un champ. Stub minimal + backend vivant = meilleure couverture pour le coût.

Pourquoi stubber au minimum : sans ces 2 stubs le splash hang (axum middleware mutex sous concurrent browser load — à investiguer côté backend hors scope E2E).

## Ajouter une nouvelle spec

```ts
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

test.describe('Mon scénario', () => {
  test('fait X et vérifie Y', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    // Asserts
    await expect(workflows.tabWorkflows).toBeVisible();
  });
});
```

## Debug

```bash
# Mode headed (browser visible)
pnpm exec playwright test --headed

# UI Playwright (timeline + replay)
pnpm test:e2e:ui

# Single spec, debug mode (pause at breakpoints)
pnpm test:e2e:debug -- smoke.spec.ts
```

Les artifacts (screenshots / videos / traces) sont dans `test-results/` (gitignored). Trace zip dispo pour les retries via `playwright show-trace test-results/.../trace.zip`.

## CI (à venir Sprint 1.5 J4)

Pas encore intégré dans `.github/workflows/ci-test.yml`. Quand ce sera le cas :
- 1 job dédié `e2e-test` avec image Playwright officielle
- Backend Rust spawné dans le job (cargo run --release avec KRONN_DATA_DIR temporaire)
- Vite auto-spawné par Playwright
- Artefacts (videos / traces) uploadés en cas d'échec
