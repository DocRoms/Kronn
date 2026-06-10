# Spec — AutoPilot fan-out : décomposition par sous-tâche (Phase 3)

> Statut : **CONCEPTION (avant code)**. Marie ce qui est LIVRÉ — autopilot décomposé (triage → 1 enfant `implement`, validé live sur EW-7247, cf [`ew7247-autopilot-live-audit.md`](./ew7247-autopilot-live-audit.md)) — avec le `BatchWorkflow` déjà conçu dans [`recursive-subworkflows.md`](./recursive-subworkflows.md) (fan-out N + merge séquentiel-rebase). Objectif : transformer l'`implement` monolithique en **N sous-WF, un par sous-tâche**, chacun scopé serré, avec tier modèle par tâche + vérif déterministe. Principe directeur (user) : **tâche plus simple = + efficace, + déterministe, - tokens** — avec la nuance « taxe de contexte » (découper par cohésion, pas à l'infini).

## 1. État des lieux
- **Livré** : `feasibility-autopilot` décomposé = parent `fetch → triage → gate → [feasibility_impl: 1 enfant implement→test→drift→commit] → pr_draft`. L'`implement` couvre TOUTES les sous-tâches en UN run agent (monolithe). Coûteux + moins déterministe sur un gros ticket.
- **Conçu (pas codé)** : `BatchWorkflow` (recursive-subworkflows.md §P2) — fan-out d'un sous-WF sur une liste, worktrees isolés, merge séquentiel-rebase, conflit → Gate. Schéma sous-tâche `{id, scope[], depends_on[], acceptance}`.
- **Phase 3 = brancher le fan-out sur le manifeste du triage** : le triage produit DÉJÀ la liste des sous-tâches → elle pilote le fan-out (décompo gratuite, aucun agent « planner »).

## 2. Le manifeste devient le work-list (enrichi)
Le triage émet aujourd'hui `{id, what, where, why/strategy/needed_from}` par item. On l'enrichit (toujours `TypedSchema`, validé) :
| Champ | Rôle |
|---|---|
| `id` | clé du sous-WF + du commit + des marqueurs KRONN |
| `scope[]` (fichiers/globs) | **contexte ET worktree de l'enfant** + anti-conflit (scopes disjoints = parallélisables ; chevauchement → sérialisé/averti) |
| `depends_on[]` | fan-out par **vagues topologiques** (les indépendants en parallèle) |
| `mechanical: bool` | **true → génération Exec/template (0 token)** ; false → agent |
| `complexity: low\|med\|high` | **→ tier modèle** : low→economy/cheap, med→default, high→reasoning |
| `acceptance` | critère de complétude (pour la vérif déterministe) |

→ Le triage (qui raisonne déjà, 1 passe chère) **pré-décide** tout ; l'exécution devient cheap + routée.

## 3. Sous-WF par sous-tâche (contexte scopé)
Chaque enfant reçoit SEULEMENT : sa **tranche de manifeste** (1 `id`, pas les N) + ses **`scope[]` fichiers** + l'**evidence linked-repo pertinente**. C'est LE levier tokens (#1). Shape :
- `mechanical:true` → **Exec/template** (génère le fichier depuis les valeurs liftées) — 0 token.
- `mechanical:false` → `implement` (tier = `complexity`) → `static_checks` (Exec lint/typecheck, 0 token, fail→implement) → `scope_check` (Exec) → `commit` (sur la branche de l'enfant).
- Pas de Gate dans l'enfant (règle MVP conservée).

## 4. Orchestration : `BatchWorkflow` sur les items du manifeste
- `BatchWorkflow` (réutilise `parse_items_as_objects` + le pattern Semaphore/wait de `batch_step.rs`) fan-out un sous-WF par item, **par vagues `depends_on`**.
- **Worktree** : ⚠️ point d'archi vs Phase 2. Le handoff Phase 2 (livré) = 1 enfant qui PARTAGE le worktree parent (séquentiel, parent en pause). N enfants parallèles **ne peuvent pas** partager un worktree (race). → fan-out = **worktree/branche isolé par enfant** + merge (conforme recursive-subworkflows.md §P2). Le shared-handoff reste pour le `SubWorkflow` simple ; le fan-out utilise l'isolé+merge.
- **Merge** : séquentiel-rebase dans le worktree parent, ordre = vagues puis index ; 1er conflit → `[SIGNAL: CONFLICT]` → Gate humain (réutilise `gate_checkpoint` 0.8.6) ; fallback agent-intégrateur. (Déjà décidé.)

## 5. Couche de vérification déterministe (0 token, entre/après les agents)
- **`static_checks`** (par enfant, avant tests) : `php -l` / `tsc --noEmit` / lint → fail → retour implement avec l'erreur exacte (moins d'itérations agent).
- **`scope_check`** (par enfant) : `git diff --name-only` vs `scope[]` de l'item → tout fichier hors-scope auto-loggé dans `.kronn/decisions.md` (le run-2 live a montré que le prompt seul ne contraint PAS le scope → déterministe obligatoire).
- **`completeness_check`** (global, avant merge) : les marqueurs `KRONN-*` trouvés (drift_check) vs les `id` du manifeste → un `id` sans marqueur = sous-tâche sautée → fail/relance ciblée. Vérifie sans agent que tout est couvert.
- **`coherence_check`** (global, après merge) : compile/lint de l'ensemble fusionné → attrape les incohérences inter-enfants (naming…).

## 6. Modèle de coût (le gain)
1 passe triage chère (raisonnement up-front, précis) → N implements **scopés + tier-matchés + certains mechanical=0-token** → vérif+merge déterministes. Gains cumulés : **scoping contexte** (#1) + **tier par tâche** (#2) + **mechanical→Exec** (#3) + **retry scopé** (seule la sous-tâche cassée re-tourne). L'agent ne tourne QUE pour la logique non-déterminée + la résolution de conflits.

## 7. Garde-fous / risques
- **Taxe de contexte** : ne pas sur-découper ; 1 sous-WF = 1 concern cohérent (le `scope[]` du triage). Sweet spot, pas micro-tâches.
- **Cohérence inter-enfants** : manifeste + conventions = contrat partagé + `coherence_check` final.
- **Worktrees isolés × N** : coût disque/CPU (worktree add). Cap de concurrence (Semaphore existant).
- **`depends_on` mal posé** → sérialisation excessive ou conflits ; le triage doit être discipliné (prompt + validation schéma).

## 8. Décisions à figer (avant code)
- **D1** — fan-out isolé-par-enfant + merge séquentiel-rebase (PAS le shared-handoff Phase 2, réservé au SubWorkflow simple).
- **D2** — manifeste enrichi `{scope[], depends_on[], mechanical, complexity, acceptance}`, émis en TypedSchema(Fail) par le triage.
- **D3** — tier modèle dérivé de `complexity` ; `mechanical:true` → Exec/template (0 agent).
- **D4** — couche vérif déterministe (static/scope/completeness/coherence) obligatoire (le live a prouvé que le prompt ne suffit pas pour le scope).

## 9. Phasage Phase 3
- **3a** — enrichir le triage (schéma + prompt : scope/depends_on/mechanical/complexity/acceptance) + le `scope_check` & `completeness_check` Exec sur l'autopilot ACTUEL (1 enfant) → gains immédiats sans le fan-out. *(faible risque, valide la couche vérif + l'enrichissement manifeste)*
- **3b** — `BatchWorkflow` fan-out + worktrees isolés + merge séquentiel-rebase + tier par item + mechanical→template. *(le gros morceau)*
- **3c** — E2E sur EW-7247 : comparer monolithe (run actuel) vs fan-out (tokens, déterminisme, qualité PR).

## 10. Questions ouvertes
1. `acceptance` par sous-tâche : texte libre (lu par completeness_check agent ?) ou prédicat déterministe (fichier existe + marqueur présent + lint OK) ? Préférence : **déterministe** pour rester 0-token.
2. Worktree isolé par enfant = N× `node_modules`/`vendor` ? → hook `after_create` symlink depuis le checkout principal (cf finding run_tests skip).
3. Sérialisation des scopes chevauchants : avertir au triage (validation) ou merger en fin de vague ?
