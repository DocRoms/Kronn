# Audit live — Autopilot décomposé (PR-C) sur EW-7247

> Run réel `15cf080d` (workflow `a020a277`), projet front_euronews, ticket EW-7247 (Epic migration Africanews multi-brand). But : relever finement ce qui marche / cloche pour fiabiliser les prompts de l'autopilot avant les prochains tickets. Date : 2026-06-11.

## Étape `fetch_issue` (JsonData) — ✅
- 0 token, instantané. Le corps EW-7247 seedé via l'endpoint `templates/feasibility-autopilot` est bien lu par le triage.

## Étape `triage` (Agent, 473 s ≈ 8 min) — ✅✅ excellent

**Ce qui marche :**
- **Canal worktree** : `.kronn/triage-manifest.md` (12,6 Ko) écrit comme demandé → l'enfant pourra le lire. Markdown propre (sections CLEAR/DECIDED/MOCKED/BLOCKED, champs `Where`, `Evidence`).
- **Audit-only respecté** : `git status` du worktree VIDE — aucun fichier code modifié. La règle « audit, don't code » tient.
- **Cross-repo evidence-lift (feature killer) opérationnel** : couleurs SCSS liftées du linked repo, pas inventées — primary `#f9d713` (evidence `front_africanews/…/_layout-variables.scss:14`), orange `#ea5932` (`:22`), fonts Overpass (`:59-60`). DFP `africanews-new` confirmé depuis la source front_africanews.
- **Grounding code > ticket** : `channel_id` AN=6 confirmé via `ChannelEnum.php:15`. Refs concrètes partout (`LocaleRequestListener.php` pour le pattern listener, priorité 210 > 200, etc.).
- **Classification juste** : 11 clear / 4 decided / 2 mocked / 5 blocked. Les 5 blocked correspondent EXACTEMENT aux bloqueurs que le ticket flagge (Adobe DTM URLs + visitor config → Data team) + scope correct Phase 0 = code, Phases 1-3 = hors-codebase (CMS/DevOps/Fastly).
- **`agent_decisions` ingérés sur le run PARENT** (INV-2 préservé par la décompo) : 11 rows (decided+mocked+blocked ; clear non ingéré = correct).
- **Schéma typé valide** (TypedSchema on_invalid=Fail passé).

**🏆 Pépite — ticket↔code discrepancy capturée :**
- Ticket EW-7247 : « channel_id Euronews = **2** ».
- Vrai code `ChannelEnum.php:10` : `EURONEWS_WEB = **1**`.
- L'agent a utilisé **1** (la valeur du code) en citant la source, IGNORANT le **2** du ticket. → c'est le comportement voulu (grounding code) ET ça lève un vrai écart ticket↔prod que l'humain doit trancher au Gate. À confirmer : le ticket est-il périmé, ou EN doit-il vraiment être 2 ?

**Pistes d'amélioration prompt (mineures) :**
- (À surveiller sur l'implement) Le triage cite EN=1 partout de façon cohérente — bon. Mais le prompt triage pourrait **expliciter** quand il détecte un écart ticket↔code : émettre une note dédiée (`decided` avec `why` = « ticket dit X, code dit Y, je retiens Y car ground truth ») plutôt que de l'enterrer dans une entrée CLEAR. Rendrait l'écart plus visible au Gate.

## Étape `review_triage` (Gate) — ✅ atteinte, en pause `WaitingApproval`
- Le manifeste s'affiche dans le gate_message via `{{steps.triage.data}}`. request_changes_target = triage. Décision en cours.

## Gate `review_triage` — ✅ approuvé (pause 347 s)
- Décision `approve` acceptée, run repris. Commentaire posé notant l'écart channel EN.

## Étape `feasibility_impl` (SubWorkflow → enfant) — ⏳ en cours
- **Phase 2 handoff confirmé LIVE** : run enfant `1458b15a` spawné, `parent_run_id=15cf080d` (linkage OK), et **aucun worktree enfant séparé** → l'enfant partage bien le worktree du parent (inherited_workspace fonctionne). Implement en cours dans ce worktree.

## Étape `feasibility_impl` / enfant `1458b15a` — ✅ Success (599 s)
- **Phase 2 confirmé au niveau git** : l'enfant a travaillé sur la branche du PARENT (`kronn/…/15cf080d`), worktree partagé.
- **implement (593 s)** : a créé TOUS les fichiers Phase-0 (7 nouveaux dossiers brands + Enum/Brand + BrandRequestListener + Services/Brand) ET modifié 7 points d'intégration (Apollo ArticleApi/SearchApi/WireApi, AdobeAnalyticsBuilder, NavbarBlock, CommonContentService). Matche `files_touched`.
  - ✅ `.kronn/decisions.md` (2,2 Ko) — vrai raisonnement d'écart (BrandContext nullable pour ne pas casser les tests).
  - ✅ Marqueurs `KRONN-ASSUMED/MOCKED/TODO` partout, chacun référence le `decision_id` du manifeste.
  - ✅ **Evidence-lift réelle** : `#f9d713 // :14`, `#ea5932 // :22` liftés de front_africanews (pas inventés).
- **run_tests (3 ms)** : skip propre `[SIGNAL: SKIPPED]` (node_modules absent) → exit 0, ne bloque pas. ⚠️ Aucune validation réelle des tests.
- **drift_check (46 ms)** : a listé tous les marqueurs KRONN. 0 token.

## Étape `pr_draft` (Agent, 156 s) — ✅
- A lu decisions.md + le manifeste, produit un corps de PR propre : `## Ticket / ## Summary / ## Implemented` (tableau ID·Marqueur·Location) `/ ## Still open (blocked)` (routage par équipe : Data team, DevOps…).
- **✅ Envelope enrichie validée** : `feasibility_impl.data.last_output` = sortie drift_check (marqueurs) → pr_draft y a accès. Flux cross-boundary OK (manifeste via parent · drift via envelope · decisions via fichier partagé).
- ⚠️ L'output contient du bavardage agent avant le corps de PR (« Reading the required files… Let me check the channel ID… »).

## 🔬 Comparaison AVANT (run 1 `15cf080d`) / APRÈS (run 2, prompts améliorés)

| Point | AVANT (run 1) | APRÈS (run 2) attendu |
|---|---|---|
| Child steps | 3 (implement/test/drift) | **4** (+ `commit`) |
| Commit du code | ❌ non commité → perdu au cleanup | ✅ commité sur la branche parent (survit) |
| Scope | ⚠️ a modifié `docs/AGENTS.md` (hors files_touched) | ✅ strictement dans files_touched |
| pr_draft | ⚠️ préambule bavard avant le corps | ✅ commence direct par `## Ticket` |
| run_tests | skip (deps absentes) | skip idem (deps non installées — réglage projet) |
| triage / evidence-lift / agent_decisions | ✅ | ✅ (inchangé) |

### Résultat run 2 (37867ebf) — 2 fixes sur 3 OK
- ✅ **Commit** : `a8548876 Kronn AutoPilot — implementation (KRONN-traced)` est le HEAD, `git status` vide, branche en avance → le code Phase-0 **survit au cleanup**. Child commit step Success. **RÉSOLU.**
- ✅ **pr_draft préambule** : sortie commence exactement par `## Ticket\nEW-7247\n\n## Summary…` — zéro bavardage. **RÉSOLU.**
- ⚠️ **Scope discipline — NON résolu** : l'agent a quand même modifié `docs/AGENTS.md` (+7) et `docs/architecture/multi-brand.md` (+28), SANS le logger dans decisions.md, malgré la règle 1 durcie. Le prompt seul ne suffit pas à contraindre le scope d'un agent.
  - **Fix suivant proposé (déterministe, pas prompt)** : un Exec post-implement qui fait `git diff --name-only` vs `files_touched` du manifeste → tout fichier hors-liste est soit (a) auto-loggé dans decisions.md, soit (b) revert (selon politique). 0 token, fiable. Note : `docs/architecture/multi-brand.md` est référencé PAR le ticket → potentiellement légitime ; `docs/AGENTS.md` plus douteux. La vérif déterministe tranche au cas par cas + rend l'écart visible.

## Verdict global du run 1 — ✅ SUCCESS de bout en bout
5 steps parent tous Success (fetch 0s · triage 473s · gate 347s · enfant 599s · pr_draft 156s). La décompo + Phase 2 + envelope + agent_decisions + evidence-lift + traçabilité KRONN : **tout fonctionne live sur le gros ticket EW-7247.**

---

## 🛠️ AMÉLIORATIONS PROMPT À FAIRE (consolidé, pour les prochains tickets)

**Bugs/risques (à corriger) :**
1. **implement ne commite pas** → tout reste en `M`/`??` → au cleanup le worktree est supprimé → **code Phase-0 perdu**. Fix : ajouter une étape commit (Exec `git add -A && git commit` après implement, ou instruction dans le prompt implement). Vaut aussi pour `ticket-to-pr` (create_pr suppose des commits).
2. **scope creep `docs/AGENTS.md`** : implement a modifié AGENTS.md (hors `files_touched`). Renforcer la règle 1 du prompt implement (« reste STRICTEMENT dans files_touched ; toute autre modif → log dans decisions.md + justifie »).
3. **run_tests jamais exécuté** (deps absentes) → pas de validation. Ajouter `exec_setup_command` (composer/pnpm install) au step run_tests de l'enfant — au prix du wall-clock. Au minimum, le pr_draft devrait dire clairement « tests NON exécutés (deps absentes) », pas seulement SKIPPED enfoui.

**Qualité (mineur) :**
4. `triage` : expliciter les écarts ticket↔code dans une entrée dédiée (ex. channel EN code=1 vs ticket=2) plutôt que noyé dans un CLEAR.
5. `pr_draft` : supprimer le préambule bavard de l'agent (directive « émets UNIQUEMENT le markdown du corps de PR, aucune phrase avant »).

**Désagentification (gain tokens — cf section ci-dessus) :**
6. `pr_draft` → Exec template (JSON manifeste + drift + statut → markdown). ~1 run Agent économisé. Le run live confirme que pr_draft n'a fait QUE de l'agrégation mécanique + de la prose résumé → parfaitement désagentifiable.
7. `recon` Exec avant triage (grep/ls ciblé du repo + linked_repos) pour réduire les 473 s / tokens d'exploration du triage.

---

## 💸 Désagentification — où cramer MOINS de tokens (analyse 2026-06-11)

Steps actuels : `fetch_issue`(JsonData,0) → **triage**(Agent) → `review_triage`(Gate,0) → enfant[ **implement**(Agent) → `run_tests`(Exec,0) → `drift_check`(Exec,0) ] → **pr_draft**(Agent). 3 steps Agent = tout le coût.

### 1. `pr_draft` → quasi-déterministe (le plus gros gain, ~1 run Agent économisé)
Le corps de PR est une **agrégation mécanique de données DÉJÀ structurées** : le manifeste (`steps.triage.data` = JSON clear/decided/mocked/blocked avec `why`/`strategy`/`needed_from`), la sortie `drift_check` (texte), le statut tests. Les sections « Implemented / Still open (blocked) / Decisions taken / Mocks to replace » sont des **transcriptions directes** des tableaux du manifeste. Seule la prose « ## Summary » (1-3 phrases) demande un LLM — et encore, le manifeste a DÉJÀ un champ `summary`.
- **Proposition** : remplacer `pr_draft` (Agent) par un **Exec** (petit python/jq) qui lit `.kronn/triage-manifest.md` + l'output drift + le statut, et rend le markdown PR via template. **0 token.** Au pire, garder un mini-Agent pour la seule phrase de résumé (ou réutiliser `summary` du manifeste → 0 token total).
- Gain : 1 run Agent complet supprimé par exécution.

### 2. `recon` Exec AVANT `triage` (split collecte/analyse — règle #2 de l'architect)
Le triage a mis **473 s** dont une grosse part à **explorer le repo + les linked_repos** (grep des color tokens, channel enums, listeners). Cette exploration est **mécanique**.
- **Proposition** : un step **Exec `recon`** déterministe (`git ls-files`, `grep -rn` ciblé : `ChannelEnum`, `*Listener`, `_tokens*.scss`, `config/brands`, color hex dans le linked repo) qui produit un JSON « candidate evidence ». Injecté dans le prompt triage → l'agent **raisonne sur des faits pré-collectés** au lieu de naviguer à l'aveugle. Réduit les tokens d'exploration ET le wall-clock du triage.
- Gain : moins de tokens triage (collecte = 0 token au lieu de tokens d'agent qui grep).

### 3. Déjà optimaux (à garder tels quels)
`fetch_issue` (JsonData/ApiCall, 0), `review_triage` (Gate, 0), `run_tests` (Exec, 0), `drift_check` (Exec, 0). ✓ La discipline désagentification tient sur 4/7 steps.

### 4. `implement` — irréductible (mais déjà optimisé)
La génération de code Phase 0 demande un vrai LLM, pas désagentifiable. MAIS l'optimisation est déjà là : implement **lit le manifeste** (`.kronn/triage-manifest.md`) au lieu de ré-explorer → l'exploration du triage n'est pas refaite. Le manifeste EST la désagentification de l'exploration d'implement.

### Synthèse
Cible token-optimale du preset : **2 steps Agent** (triage + implement) au lieu de 3 — en désagentifiant `pr_draft`. + un `recon` Exec pour alléger le triage. Soit ~1.3 run Agent économisé par ticket, sans perdre la traçabilité (manifeste + KRONN-* + decisions.md restent).

## 🚀 Désagentification — pistes pour la Phase 3 (décomposition par sous-tâche / BatchWorkflow)

Principe : **maximiser les Exec déterministes, minimiser les appels agent.** Le run live confirme que le feedback test→agent est déjà 0-token ; on pousse le principe.

- **A. Gate de checks statiques AVANT les tests** : Exec `php -l` / `tsc --noEmit` / lint / `composer validate` / `git diff --check` entre implement et run_tests. Fail → retour implement avec l'erreur exacte. Attrape les fautes triviales sans brûler une itération agent + suite de tests complète. 0 token.
- **B. ⭐ Completeness-check déterministe (anti-skip)** : diffé les marqueurs `KRONN-*` que `drift_check` trouve VS les `decision_id` du manifeste. Tout `decided/mocked/blocked` sans marqueur ⇒ l'agent a sauté une sous-tâche ⇒ fail → retour implement. Vérifie mécaniquement (0 token) que l'agent a TOUT couvert — remplace une partie de la review agent.
- **C. ⭐ Template-générer les items 100% déterminés par le manifeste** : config/enum/yaml dont les valeurs sont déjà liftées (channel_id, DFP unit, couleurs) ⇒ génération par Exec template, 0 token. L'agent ne fait que la logique (listeners, injection runtime).
- **D. Fan-out gratuit** : en BatchWorkflow, 1 sous-WF par sous-tâche piloté par le tableau du manifeste déjà produit — aucun agent « planner ».
- **E. Merge déterministe** : rebase séquentiel des branches sous-WF (Exec, 0 token) ; agent invoqué uniquement sur conflit.
- **F. Tests scopés** : ne lancer que les tests touchant les fichiers du sous-WF courant (mapping changed-files→tests, déterministe).

Cible Phase 3 : sur N sous-tâches, l'agent ne tourne QUE pour la logique + la résolution de conflits ; tout le reste (split, génération de config, checks statiques, merge, completeness) est 0-token.

## 🌙 Night shift 2026-06-12 — Phase 3b (fan-out per-task) livrée + runs 3/4

### Run 3 (monolithe + couche 3a) — ✅ Success, 72 181 tokens, ~26 min
- Triage enrichi vérifié : `scope/complexity/mechanical/acceptance` par item + `files_touched.txt` + `decision_ids.txt` écrits.
- `scope_check` a flaggé du hors-scope dans decisions.md ✓ (advisory).
- 🐛 **Trouvé** : le `[SIGNAL: MISSING]` inline de completeness_check ne déclenche PAS `on_result` (les signaux matchés = dernières lignes, où l'Exec émet OK/exit_0). **Fix : exit 3 → `[SIGNAL: exit_3]`**. + `decision_ids.txt` = decided+mocked seulement (les blocked n'ont pas d'emplacement code fiable).
- ✅ commit : 20 fichiers, branche préservée `acbb67cc`.

### Phase 3b livrée (MVP fan-out séquentiel)
- `WorkflowStep.sub_workflow_foreach_file` (+24 sites mass-patchés) ; executor `execute_foreach` : 1 run enfant PAR item de `.kronn/tasks.json` (écrit par triage, 4ᵉ fichier), item courant exposé via `.kronn/current_task.json`, worktree partagé séquentiel, cancel parent honoré entre items, cap 30, envelope `{mode:foreach, total, succeeded, failed, items[]}` + signaux OK/PARTIAL/SUBWF_FAILED. Param endpoint `decomposed:true` → enfant per-task (implement lit SA tranche). Tests : 17 big_ticket + 5 sub_workflow + régression 3078 ✓.
- 🐛 incident infra : deadlock de shells `until pgrep -f "cargo test"` (s'auto-matchaient) → tués, leçon notée.

### Run 4 (DÉCOMPOSÉ, 13 items) — PARTIAL 12/13, 132 223 tokens, fan-out 1 825 s
- ✅ **Le fan-out marche** : 13 enfants séquentiels, ~1,5-2 min/item, **12 commits per-task** sur la branche, `current_task.json` = tranche enrichie, envelope foreach exacte `{13, 12, 1}` + `[SIGNAL: PARTIAL]`.
- ✅ **La boucle completeness→implement (exit_3) a fonctionné en live** : item `adobe-ts-africanews` re-implémenté 2× puis Failed proprement (l'agent écrivait un marqueur avec un id ≠ celui de la tâche — vrai catch).
- 🐛 **Trouvé : PARTIAL tuait le run** (12/13 ok → parent Failed, pr_draft jamais exécuté). **Fix : PARTIAL = step Success** (signal PARTIAL reste branchable) → pr_draft documente l'échec. + prompt per-task durci : marqueur = id EXACT + feedback `{{steps.completeness_check.output}}` au re-run.
- 💸 **Limite découverte (l'objectif !) : 132k tokens vs 72k monolithe** — la taxe de contexte ×13 dépasse le gain de scoping en séquentiel naïf. MAIS : 7/13 items étaient `mechanical:true` → en les routant vers Exec/template (0 token) + tier economy sur les `low`, l'équation s'inverse. Le fan-out apporte déjà : commits granulaires, vérif per-task, retry scopé (1 seul item re-roulé), 12/13 prouvés complets.

### Run 5 (DÉCOMPOSÉ v2 : PARTIAL=Success + marker-id exact + feedback loop) — ✅ SUCCESS 16/16
- **16/16 enfants Success, 0 échec** (le durcissement marker-id a éliminé l'échec type run-4 : zéro boucle completeness ce run).
- Envelope `{foreach, 16, 16, 0}` · **15 commits per-task** (1 item sans diff → skip propre) · branche préservée `ahead: 15` · pr_draft exécuté (81 s, 10,4 Ko, qualité OK — ⚠️ 1 phrase de préambule a encore fuité malgré la consigne, à régler définitivement par la désagent pr_draft→Exec).
- Totaux : **157 918 tokens** (enfants 117 139) · fan-out 2 387 s · wall ~56 min.

## 📊 COMPARAISON FINALE monolithe vs décomposé (EW-7247)

| | Run 3 (monolithe+3a) | Run 4 (décomposé v1) | Run 5 (décomposé v2) |
|---|---|---|---|
| Verdict | ✅ Success | ⚠️ 12/13 (bug PARTIAL→fixé) | ✅ **16/16 Success** |
| Tokens totaux | **72 181** | 132 223 | 157 918 |
| Implémentation | 1 run agent (728 s) | 13 enfants (1 825 s) | 16 enfants (2 387 s) |
| Wall total | ~26 min | ~47 min | ~56 min |
| Commits | 1 global | 12 per-task | **15 per-task** |
| Vérification | globale | per-task (1 vrai catch) | per-task, 16/16 prouvés |
| Retry si échec | tout re-roule | 1 seul item | 1 seul item |
| Variance triage | — | 13 items | 16 items (non-déterminisme granularité) |

**Lecture honnête** : en séquentiel naïf, le décomposé coûte aujourd'hui **~2× tokens et ~2× temps** (taxe de contexte × N) MAIS apporte fiabilité prouvée par tâche, commits granulaires (PR lisible), retry scopé. **Le flip économique est identifié et à portée** : 12/16 items étaient `mechanical:true` → les router vers Exec/template (0 token) éliminerait ~75 % des appels implement → projection SOUS le coût monolithe, avec tous les bénéfices. + tier economy sur `complexity:low`.

## 🌅 RÉCAP DU MATIN (night shift 2026-06-12, ~00h-03h)

**Livré & validé live cette nuit :**
1. **3a validé sur run 3** (manifeste enrichi scope/complexity/mechanical/acceptance · 4 fichiers `.kronn/` · scope_check advisory · commit step).
2. **Phase 3b CONSTRUITE + VALIDÉE** : fan-out per-task (`sub_workflow_foreach_file`, executor foreach séquentiel worktree partagé, `current_task.json`, cap 30, envelope foreach, cancel inter-items) + variant endpoint `decomposed:true` + enfant per-task. Tests 17+5 + régression 3078 ✓.
3. **4 bugs trouvés & fixés en live** : (a) `[SIGNAL: MISSING]` inline avalé par l'envelope → **exit 3 / `exit_3`** (et la boucle a ensuite fonctionné en vrai au run 4) ; (b) **PARTIAL tuait le run** (12/13 → Failed, pas de pr_draft) → PARTIAL=Success-branchable ; (c) **marker-id mismatch** → id EXACT + feedback `{{steps.completeness_check.output}}` au re-run (résultat : 16/16 au run 5) ; (d) deadlock infra de shells `pgrep` auto-matchants → tués, leçon notée.
4. 2 runs de décompo complets (4 & 5) + baseline (3) = la comparaison chiffrée ci-dessus.

**Roadmap restante (par ordre de valeur) :**
- **mechanical→Exec/template** (le flip économique, 12/16 items candidats) · **tier routing** par `complexity` · désagent **pr_draft→Exec** (supprime aussi le préambule résiduel) · port du foreach au **preset frontend** (aujourd'hui endpoint-only) · UI : afficher les N enfants d'un foreach dans RunDetail (le drill montre le dernier) · parallélisme par vagues `depends_on` (worktrees isolés + merge, conçu dans recursive-subworkflows.md §P2).

## 🔍 Revue critique post-runs — points faibles de la logique (2026-06-12)

**Structurels :**
1. ⭐ **Frontière de confiance non validée** : l'envelope triage est TypedSchema-validée, mais les 4 fichiers `.kronn/` écrits par l'agent ne le sont pas — le Gate approuve l'envelope, le fan-out exécute `tasks.json` (divergence possible). **Fix : Kronn dérive les fichiers machine depuis l'envelope validée** (déterministe, 0 token) ; l'agent n'écrit que le manifest.md lisible. Supprime le gap + raccourcit le triage.
2. **`last_output` foreach = sortie du commit du dernier enfant** (bruit) — pr_draft a dû ré-explorer. Fix : agréger les drift des enfants dans l'envelope OU drift_check global au parent post-foreach.
3. **Pas de resume du fan-out** : restart à l'item 9/16 → tout re-roule. Fix : skip déterministe des items déjà commités (détectables via les commits per-task).
4. **Variance du triage** : 13 vs 16 items sur le même ticket — granularité non reproductible (limite LLM-as-planner ; atténuer via recon Exec + critères de découpe stricts).

**Moyens :** 5. messages de commit génériques (lire current_task.json → `[<id>]: <what>` — quick win) · 6. `git add -A` commite le hors-scope (scope_check advisory) · 7. PARTIAL=Success peut masquer des échecs → surfacer en UI/notify · 8. items de tasks.json non validés (id/scope requis) · 9. `depends_on` non vérifié.

**Contextuels :** 10. run_tests ×N avec vraies deps = brutal (tests scopés + symlink deps) · 11. 30 items × 2-4 min vs guard 120 min non géré · 12. cancel parent ne tue pas l'enfant en cours (seulement entre items).

**Top 3 d'attaque : #1 (fichiers dérivés par Kronn) · #5 (commit messages) · #2 (drift au parent).**

### Re-priorisation après le contrat d'état partagé (2026-06-12, décision user)
Per-task lit désormais **`decisions.md` d'office** (état courant) + **manifest opt-in** (état initial) — livré+testé. Conséquences sur la liste :
- **#1 renforcé** (les fichiers sont devenus load-bearing pour les tâches suivantes) et **absorbe #8** (Kronn-dérivé = forme garantie par construction).
- **#9 (`depends_on`) monte** : l'ordre porte du sens maintenant que N lit l'état de 1..N-1.
- **#3 (resume) facilité** : le journal donne le contexte des tentatives précédentes au re-run.
- **#2 adouci** : pr_draft lit déjà decisions.md (le drift parent reste utile pour la table verbatim).
- **🆕 #13 — bloat/contamination de decisions.md** : il croît sur N tâches et chaque enfant le lit → coût token linéaire + risque qu'une entrée confuse pollue les suivantes. Parade livrée : discipline d'écriture (1-3 lignes, préfixe `[<task id>]` attribuable). Parade future : cap déterministe / rotation.

**Top 3 mis à jour : #1 (Kronn-derived, absorbe #8) · #5 (commit messages [id]) · #9 (validation depends_on).**

## 🏁 RUN 6 (batch fixes : engine-derived + drift parent + [id] commits + état partagé) — ✅ 19/19 PARFAIT

Le run de validation du lot « critiques + git + quick wins » (demande user 2026-06-12) :
- **Fix #1 vérifié live** : logs runner `Triage machine file derived` ×3 — les fichiers machine sont écrits par **Kronn depuis l'envelope validée** ; tasks.json 19 items, **topo-sort sans violation** (7 items `depends_on`), decision_ids = decided+mocked only. Le gap de confiance est fermé.
- **19/19 enfants Success, 0 échec** — envelope `{foreach, 19, 19, 0}`.
- **18 commits `[task-id]` + body = fichiers** (ex. `[webpack-brand-entries]` / body `webpack.config.js`) — l'historique de branche est auto-documenté. Branche `ahead: 19`.
- **drift_check parent** exécuté sur le worktree final (7 marqueurs) → pr_draft l'embarque.
- **pr_draft commence par `## Ticket`** (préambule enfin éradiqué), 13,5 Ko, section « Failed sub-tasks » correctement OMISE (zéro échec).
- Totaux : **147 876 tokens** · triage 743 s · fan-out 2 388 s · pr_draft 129 s.

## 📊 COMPARAISON FINALE 4 RUNS (EW-7247)

| | Run 3 monolithe | Run 4 décomp v1 | Run 5 décomp v2 | **Run 6 décomp v3 (lot complet)** |
|---|---|---|---|---|
| Verdict | ✅ | ⚠️ 12/13 | ✅ 16/16 | ✅ **19/19** |
| Tokens | **72 181** | 132 223 | 157 918 | 147 876 |
| Fan-out | 728 s (1 agent) | 1 825 s | 2 387 s | 2 388 s (19 items) |
| Commits | 1 global | 12 génériques | 15 génériques | **18 × `[id]`+fichiers** |
| Intégrité fichiers | agent (non validé) | agent | agent | **engine-derived ✓ topo ✓** |
| Drift pour PR | envelope (ok) | bruité | bruité | **parent, worktree final ✓** |
| pr_draft propre | ⚠ préambule | — (pas exécuté) | ⚠ 1 phrase | **✓ `## Ticket` direct** |
| Fiabilité | — | 1 échec catché | 0 | 0 (19 tâches vérifiées) |

**Conclusion** : la mécanique décomposée est maintenant **fiable et traçable de bout en bout** (intégrité par construction, historique git lisible, vérifs déterministes). Le surcoût tokens (~2×) reste le prix du séquentiel naïf — le flip = **mechanical→Exec** (6-12 items/run candidats) + **tier routing**, prochaine étape route.

## ⚡ RUN 7 — flip économique (mechanical→engine + tier routing) : mécanique ✓, économie nuancée

**Mécanique validée live** : 15/15 Success · **3 items `MechanicalApplied` à 0 token** (commits `— mechanical (engine-applied)`, instantanés, garde-fous chemins+marqueur OK) · tier routing actif (10 low→Economy, 2 high→Reasoning) · **fan-out 27 % plus rapide** (1 753 s vs 2 388 s run 6).

**Économie : 180 421 tk > run 6 (147 876)** — analyse de la distribution :
- triage **41 415 tk** (le plus cher à date) : il paie maintenant la **planification des contenus** (`files[]` inline) — coût déplacé, pas créé, mais payé même pour ce qui reste agent.
- 12 enfants agents = 134 525 tk dont **UN outlier à 45 523 tk** (`apollo-channel-dynamic` — l'item bundlait 4+ fichiers API ; en run 6 ce périmètre était éclaté en plusieurs items) ; les 11 autres ≈ 8-13 k.
- Seulement **3/15 items** éligibles engine-appliable sous la définition stricte (créations de petits fichiers) — l'économie (~24 k) a été absorbée par l'inflation triage + l'outlier.

**Verdict honnête** : (1) les mécanismes du flip marchent et font gagner du **temps** ; (2) sur UN run, l'économie tokens est **dominée par la variance du triage** (13/16/19/15 items selon les runs, granularité fluctuante) ; (3) le vrai levier suivant = **discipline de granularité** (un item comme apollo-channel-dynamic devrait être éclaté — règle anti-outlier au triage) et **A/B sur manifeste identique** (re-run du même tasks.json gelé) pour mesurer proprement. Comparer des runs à manifestes différents = comparer des tickets différents.

| | Run 3 mono | Run 4 | Run 5 | Run 6 | **Run 7 (flip)** |
|---|---|---|---|---|---|
| Verdict | ✅ | ⚠️ 12/13 | ✅ 16/16 | ✅ 19/19 | ✅ **15/15** |
| Tokens | 72 181 | 132 223 | 157 918 | 147 876 | 180 421* |
| Fan-out | 728 s | 1 825 s | 2 387 s | 2 388 s | **1 753 s** |
| 0-token items | — | — | — | — | **3 (engine)** |
| Items | 1 | 13 | 16 | 19 | 15 (*1 outlier 45 k) |

**Roadmap affinée** : règle anti-outlier au triage (scope > N fichiers ⇒ éclater) · A/B manifeste gelé (re-run depuis tasks.json existant) · augmenter le taux d'items engine-appliables (templates d'édition, pas seulement création) · vérif que le tier Economy s'applique bien au niveau CLI (à instrumenter).

## 🧠 Améliorations système (principe : tâche simple = + efficace + + déterministe + - tokens)

Principe user validé, AVEC la nuance « taxe de contexte » (chaque agent paie le chargement contexte au démarrage → découper par COHÉSION, pas à l'infini ; le gain vient du scoping, pas du nombre de splits).

1. **⭐ Scoper le contexte par sous-tâche** : chaque sous-WF reçoit sa SEULE tranche de manifeste (1 decision_id) + ses SEULS files_touched + l'evidence linked-repo pertinente. Le vrai levier tokens.
2. **⭐ Model tier par sous-tâche** : la décompo débloque le routage modèle. Tâches mécaniques → tier economy/cheap (Haiku) ; logique complexe → tier fort. `agent_settings.tier` existe. Plus gros gain €/token.
3. **Flag `mechanical: bool` au triage** : items déterminés (config/enum/valeurs liftées) → génération Exec/template (0 token) ; le reste → agent. Désagent au cas par cas, décidée par le triage qui raisonne déjà.
4. **Maximiser la précision du triage** → implement devient quasi-transcription (moins de raisonnement = moins de tokens + plus déterministe). « Raisonner une fois, exécuter cheap N fois ».
5. **Vérif déterministe entre steps agent** (completeness-check vs manifeste, lint, typecheck) : attrape hallu/skip à 0 token, évite les re-runs agent.
6. **Retry scopé (fail-fast)** : décompo fine → seule la sous-tâche cassée re-tourne, pas tout le pipeline. Gain sur le chemin malheureux.
7. **Garde-fou cohérence** : N agents → risque d'incohérence (naming…). Parade = manifeste + conventions comme contrat partagé + check de cohérence final déterministe (compile/lint global).

## 🧠🧠 RUN 8 — two-brains (plan Reasoning + reviewer adversarial + débat) : ✅ 19/19, qualité ↑↑, coût ↑↑

**Pipeline validé live de bout en bout** : triage (tier Reasoning épinglé) → plan_lint (Exec 0 tk) → plan_review adversarial → 2× `NEEDS_RETRIAGE` (findings réels via `.kronn/plan-review.md`, dont un trou de couverture SCSS et la granularité 13→19 items) → `PLAN_APPROVED` au round 3 → gate humain (lint + summary review embarqués) → fan-out 19 items.

**Résultat fan-out** : **19/19, 0 échec** — 15 Success agents + **4 MechanicalApplied à 0 token**. 19 commits `[task-id]` impeccables (4 `— mechanical (engine-applied)`). drift_check parent 60 ms. pr_draft direct `## Ticket`, cite les marqueurs et liste **7 dépendances cross-team bloquées** explicitement.

**Tokens : 376 110 — 2,5× run 6.** Distribution :
- **Débat pré-implémentation = 177 197 tk** (47 %) : 3× triage (44,9 k + 47,2 k + 37,2 k = 129,3 k) + 3× plan_review (12,2 k + 19,7 k + 15,9 k = 47,9 k). Chaque `NEEDS_RETRIAGE` re-paie un triage COMPLET (~40-47 k) — c'est la régénération intégrale qui coûte, pas la review elle-même.
- Fan-out 191 307 tk pour 15 items agents (~12,7 k/item, dans la norme run 7) + 4 items gratuits.
- pr_draft 7 606 tk.

| | Run 6 | Run 7 (flip) | **Run 8 (two-brains)** |
|---|---|---|---|
| Verdict | ✅ 19/19 | ✅ 15/15 | ✅ **19/19** |
| Tokens | 147 876 | 180 421 | **376 110** |
| Pré-implémentation | ~40 k (1 triage) | 41 k | **177 k (3 rounds)** |
| 0-token items | 0 | 3 | **4** |
| Qualité plan | bon | 1 outlier 45 k | **0 outlier, gaps catchés avant code** |

**Verdict honnête** : le débat améliore RÉELLEMENT le plan (zéro outlier, zéro échec, gaps attrapés avant d'écrire du code — sur un vrai gros ticket c'est un re-run d'item évité). Mais à 3 rounds le surcoût (+177 k) dépasse ce qu'un re-run d'item raté coûterait (~13 k). **Recos** : (1) **cap reviewer à 1 round par défaut** (2 = opt-in « ticket critique ») ; (2) **re-triage incrémental** — le reviewer pointe des items précis, le triage ne devrait régénérer QUE ces items (merge engine-side), pas tout le manifeste ; (3) le **casting** (déployé post-run 8 : plan=Claude Reasoning, review=Codex, fan-out=Sonnet) doit faire baisser les 3 postes à la fois — à mesurer au run 9.

**Casting livré (post-run 8, non encore testé live)** : `reviewer_agent` param (défaut Codex) · triage épinglé ModelTier::Reasoning · fan-out tier routing (low→Economy) · **run_tests v2 au parent** : double suite **JS (yarn/npm) + PHP (phpunit)**, symlink node_modules+vendor depuis le checkout principal, verdict `TEST VERDICT — JS: x | PHP: y` cité verbatim dans la PR, exit 0 toujours (les échecs sont documentés, pas bloquants) · **static_checks enfant** (php -l + JSON sur fichiers changés, exit 2 → re-implement max 2). ⚠️ Prérequis JS : `yarn install` dans le checkout principal de front_euronews (node_modules absent → JS: SKIPPED).

## ✅ RUN 11b — pipeline COMPLET propre de bout en bout (casting opus/Codex/sonnet/haiku)

Premier run qui traverse **tout** le pipeline sans intervention manuelle ratée, après une session de durcissement (8 correctifs). EW-7247 décomposé, `decomposed:true`.

**Casting effectif** (fable ayant perdu l'accès compte Claude en cours de session → recâblé) : triage **opus** (reasoning) · plan_review **Codex gpt-5.5** · implement **sonnet** (default) avec **tier routing par complexité** (low→haiku, med→sonnet, high→opus) · steps mécaniques 0-token.

**Déroulé** : triage opus 46,9k → review Codex 191,6k → `NEEDS_RETRIAGE` → re-triage **incrémental** (34,3k, 14 items hydratés via `unchanged[]`) → review 135,5k → cap 1 épuisé → **gate humaine APPROVE** → fan-out **20/20 Success, 0 échec** (17 agents + 3 MechanicalApplied 0-tk, 20 commits `[id]`) → run_tests v3 → drift → **pr_draft Success** (`## Ticket` direct, CLEAR/DECIDED + marqueurs KRONN + tags `[MechanicalApplied]`).

| Step | Statut | Durée | Tokens |
|---|---|---|---|
| triage (r1) | ✅ | 549s | 46 882 |
| plan_review (r1) | ✅ | 285s | 191 582 |
| triage (r2 incrémental) | ✅ | 370s | 34 345 |
| plan_review (r2) | ✅ | 266s | 135 470 |
| review_triage (gate) | ✅ APPROVE | — | 0 |
| feasibility_impl (fan-out 20) | ✅ 20/20 | 5461s | 336 336 |
| run_tests v3 | ✅ | 270s | 0 |
| drift_check | ✅ | 0s | 0 |
| pr_draft | ✅ | 156s | 7 411 |
| **TOTAL** | **✅ Success** | ~3h | **752 026** |

**run_tests v3 validé live** : JS = **PASS** (3400 tests, jest dans le container Kronn ; le fix « couverture qui dippe ≠ échec de test » élimine le faux FAIL du run-10) · PHP = la suite a **réellement tourné** dans le service php **dockerisé du projet** (`docker compose run --rm -v <worktree>/application:/app -v <main>/vendor:/app/vendor php vendor/bin/phpunit`, 3625 tests exécutés) — **176 failures + 83 errors RÉELS** introduits par la migration (premier passage sur un Epic énorme → à reprendre, c'est le signal honnête attendu). NB : ce run a tagué PHP « ERROR(harness) » par un match de substring trop large ; **classification corrigée après coup** (un résumé `Tests: N` ⇒ la suite a tourné ⇒ FAIL réel, pas harness).

**Coût reviewer = poste n°1** : plan_review 191k + 135k = **327k / 752k (43%)**. Codex gpt-5.5 en reasoning relit tout à chaque round. Reco prioritaire : capper l'effort de raisonnement du reviewer ou le passer en tier économique (la review trouve les vrais trous même sans reasoning max).

**Comparaison runs décomposés** :
| | Run 6 | Run 8 (2-brains) | Run 10 (gate loop) | **Run 11b (clean)** |
|---|---|---|---|---|
| Verdict | 19/19 | 19/19 | 17/21 (rate-limit) | **20/20, 0 échec** |
| Tokens | 147k | 376k | 871k | 752k |
| pr_draft | ✅ | ✅ | ❌ rate-limit | **✅** |
| Tests réels | — | — | JS only | **JS PASS + PHP exécuté (Docker)** |
