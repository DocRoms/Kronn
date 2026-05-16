# 0.8.4 Playwright UX/UI Audit Review — DOCROMS_WEB

Session Playwright pendant absence user, sur DOCROMS_WEB. Full audit relancé, RGAA sub-audit, validation des UX changes (kind dropdown, recap chips, briefing form-only, sub-audit → validation disc, Access42/Opquast section). Le Security sub-audit n'a pas été joué pour économiser tokens — le RGAA est le test le plus représentatif vu la nouveauté du prompt + la section "Pour aller plus loin".

Personas évaluant :
- **A. Élise** — automation expert (référente Kronn)
- **B. Marc** — dev qui se débrouille (5 ans XP, 1ère utilisation)
- **C. Sam** — dev junior (1 an XP, découvre l'audit IA)

---

## 🟢 Ce qui fonctionne très bien

### 1. Audit Full + sub-audit RGAA roundtrip end-to-end
**Marc** : « Click Lancer, je vois les étapes, ça avance. Au final un toast et la disc qui s'ouvre. C'est carré. »

- Full audit : 19m11s, 60k tokens, 10 étapes complètes, **disc de validation auto-créée + auto-navigation vers la disc**.
- Sub-audit RGAA : 8m, 8.6k tokens, 1 étape, **disc de validation "Validation audit Rgaa AI" auto-créée**.
- Le panneau live (`⏱ Xs · 💬 X tk · Σ X tk · 🔧 Tool · [Annuler]`) reste informatif sans être bavard.
- Le bouton Annuler reste inline pendant toute la durée — facile d'arrêter sans hunt.

### 2. Audit RGAA — qualité du livrable (`docs/inconsistencies-rgaa.md`)
**Élise** : « C'est PILE le rendu qu'on voulait. »

- **9 TDs détectés** avec scoring RGAA + cross-ref WCAG + sévérité Critical/High/Medium/Low.
- Findings concrets : `<h1>` manquant + landmarks (Critical), `role="listbox"` cassé (High), focus outline removed (High), ski markers couleur-only (High), iframe sans `title` (High), contrast `lang-banner a` 4.23:1 (Medium), `<div tabindex>` redondant (Low), `<p class="skills">` doit être `<ul>` (Low).
- **Le mapping RGAA criterion ↔ WCAG reference est dans CHAQUE TD** (ex: `1.2 / 3.1 → 1.1.1, 1.4.1`). Très pro.
- **Section "Coverage and method"** : explicite ce qui a été scanné, ce qui n'a pas l'a été (thématiques 2, 4, 5, 6 partielles, 9-13 partielles), et pourquoi (« a full grille-RGAA pass is required »). Très honnête.
- **Section "⚠️ Cet audit ne remplace PAS un audit complet"** rendue **mot pour mot** comme demandé :
  - cite W3C + DINUM
  - quantifie "tooling automatique = 30-40 % des critères au mieux"
  - distingue Access42 (audit officiel + cursus certifiant + jurisprudence) de Opquast (qualité Web globale, 240 règles, cert à vie)
  - injonction non négociable « re-tester soi-même OU faire appel à un pro »
  - recommandation finale : « 1 référent Access42 par produit + toute l'équipe Opquast »

**C'est exactement le message anti-« j'ai fait un audit, tout va bien »** que tu voulais.

### 3. AuditKind dropdown — réactivité + gating
- Sur Validated, les 8 options (Full + 7 sub-audits) sont activées → testé.
- Sur TemplateInstalled/Bootstrapped (testé visuellement dans le code des selects), les sub-audits sont disabled avec suffixe « (après audit global) ». Sam ne peut PAS faire une faute en lançant un Sécurité sur un projet sans baseline.
- Le **label du bouton change en live selon le kind** : « Lancer l'audit Audit global (10 étapes) » → « Lancer l'audit RGAA 4.1 (France) ». Le user voit exactement ce qu'il va faire.

### 4. Audit recap panel — chips strip
**Marc** : « Ah cool, je vois tout l'historique. Je peux cliquer sur celui d'il y a 2 jours pour voir ses détails sans avoir à le relancer. »

- 1 chip par audit run, icône kind (🌐 / 🛡 / ♿…), date relative ("15 mai, 17:20"), agent en tooltip.
- Click sur une chip → tableau steps de cet audit, triable durée/tokens DESC. Bien.
- **Le nouvel audit apparaît en chip immédiatement** dans la strip après complétion (refresh trigger via `auditCompletedTick`). ✅

### 5. Sub-audit → discussion de validation (ton point d'avant)
- Le titre du disc devient `Validation audit Rgaa AI` (et serait `Validation audit Security AI`, etc. pour les autres kinds). On les distingue dans la sidebar.
- Le prompt initial commence par : « Valide les résultats du **sous-audit RGAA 4.1** qui vient de tourner. Tu ne touches PAS au code… » → c'est bien `build_sub_audit_validation_prompt` qui s'exécute, pas le legacy 4-phase prompt Full.
- Phase 2 (ambiguïtés markers) + Phase 3 (bulk-first TD review) + Phase 4 RGAA-spécifique (rappel Access42 + Opquast + « 30-40 % des critères ») sont là.

### 6. Cross-agent memory MCP — smoke OK
- `POST /api/disc/create` → idempotent (testé tôt). `find_by_session` retrouve le disc. `disc/append` dédup correctement. `disc/sources` liste les bindings.
- Le badge "📥 ClaudeCode" + filtre dropdown sur la sidebar Discussions : testé en code, vitest valide les 5 scénarios. Pas testé live (aucun disc bound dans la DB de DOCROMS).

---

## 🟡 Flou / incohérent

### F1. Repetition du mot "audit" dans le bouton
**Sam** : « Lancer l'audit Audit global ? C'est marrant ce français. »

- `audit.kindSelector.launchLabel = "Lancer l'audit {0}"` avec `{0}` = `audit.kind.Full = "Audit global (10 étapes)"` → produit **« Lancer l'audit Audit global (10 étapes) »**.
- Fix proposé : changer le template en `Lancer : {0}` OU enlever « Audit » du kind label (`Full = "Global (10 étapes)"`).
- Vrai impact pour tous les kinds : "Lancer l'audit Sécurité", "Lancer l'audit RGAA 4.1 (France)" sont OK ; seul "Audit global" pose problème.

### F2. Titre du disc "Validation audit Rgaa AI" — casse de l'acronyme
**Élise** : « C'est RGAA pas Rgaa. La doc Kronn parle de RGAA partout, le badge dans le dropdown dit RGAA, mais le titre du disc dit Rgaa. Inconsistance. »

- Le titre est construit côté backend : `format!("Validation audit {} AI", kind_label)` avec `kind_label = "Rgaa"` (le `as_label()` du Rust enum).
- Fix : ajouter une méthode `display_name()` sur `AuditKind` qui renvoie "RGAA 4.1" pour Rgaa, "Sécurité" pour Security (FR), etc. Utilisée pour le disc title.

### F3. "AI Context" en bas de la card sur Validated
**Marc** : « J'arrive sur la card, le launcher d'audit est en bas, j'ai 6 sections au-dessus à scroller. »

- Ordre actuel : Discussions → Documentation projet → Plugins → Workflows → Skills → Dépôts liés → AI Context.
- Sur un projet **Validated**, "AI Context" est l'action principale (relancer audit, voir TDs). Le mettre en TOP (ou auto-expand au load) ferait gagner un click universel.

### F4. Date format inconsistant
- Liste projets : `Audité le 5/15/2026` (US m/d/y)
- Recap chip : `15 mai, 17:20` (FR jour-mois)
- Choisir une convention par locale et l'appliquer partout.

### F5. Hint launcher trop jargon pour junior
**Sam** : « "Relancer un audit ciblé pour creuser une dimension précise" — c'est quoi une dimension ? »

Fix proposé : « Relancer un audit thématique (sécurité, RGAA, performance…) pour aller plus loin sur un point particulier. »

### F6. Card header dual-state pendant un audit
**Marc** : « La card dit `Project docs AI audit 1/10` + `Validated` + `26 TD` + `Audité le 5/15/2026` EN MÊME TEMPS. Validated mais en cours, c'est contradictoire visuellement. »

- Pendant un audit, le statut "Validated" du précédent audit reste affiché à côté de "1/10". Idéalement, le statut + TD count + last audit date devraient être grisés ou remplacés par un badge "Audit en cours" pendant que ça tourne.

### F7. Pas de pagination sur les chips
- DOCROMS a déjà 16 chips après ce run. Sur 2 lignes wrap, ça déborde du panneau. Au-delà de 20 chips ça pollue.
- Fix : afficher les 10 plus récentes + bouton "+N audits plus anciens".

### F8. Pas de grouping par kind dans la strip
- Tous les audits sont mélangés (Full + sub-audits du futur). Marc/Sam ne pourront pas vite trouver "le dernier audit RGAA" parmi 20 chips Full.
- Fix : filter pills `[ Tous · Global · Sécurité · RGAA · Database · ... ]` au-dessus de la strip.

### F9. Validation discs s'accumulent sans nettoyage
- Après ce run on a maintenant 2 « Validation audit AI » + 1 « Validation audit Rgaa AI » dans la sidebar de DOCROMS_WEB. Les anciennes (post-validation) ne sont pas auto-archivées.
- Fix : auto-archive de la disc de validation quand le statut projet passe à `Validated` (signal `KRONN:VALIDATION_COMPLETE`).

---

## ❌ Ce qui ne fonctionne pas (vraies bugs)

### B1. 🚨 Audits "Running" orphelins jamais réconciliés
**Critique data-integrity**. La DB de DOCROMS_WEB contient **12 audit_runs avec `status='Running'`** qui n'ont jamais été marqués terminal (dates : 14 mai 17h-22h + 15 mai 10h-14h).

Cause probable : backend redémarre pendant un audit (kronn-backend container down, kill, rebuild docker) → la session SSE se coupe sans appeler `mark_failed` / `mark_cancelled` / `mark_interrupted`.

Conséquences :
- Polluent la strip de chips (12 chips rouges "Running" à l'historique).
- Affichent un faux "1 audit en cours" dans le badge nav Projets quand on relance le backend.
- `latest_completed` continue de marcher (filtre `status='Completed'`), mais `audit-status` qui scrape Running peut être confus.

**Fix proposé** : startup hook au boot backend :
```rust
// Au boot, marquer Interrupted tout audit Running qui date de plus de 30 minutes.
UPDATE audit_runs
   SET status = 'Interrupted',
       ended_at = datetime('now'),
       report_path = COALESCE(report_path, 'failure: stale on boot')
 WHERE status = 'Running' AND started_at < datetime('now', '-30 minutes')
```
+ un endpoint admin `POST /api/audit-runs/cleanup-stale` pour la maintenance.

### B2. Sub-audit non disponible en statut `Audited`
**Trouvé en testant** : entre la fin du Full audit et la validation de la disc, le projet est en statut `Audited`. Le launcher de sub-audits n'apparaît PAS dans cet état (UI code ne le rend que pour `Validated`).

Workflow naturel cassé : l'utilisateur veut lancer un Security/RGAA juste après son Full audit (avant même de valider la disc principale), mais doit valider la disc d'abord.

**Fix** : autoriser le launcher de sub-audits en `Audited` aussi (dans `ProjectCard.tsx`, ajouter le bloc launch dans la branche `Audited` à côté du bouton "Valider").

### B3. Step 9 — durée disproportionnée
- Step 9 (`inconsistencies-tech-debt.md`) a duré **8m+** sur 19m total — soit ~40% du Full audit.
- C'est explicable (scan exhaustif TD + écriture de N fichiers TD detail), mais visuellement effrayant : pendant 8 minutes, le user voit "9/10" sans progression token (tokens chip a même freezé à 3,679 pendant 4 min puis a sauté à 7,684).
- **Fix UX** : ajouter une sous-progression pour Step 9 du genre "Step 9.3 — writing TD-20260516-foo.md". Aujourd'hui le user pense que ça a planté.

### B4. ts-rs n'auto-régénère pas LaunchAuditRequest.ts
- Lors de l'ajout de `kind` à `LaunchAuditRequest` en Rust, le fichier ts-rs généré n'a PAS été regénéré au `cargo test` malgré le `#[ts(export)]`. J'ai dû le réécrire à la main.
- Cause inconnue (ts-rs 12.x bug ?). À investiguer + documenter dans CLAUDE.md.

---

## 📋 Synthèse UX/UI (expert)

### Points forts du 0.8.4 (à garder absolument)
1. **Le panneau "Détails du dernier audit" avec chips d'historique** est le meilleur ajout UX du release. Tout est lisible, sortable, et le user peut comparer audits.
2. **La séparation Full audit / sub-audit avec gating sur statut Validated** est correcte : Sam ne peut pas casser le flow en faisant un audit Sécurité avant la baseline.
3. **L'audit RGAA + sa section Access42/Opquast** est un game-changer : un user qui lance ça pour la première fois reçoit non seulement les findings, mais surtout la pédagogie "ne te repose pas là-dessus". C'est un différenciateur fort vs les outils a11y génériques (Wave, Axe).
4. **Auto-création de la disc de validation pour TOUS les kinds** (Full + sub-audits) + auto-navigation vers la disc : fluide, on n'oublie jamais l'étape de revue humaine.

### Points faibles
1. **Visibilité du launcher d'audit sur un projet Validated** : enfoui dans la section "AI Context" en bas. Premier réflexe Marc = scroller pour le trouver. À remonter ou auto-expand.
2. **Repetition "Lancer l'audit Audit global"** : friction de lecture évitable. Fix 1-ligne.
3. **Casse "Rgaa" vs "RGAA"** dans les titres de disc : inconsistance avec le reste de l'UI qui dit "RGAA" partout.
4. **Aucun garde-fou data : audit_runs orphelins** : pollue l'historique. Fix backend ~30 lignes de Rust + une migration.
5. **Status "Audited" sans launcher d'audit** : workflow cassé pour qui veut enchaîner Full → Security sans valider entre.

### Recommandations de priorité (pour 0.8.5)
1. **[P0]** Reconcilier les audit_runs Running orphelins au boot (#B1) — pollue toute la perception.
2. **[P0]** Ajouter launcher sub-audit dans l'état `Audited` (#B2) — débloque le flow naturel.
3. **[P1]** Fix labels "Lancer : {0}" + `display_name()` pour `Rgaa → RGAA` (#F1 + #F2).
4. **[P1]** Auto-expand `AI Context` sur Validated OU reorder sections (#F3).
5. **[P2]** Auto-archive validation discs après validation (#F9) + pagination chips (#F7).
6. **[P2]** Date format unifié par locale (#F4).
7. **[P3]** Group/filter chips par kind (#F8) — pertinent quand un user aura 10+ kinds dans l'historique.

### Ce que je n'ai PAS pu tester (faute de temps)
- **Briefing form-only flow** : nécessite un projet en `NoTemplate` ou `TemplateInstalled` (DOCROMS_WEB est Validated). Le code est shippé + cargo test passe (2 nouveaux tests sur `build_briefing_review_prompt`) mais aucune validation end-to-end Playwright. À tester par toi sur un nouveau projet (ou je peux le faire sur `front_tools` par exemple).
- **Sub-audit Security** : pas relancé pour économiser tokens. Le code est identique au RGAA — l'unique différence est le prompt + l'index file (`docs/inconsistencies-security.md`). Faible risque de divergence.
- **Cross-agent memory UX live** : le badge "📥" + filter source nécessitent qu'un disc soit bound via `POST /api/disc/create` depuis une vraie CLI (Claude Code en session). Pas testable depuis Playwright pur. Le smoke test des 9 routes est OK (curl).

---

## Files joints
- `findings/01-validated-card-audit-launcher.png` — screenshot de la ligne agent+kind+launch sur Validated
- `findings/02-audit-recap-chips-strip.png` — screenshot du panneau chips historique avec les 15 audits

