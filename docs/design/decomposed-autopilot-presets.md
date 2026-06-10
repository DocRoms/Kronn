# Spec de conception — Templates Workflows décomposés via SubWorkflow + maj skill Architect

> Statut : **CONCEPTION (avant code)**. Suite directe de [`recursive-subworkflows.md`](./recursive-subworkflows.md) (feature `StepType::SubWorkflow` livrée Phases 1a→1c, 2026-06-11). Tout est ancré `file:line` sur le code réel.
>
> Décision produit (2026-06-11) : on vise l'**option 3** (la plus complète) = un preset peut créer *N* workflows (enfant(s) + parent qui les référence), et les presets Autopilot existants sont réécrits pour que la boucle `implement↔test↔review` devienne un **sous-workflow**. L'option 3 **contient** l'option 2 (la brique « preset → N workflows » est commune).

## 1. Objectif

Rendre `SubWorkflow` *réellement utilisable* par les deux chemins de création :

1. **Chemin IA** (skill `workflow-architect`) — le skill décrit aujourd'hui « **eight step types** » et ignore `SubWorkflow`. Il doit apprendre : quand l'utiliser, le schéma, les signaux, les contraintes, **et** un *protocole de composition* (créer l'enfant en draft d'abord, puis le parent qui le référence).
2. **Chemin presets** (`v07-presets.ts` + endpoint `feasibility-autopilot`) — un preset produit aujourd'hui **UN** workflow auto-portant. Un step `SubWorkflow` pointe vers un `sub_workflow_id` d'un workflow **déjà persisté**. Il faut donc une instanciation **multi-workflow** : créer l'enfant, récupérer son id, patcher le `sub_workflow_id` du parent, créer le parent.

## 2. Verdict de faisabilité (sourcé)

Faisable, **sans réécrire le moteur** — mais trois invariants du code dictent la découpe (vérifiés, pas supposés) :

### INV-1 — `Goto` ne traverse jamais la frontière parent/enfant
`validate_sub_workflow_graph` + check dangling-target (`api/workflows.rs:328`) valident les cibles `Goto` **dans le même workflow**. Une arête `on_result.Goto` ne peut donc cibler que des steps du *même* graphe. Conséquence directe sur la découpe des deux presets (cf §4).

### INV-2 — `agent_decisions` s'attache au `run_id` de l'`execute_run` courant
L'`upsert` du manifeste triage se fait dans la boucle de step (`runner.rs:1108`), keyé sur le `run_id` de l'execute_run en cours. Si le step `triage` part dans l'enfant, ses décisions s'attachent au **run enfant** → visibles via `GET /api/agent-decisions?project_id=…` mais **plus** via le `run_id` du parent.

### INV-4 — ⚠️ PAS de continuité de worktree enfant→parent (Phase 2 manquante)
**Découvert en attaquant PR-B (2026-06-11).** Chaque run a un worktree git **isolé sur sa propre branche** (`workspace.rs`). Le run enfant implémente dans SON worktree ; le step `SubWorkflow` ne ramène que des métadonnées (`{child_run_id, child_workflow_id, child_status, child_steps}`, `sub_workflow_step.rs:161`) — **pas** la branche produite, et le parent n'est pas dessus. La branche enfant est « préservée » (`produced_branches`) dans le repo mais inexploitable telle quelle par un step parent. Le handoff/merge enfant→parent est explicitement **Phase 2** (`sub_workflow_step.rs:99` « Sharing/merge is Phase 2 »).

**Conséquence dure** : tout step parent qui doit LIRE le travail code de l'enfant (typiquement `create_pr`) est **bloqué** tant que Phase 2 n'existe pas. Un step qui touche le worktree doit vivre **dans le même run** que ceux qui l'ont produit → soit tout dans l'enfant, soit tout dans le parent. On ne peut pas implémenter dans l'enfant et créer la PR dans le parent.

### INV-3 — `project_id` doit être partagé parent→enfant
L'injection `linked_repos` + univers-projets se fait **une fois au démarrage de chaque `execute_run`** (`runner.rs:203`), keyée sur le projet du workflow. L'addendum `[TRIAGE]` (`triage::is_triage_step`, `big_ticket_template.rs:223`) et les MCP projet suivent la même logique per-run. → l'enfant créé par un preset **doit hériter du `project_id` du parent**, sinon la boucle perd linked_repos + MCP + addendum.

> Les fondations sous-WF (budget partagé `SharedBudget`, profondeur `MAX_SUBWORKFLOW_DEPTH=5`, cycle-check au save + runtime, `child_run_id`, signal `SUBWF_FAILED`) sont **déjà livrées** (`sub_workflow_step.rs`). Ce spec ne touche QUE l'instanciation des presets + la doc Architect + la réécriture des graphes de 2 presets.

## 3. Brique commune — instanciation preset → N workflows

Aujourd'hui (`WorkflowWizard.tsx:301`, `buildV07Presets`) : un preset est un *prefill* client → 1 `POST /api/workflows`. Pour le multi-workflow, **réutiliser le mécanisme `bundle` qui existe déjà** plutôt que d'en inventer un :

- `POST /api/workflows/bundle` crée déjà *atomiquement* (transaction, rollback) un workflow + ses QP/QA/Custom-APIs, avec substitution de sentinelles `@bundle:<id>` dans les champs du workflow (`workflow-architect.md` §B).
- **Extension proposée** : ajouter une catégorie `child_workflows[]` au bundle, et reconnaître `@bundle:<id>` au point de substitution **`sub_workflow_id`**. Le serveur crée d'abord les `child_workflows` (héritant du `project_id` du parent — INV-3), récupère leurs ids, puis substitue dans le step `SubWorkflow` du parent.
- **Ordre de création + cycle** : enfants avant parent ; le `validate_sub_workflow_graph_db` (`api/workflows.rs:382`) tourne déjà sur chaque create → re-vérifié naturellement post-substitution.
- **Côté preset** : un preset « décomposé » déclare `child_workflows` + un parent avec `sub_workflow_id: '@bundle:implement-verify'`. Le wizard route ces presets vers `/bundle` au lieu de `/workflows`.

> Décision : **réutiliser `bundle`, ne pas créer un 2ᵉ chemin d'instanciation.** Le `feasibility-autopilot` (endpoint backend dédié `api/workflows.rs:864`) bascule lui aussi sur la création multi-workflow interne.

## 4. Réécriture des graphes (le cœur du risque)

### 4.1 `ticket-to-pr` (PR-B, faible risque)

Aujourd'hui (9 steps, `v07-presets.ts:495`) :
`fetch_issue → analyze → plan_gate → implement → run_tests → review → ready_gate → create_pr → notify_done`

Découpe :
- **Parent** : `fetch_issue → analyze → plan_gate → [SubWorkflow: implement-verify] → ready_gate → create_pr → notify_done`
- **Enfant `implement-verify`** : `implement ↔ run_tests ↔ review` avec les Gotos **internes** (`run_tests ERROR → Goto implement`, `review NEEDS_CHANGES → Goto implement`) — **aucun Gate dedans** (✓ conforme à la contrainte MVP).

Adaptation imposée par **INV-1** : `ready_gate.gate_request_changes_target` ne peut plus valoir `implement` (qui vit dans l'enfant). Il ciblera le **step sous-workflow** → re-run complet du child (qui ré-entre dans sa boucle). Sémantique acceptable : « renvoyer pour corrections » = relancer la boucle implement/test/review.

Branche d'échec : l'enfant qui épuise ses Gotos termine `Failed` → le step SubWorkflow émet `SUBWF_FAILED` → `on_result` parent : `contains SUBWF_FAILED → Goto [le step subworkflow]` (max 1-2) ou fall-through vers `on_failure`.

### 4.2 `feasibility-autopilot` (PR-C, risque réel — réécriture)

Aujourd'hui (7 steps, endpoint backend) :
`fetch_issue → triage(TypedSchema Fail) → review_triage(Gate) → implement → run_tests → drift_check → pr_draft`
avec `implement.on_result: BLOCKED → Goto triage`.

Le couplage `implement → Goto triage` **traverse** la frontière si triage reste parent et implement part enfant (**INV-1** casse). Deux options de découpe :

- **Option C1 (recommandée)** : triage **reste au parent** (read-only, produit le manifeste), `review_triage` Gate **reste au parent**, puis **enfant** = `implement → run_tests → drift_check` (boucle interne `run_tests ERROR → Goto implement`). Le `BLOCKED` mid-implementation ne peut plus `Goto triage` (cross-boundary) → il devient `[SIGNAL: BLOCKED]` → l'enfant `Stop`/`Failed` → `SUBWF_FAILED` remonte → `on_result` **parent** : `contains SUBWF_FAILED → Goto triage` (re-triage avec le contexte du blocage). On reconstruit la sémantique « bloqué → re-triage » **au niveau parent**, ce qui est plus propre (le re-triage repasse par le Gate humain).
  - **INV-2** : triage reste parent → `agent_decisions` continuent de s'attacher au run parent. ✓ Pas de régression sur `GET /api/agent-decisions?run_id=<parent>`.
  - **INV-3** : l'enfant hérite du `project_id` → addendum/linked_repos OK pour `implement`. (Le `[TRIAGE]` addendum reste sur le triage parent. ✓)
  - `pr_draft` **reste parent** (lit `{{steps.<subwf>.…}}` + drift via l'enfant — voir §5).

- **Option C2 (rejetée)** : tout (triage+implement+test+drift) dans l'enfant, seul le Gate au parent. Rejetée : le Gate humain doit voir le manifeste triage *avant* implement, donc le Gate doit être **après** triage et **avant** implement → si triage est dans l'enfant, le Gate parent ne peut s'intercaler au milieu de l'enfant (pas de Gate cross-boundary, et un Gate dans l'enfant est interdit en MVP). Incohérent.

> **Décision : C1.** Le Gate et le triage restent au parent ; seul le bloc d'exécution (implement/test/drift) descend en sous-workflow. Le `BLOCKED → re-triage` se reconstruit via `SUBWF_FAILED` au parent.

## 5. Passage de données parent ↔ enfant (deux gaps confirmés dans le code)

L'enfant a son propre `TemplateContext` (nouvel `execute_run`). Le parent ne lit PAS `{{steps.implement.…}}` de l'enfant directement. **État Phase 1 vérifié** :

- **Entrée parent → enfant : INEXISTANTE aujourd'hui.** Le child run est construit avec `state: Default::default()` et `trigger_context: Some({ "__subwf_depth__": … })` uniquement (`sub_workflow_step.rs:90,109`). → aucun moyen de passer `{{steps.triage.data}}` (parent) à `implement` (enfant). **Extension PR-C** : ajouter un champ optionnel `sub_workflow_inputs: Map<String,String>` au step (templates rendus contre le ctx parent), seedé dans le `trigger_context` / `state` du child avant `execute_run`. ~petite extension localisée à `sub_workflow_step.rs`.
- **Sortie enfant → parent : MÉTADONNÉES SEULEMENT aujourd'hui.** L'envelope du step expose `data = { child_run_id, child_workflow_id, child_status, child_steps }` (`sub_workflow_step.rs:161-166`) — **pas** l'`output` du dernier step de l'enfant. → `create_pr` / `pr_draft` (parent) ne peuvent PAS lire le verdict de tests ni le drift via `{{steps.<subwf>.data}}`. **Extension PR-C** : enrichir `data` avec le `output`/`summary` du dernier step (ou d'un step nommé) du child run — l'info est déjà dans `child_run.step_results`, il suffit de la projeter dans l'envelope.

> Ces deux gaps sont **petits et localisés** (`sub_workflow_step.rs` uniquement, pas de changement de moteur). Ils sont le vrai prérequis technique de PR-C — à livrer dans PR-C ou en mini-PR dédiée juste avant. **Fallback sans extension** : `drift`/`pr_draft` parent relisent le manifeste triage depuis `agent_decisions` (INV-2, déjà persisté) plutôt que via l'envelope enfant — possible mais ne couvre pas le verdict de tests.

## 6. Ripple skill `workflow-architect.md` (PR-A, inconditionnel)

Points à éditer (le skill dit « eight step types », `:18`) :
1. Intro : « eight » → « nine ».
2. **Nouvelle §9 `SubWorkflow`** : rôle (« ce step EST un pipeline réutilisable »), JSON exemple (`{ step_type:{type:"SubWorkflow"}, sub_workflow_id:"…" }`), coût (= coût du run enfant, compté dans le `SharedBudget` de l'arbre).
3. **Arbre de décision** : nouvelle entrée « est-ce un bloc réutilisable / un workflow existant fait déjà ce morceau ? → SubWorkflow (référence-le plutôt que de recopier ses steps) ».
4. **Schéma** : ajouter `SubWorkflow` à l'enum `step_type` + table « Fields specific to SubWorkflow » (`sub_workflow_id` requis).
5. **Table des signaux** : `SubWorkflow` émet `OK` (enfant Success) / `SUBWF_FAILED` (sinon) → branchable via `on_result`.
6. **Contraintes / Gotchas** : pas de `Gate` dans un sous-WF (MVP), profondeur ≤ 5, pas de cycle (vérifié au save), `Goto` interne au child uniquement (INV-1), l'enfant hérite du `project_id` (INV-3).
7. **Protocole de composition** (nouveau) : pour créer un parent+enfant via le chemin IA → `workflow_create_draft` l'enfant d'abord (récupérer son id), puis le parent référençant cet id ; OU bundle `child_workflows[]` + `@bundle:` sur `sub_workflow_id`.
8. **§ Feasibility-Gated** : documenter la variante décomposée (C1) une fois PR-C livrée.
9. **Validation** : `sub_workflow_id` non vide sur tout step SubWorkflow.

## 6bis. Phase 2 — handoff de worktree (décidé : option α, 2026-06-11)

INV-4 impose de construire le handoff AVANT de décomposer `ticket-to-pr`. **Design retenu : l'enfant PARTAGE le worktree du parent** (le plus simple et correct pour un SubWorkflow séquentiel — le parent est en pause `await` pendant le run enfant, zéro accès concurrent).

Mécanique (ancrée code) :
- `execute_run` gagne un param `inherited_workspace: Option<String>`. Quand `Some(path)` : **attach** (réutilise `Workspace::attach`, déjà là pour le resume) au lieu de `create` ; l'enfant commit sur la branche du parent.
- L'enfant inherited **skip `before_run`** (déjà setup par le parent) et **skip cleanup + `produced_branches`** (`cleanup(self)` est explicite, pas de `Drop` destructeur → ne pas l'appeler suffit ; le parent possède le cycle de vie du worktree).
- Le dispatch SubWorkflow passe `run.workspace_path` du parent → `execute_sub_workflow_step(parent_workspace)` → `execute_run(inherited_workspace)`.
- Les 4 autres appelants d'`execute_run` passent `None` (run top-level = worktree propre).
- Prérequis INV-3 : l'enfant a le même `project_id` que le parent → `repo_path` résout le même repo → `attach` valide.

Résultat : implement (enfant) et create_pr (parent) tournent dans **le même worktree, sur la même branche** → la PR contient le travail de l'enfant, et le `ready_gate` pré-PR du parent est **conservé**. Décompo 100% fidèle.

## 6ter. Canal de données parent↔enfant — convention `plan.md` / `decisions.md` (décidé 2026-06-11)

INV-4 (handoff worktree) une fois construit, le worktree partagé **EST** le canal de données — les agents lisent/écrivent des fichiers, sans champ `sub_workflow_inputs` ni plomberie artifacts. Convention retenue (idée user) :

- **`.kronn/plan.md`** — écrit par `analyze` (parent), l'intention initiale validée (figée après `plan_gate`). Lecture seule ensuite.
- **`.kronn/decisions.md`** — journal append-only que **tout step** complète : déviations, mocks, blocages + **le pourquoi**. `implement` y écrit ses écarts ; `review` y lit + ajoute findings ; `create_pr` embarque les deux dans le corps de PR.

Effet : tout agent (parent ET enfant) voit en continu le **delta intention↔réalité**. Cousin lisible des marqueurs `KRONN-ASSUMED/MOCKED/TODO` du Feasibility-Gated.

**Ça résout les deux directions du §5 SANS code moteur supplémentaire** : entrée parent→enfant = l'enfant lit `plan.md` ; sortie enfant→parent = `create_pr` lit `decisions.md` (+ l'envelope enrichie `data.last_output` pour le verdict). → **les DEUX gates conservés (fidélité 100%)**. Le `sub_workflow_inputs` (OQ#1) devient inutile pour cette famille de presets.

## 6quater. Feature connexe (hors PR-B) — board global « en cours / pas fini »

Décidé 2026-06-11 : un agent doit pouvoir consulter ce qui tourne *ailleurs* (autres runs non terminés). **Choix user : tool MCP, pas fichier** — exposer/améliorer `workflow_list(status=running)` (kronn-internal) plutôt qu'un `.kronn/history.md` (donnée DB fiable, jamais obsolète ; inconvénient : pas lisible hors-Kronn). À spec/chiffrer **séparément** ; ne pas fondre dans PR-B. NB : `plan.md`/`decisions.md` restent **non scopés par run** (chemin stable à travers l'arbre parent+enfant ; l'isolation worktree sépare déjà les runs concurrents — scoper `.kronn/<run>/` casserait le canal car parent et enfant ont des run_id distincts).

## 7. Phasage

| PR | Contenu | Risque | Tests |
|----|---------|--------|-------|
| **PR-A** | Infra bundle `child_workflows[]` + substitution `@bundle:` sur `sub_workflow_id` (héritage `project_id`) · maj complète skill `workflow-architect.md` · wizard route les presets décomposés vers `/bundle` | Moyen (backend bundle + front routing) | bundle multi-WF (enfant créé + id substitué + cycle re-checké) ; pin skill « nine step types » |
| **PR-B** | Preset `ticket-to-pr` décomposé (parent + enfant `implement-verify`) · `ready_gate` cible le step SubWorkflow | Faible | instanciation crée 2 WF liés ; ready_gate request_changes re-run child ; SUBWF_FAILED → on_failure |
| **PR-C** ✅ LIVRÉ (code) | `feasibility-autopilot` décomposé (C1) : parent triage+gate+pr_draft, enfant implement+test+drift. Manifeste passé via `.kronn/triage-manifest.md` (worktree partagé, pas de champ `sub_workflow_inputs`). Endpoint crée enfant→parent. `BLOCKED→Goto(triage)` reconstruit en `SUBWF_FAILED→Goto(triage)` parent ; blocage mid-implement = tracé (KRONN-TODO+decisions.md). | Élevé | front 30 ✓ + back 16 ✓ + suites complètes ✓. **Reste : E2E live EW-7247 post-déploiement** (seul moyen de prouver bout-en-bout) |
| **Board MCP** ✅ LIVRÉ | tool `workflow_active_runs` (kronn-internal) — runs in-flight (Running/WaitingApproval/Pending) sur tous les workflows. Pure lecture `GET /api/workflows`, drill via `workflow_run_status`. | Faible | 3 tests python ✓ |

## 8. Décisions figées

- **D1** — Réutiliser `bundle` (catégorie `child_workflows[]` + sentinelle `@bundle:` sur `sub_workflow_id`), pas de 2ᵉ chemin d'instanciation.
- **D2** — L'enfant hérite **toujours** du `project_id` du parent (INV-3).
- **D3** — `ticket-to-pr` : `ready_gate.request_changes_target` = le step SubWorkflow (re-run child), pas `implement`.
- **D4** — `feasibility-autopilot` : découpe **C1** (triage + Gate au parent ; implement/test/drift en enfant ; `BLOCKED → re-triage` reconstruit via `SUBWF_FAILED` parent).
- **D5** — Skill Architect mis à jour en **PR-A** (inconditionnel), pas attendu PR-C.

## 9. Questions ouvertes restantes

Les deux gaps de §5 (entrée variables parent→enfant ; sortie enrichie enfant→parent) sont **tranchés** : confirmés inexistants en Phase 1, sizés en petites extensions de `sub_workflow_step.rs`, prérequis PR-C. Restent :

1. **Granularité de la sortie enrichie** : projeter le `output` du **dernier** step de l'enfant, ou permettre de nommer le step à exposer (ex. `run_tests` pour le verdict + `drift_check` pour les marqueurs) ? Le `pr_draft` parent a besoin des **deux**. Proposition : exposer `data.last_output` **et** `data.steps_outputs[<name>]` (map des outputs par nom de step du child). À valider en attaquant PR-C.
2. **`ticket-to-pr` `create_pr`** : confirme le besoin de la sortie enrichie (lit `{{steps.run_tests.…}}` aujourd'hui). Couvert par l'extension §5 — pas un blocage résiduel, juste à tester.
