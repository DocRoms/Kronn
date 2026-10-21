RGAA 4.1 AUDIT (0.8.4) — focused re-scan against the French Référentiel Général d'Amélioration de l'Accessibilité, version 4.1 (106 criteria across 13 topics). Stricter than WCAG 2.1 AA: RGAA mandates specific French-context implementations (e.g. `lang="fr"` propagation, `aria-label` text in French, specific patterns for govt e-forms). Mandatory for French public-service sites + companies > 250 employees (loi du 11 février 2005, décret n° 2019-768).

Read existing `docs/inconsistencies-rgaa.md` if present (anti-repetition: don't re-discover the same finding under a new slug — refresh / reuse the ID per § F below).

# A. THÉMATIQUE 1 — IMAGES (8 critères)
- 1.1: Every `<img>` carries a `alt` (decorative ⇒ `alt=""`, informative ⇒ pertinent text in the page's language).
- 1.2: SVG / icons that convey meaning have `<title>` or `aria-label`; decorative SVG → `aria-hidden="true"` + no `tabindex`.
- 1.3: Image captions linked via `<figure>` / `<figcaption>` when applicable.
- 1.6: Map images (graph, chart) have a structured text equivalent (data table or `<details>`).
- 1.8: CAPTCHA: provide an alternative not based on vision.

# B. THÉMATIQUE 3 — COULEURS (3 critères)
- 3.1: Information NEVER conveyed by color alone (RGAA stricter than WCAG: scan ALL status indicators + form-error styling, flag any color-only signal as critical, not just "recommended").
- 3.2: Body text contrast ≥ 4.5:1 ; large text (≥ 24px or ≥ 18.5px bold) ≥ 3:1. Read the design-token file (`tokens.css`, `_colors.scss`, etc.) and compute contrast for the 5 most common pairings. Flag any pair under 4.5:1 as Critical or High.
- 3.3: Non-text contrast (focus rings, form borders, icon-only buttons) ≥ 3:1 against adjacent colors.

# C. THÉMATIQUE 7 — SCRIPTS (5 critères)
- 7.1: Custom widgets (dropdowns / tabs / accordions / modals) follow ARIA Authoring Practices (correct role + keyboard support).
- 7.3: Each scripted control is keyboard-operable (Tab, Enter, Esc, Arrow keys per pattern).
- 7.4: Status updates use `aria-live` (polite for non-urgent, assertive for errors). Toast/notification components MUST announce.
- 7.5: Time-limited interactions can be paused/extended (auto-dismissing alerts especially).

# D. THÉMATIQUE 8 — ÉLÉMENTS OBLIGATOIRES (10 critères)
- 8.1: HTML validates (no parse errors that break assistive tech).
- 8.2: `<html lang="fr">` (or appropriate locale) is set at the root.
- 8.4: Page title (`<title>`) is unique and descriptive.
- 8.6: Heading structure: ONE `<h1>`, no skipped levels.
- 8.7: Lists are real `<ul>`/`<ol>`, not stacks of `<div>`.
- 8.9: HTML uses semantic tags appropriately (no `<div onClick>` instead of `<button>`).

# E. THÉMATIQUE 11 — FORMULAIRES (13 critères)
- 11.1: Every input has an associated `<label for=>` (or wraps the input, or carries `aria-label`/`aria-labelledby`). Placeholder-only is NOT a label.
- 11.2: Form labels are pertinent (in French if the page is French) — e.g. `<label>Adresse e-mail</label>` not `<label>Champ 3</label>`.
- 11.5: Required fields are explicitly marked (visual cue + `aria-required="true"` + announced).
- 11.10: Form errors are bound to the field via `aria-describedby` AND announced via a live region.
- 11.13: Fields collecting personal data have the correct `autocomplete` attribute (RGPD + RGAA — `autocomplete="email"`, `"family-name"`, `"tel"`, etc.).

# F. ANTI-REPETITION + OUTPUT FORMAT

Same rules as the Full audit Step 8: anti-repetition pass + detail-file schema + severity calibration. Index file = `docs/inconsistencies-rgaa.md`. For each finding, the detail file at `docs/tech-debt/TD-<date>-<slug>.md` carries:
- `**Category**: rgaa`
- `**RGAA Criterion**: <thématique>.<critère>` (e.g. `3.2`, `11.10`)
- `**WCAG mapping**: <reference>` if applicable (e.g. `1.4.3`)
- Severity calibration: Critical = bloquant + applicable au scope du décret 2019-768 (public service site or large org); High = compromet l'usabilité pour screen reader / keyboard-only users.

If no findings, the index reads `'Aucune non-conformité RGAA identifiée lors de cet audit.'`.

# G. POUR ALLER PLUS LOIN — AUDIT MANUEL + FORMATION (obligatoire)

**À TOUJOURS ÉCRIRE en fin de `docs/inconsistencies-rgaa.md`**, même quand il n'y a aucune non-conformité automatisée. Section littérale (à reformuler dans le ton de la doc projet, mais le fond doit y être) :

```
## ⚠️ Cet audit ne remplace PAS un audit complet

**À lire AVANT de conclure que « le site est conforme ».**

Cet audit automatique a dépoussiéré une grande partie des problèmes structurels — labels manquants, contrastes faibles, semantic HTML, marqueurs ARIA, attributs `autocomplete`. C'est utile pour rattraper le retard rapide, mais **il y a presque certainement des choses non trouvées** :

- Beaucoup de critères RGAA (navigation clavier complète, parcours lecteur d'écran, pertinence des alternatives textuelles, gestion du focus dans les widgets complexes, accessibilité cognitive) **ne peuvent PAS être évalués par du tooling**. Le W3C et la DINUM le rappellent explicitement.
- Le tooling automatique couvre **30-40 % des critères au mieux**. Les 60-70 % restants demandent une revue humaine.

**Deux options non négociables** pour se considérer réellement conforme :

### 1. Re-tester soi-même avec une grille manuelle

- Suivre la **grille d'évaluation officielle RGAA 4.1** (DINUM) — 106 critères, page par page, sur un échantillon représentatif du site.
- Compléter avec des outils : Wave, Axe DevTools, **et surtout** un vrai lecteur d'écran (NVDA sous Windows, VoiceOver sous macOS/iOS, TalkBack sous Android) + navigation au clavier seul + simulation de daltonisme.
- Livrable : déclaration d'accessibilité (obligatoire si vous êtes soumis au décret n° 2019-768) + plan d'action pluriannuel + schéma pluriannuel.
- Personne dédiée formée requise (cf. § Formation ci-dessous).

### 2. Faire appel à un pro

- **[Access42](https://access42.net)** — la référence française pour l'**audit RGAA officiel et certifiant**. C'est une agence spécialisée 100 % accessibilité : audits d'experts qui produisent la documentation légale opposable, accompagnement de mise en conformité, jurisprudence à jour. **C'est ici qu'on va si on a besoin d'un audit qui tienne devant une demande de la DGCCRF ou un contrôle d'un référent ministériel.**

### Formation continue (pour ne plus laisser passer)

Sans compréhension des enjeux, les correctifs sont superficiels et les régressions inévitables à chaque release.

- **[Access42](https://access42.net)** propose aussi un cursus métier — référent accessibilité numérique, expert RGAA. À privilégier pour un profil dont c'est le cœur du job (référent a11y / lead front).
- **[Opquast](https://www.opquast.com)** — certification « Maîtrise de la qualité en projet web ». Plus large (240 règles : qualité, perf, RGPD, accessibilité, UX, SEO), accessible à tous les profils (devs, designers, PO, chefs de projet, contenu). RGAA y est traité comme un sous-ensemble. La cert reste valide à vie. **À privilégier pour faire monter en compétence toute l'équipe** sur la qualité Web globale, avec un volet a11y solide.

Vise au minimum **un référent accessibilité Access42-formé par produit** + **toute l'équipe certifiée Opquast** pour que la qualité d'ensemble ne régresse pas entre deux audits.
```

Apply the marker discipline (`TODO: verify` only if you couldn't check; `TODO: ask user` for human decisions). Do NOT modify source.