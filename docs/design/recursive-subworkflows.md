# Spec de conception — Sous-workflows récursifs (Kronn)

> Statut : **CONCEPTION (avant code)**. Issu d'un panel 5-lentilles (ré-entrance · modèle d'état · fan-out/merge · ripple Architect · UX), 2026-06-10. Tout est ancré `file:line` sur le code réel.

## 1. Objectif
Décomposer un gros step (ex. `implement` sur EW-7247, qui a crashé en monolithe) en sous-tâches, **chacune un sous-workflow complet** (implement→review→tests) qui peut **boucler en interne** (tests échouent → retour implement, N fois max), s'exécute en **worktree isolé**, puis **merge** vers une PR.

## 2. Verdict de faisabilité
Deux fondations **existent déjà** → on ne réécrit pas le moteur :
- **Le runner boucle déjà** : `goto_fires` / `max_iterations` par arête / `max_total_iterations` (`runner.rs:388-403`, `ConditionAction::Goto` `:1079`). → les boucles INTERNES d'un sous-workflow sont **gratuites** (un sous-WF est juste un workflow).
- **Le modèle de run a déjà `parent_run_id`** (migration 030, `REFERENCES workflow_runs(id) ON DELETE SET NULL` + index) + `run_type` + `batch_*`. → l'arbre de runs = fermeture transitive de `parent_run_id`, **aucune colonne neuve**.

## 3. Deux primitives distinctes (décision)
- **`StepType::SubWorkflow { sub_workflow_id }`** — lance **UN** run enfant (imbrication profondeur 1+). Pour "ce step EST un pipeline réutilisable".
- **`StepType::BatchWorkflow`** — **fan-out** d'un sous-WF sur une liste (`batch_items_from`), N runs enfants isolés + merge. C'est ce que le cas EW-7247 exige (N sous-tâches).
> `create_batch_run` (`db/workflows.rs:361`) crée des **discussions**, PAS des runs → non réutilisable pour le fan-out de runs ; il faut spawner des `WorkflowRun` enfants pilotés par `runner.rs`. Le pattern Semaphore/wait WS de `batch_step.rs:218-331` se réutilise.

## 4. Architecture convergée

### 4.1 Ré-entrance du runner
- Garder `execute_run` (signature publique inchangée — `resume_run`/api l'appellent). Extraire `execute_run_inner(depth, call_stack, budget, parent_cancel)`.
- **`Box::pin` UNIQUEMENT le bras `SubWorkflow`** du match (`runner.rs:597`) — pas tout le match (évite une alloc heap par step en run plat). Le sous-run a sa propre row `WorkflowRun` (`parent_run_id = parent.id`) → deux `&mut` sur deux objets distincts, zéro conflit d'emprunt.

### 4.2 Budget partagé (LE point de correction dur)
Aujourd'hui `llm_calls_count`, `tokens_used`, le timeout sont **par-run** (`runner.rs:497,926,462`) → un sous-run repart à zéro et explose le budget parent.
- Introduire `SharedBudget { llm_calls: Arc<AtomicU32>, tokens: Arc<AtomicU64>, deadline: Instant }` descendu par paramètre. Les 3 sites de check/incrément lisent/écrivent le compteur partagé (`fetch_add`).
- **Le timeout devient un `deadline: Instant` absolu** calculé au top-level (sinon chaque sous-run se réoffre 30 min). Cohérent avec le wall-clock existant.
- `goto_fires` / `step_revisits` restent **locaux** par sous-run (anti-boucle interne voulue, pas du budget cumulatif).
- `parent.tokens_used += child.tokens_used` après le sous-run (l'agrégation remonte l'arbre sans code spécial).
- Garde-fou anti-régression : `SharedBudget::single_run()` se comporte exactement comme aujourd'hui (pinné par les tests guards `runner.rs:2519-2562`).

### 4.3 Récursion sûre
- **Profondeur** : param `depth` + `MAX_SUBWORKFLOW_DEPTH` (≈5), check runtime → `StoppedByGuard`.
- **Cycle** (A→B→A) : **statique au save** (DFS sur le graphe `workflow_id → sub_workflow_ids`, nouveau `subworkflow_graph.rs`, rejet à l'enregistrement) **ET runtime** (`call_stack: Vec<workflow_id>` ; si l'enfant ∈ call_stack → abort). Defense-in-depth contre le JSON hand-edité.
- ⚠️ **Saut architectural** : `validate_step_references` (`template.rs:236`) est aujourd'hui **pure/synchrone** ; le cycle-check est **I/O DB récursif** (charge les sous-WF via `get_workflow`). C'est le seul vrai changement de nature dans la validation.

### 4.4 Cancel
`CancellationToken.child_token()` (natif tokio_util). Ajouter `CancelGuard::insert_child(registry, key, parent_token)` (`lib.rs:235`) → annuler le parent tue les sous-runs en cascade (kill_on_drop existant).

### 4.5 Modèle d'état
- `StepResult` gagne `child_run_id: Option<String>` (fan-out : `child_run_ids: Vec<String>`), JSON blob `#[serde(default)]` → **zéro migration** (même pattern que `is_rollback`/`step_kind`).
- Run enfant : `run_type = "subworkflow"`, `parent_run_id` posé. **`step_results` reste PLAT** par run ; l'arbre vit dans les rows liées. `next_step_index_for_resume` (lookup par NOM, `runner.rs:54`) **ne casse pas** (il ne voit que les steps de SON run).

## 5. Les 3 problèmes vraiment durs (+ résolution)

### P1 — Gate imbriqué (le plus dur)
Un Gate dans un sous-run → l'enfant passe `WaitingApproval`, le parent est "waiting-on-child". `resume_run` (`runner.rs:1572`) est **mono-niveau**.
- **MVP : INTERDIRE `Gate` dans un `SubWorkflow`** au save (miroir de l'interdiction Gate-en-rollback `runner.rs:1412`). Débloque tout le reste sans toucher au resume.
- **V-full** : état durable `state["waiting_on_child"]=<child_id>` (parent reste `Running`), reprise **bottom-up** : approve gate enfant → `resume_run` enfant → à `Success`, `resume_parent_if_waiting(child_id)` ré-entre `execute_run`. **Garde-fou neuf** : `claim_child_completion(parent_id, child_id)` CAS (calque exact du `claim_waiting_run` TOCTOU corrigé aujourd'hui) — sinon double ré-entrée (WS chaud + boot-scan).

### P2 — Fan-out + worktree + merge
- N runs enfants, Semaphore borné (`N ≤ global_agents/2` pour éviter la famine de permits), worktree par `run_id` (`workspace.rs:133` → **zéro collision gratuite**), base commune = HEAD figé du parent (ajouter `base_ref` à `Workspace::create`).
- **Merge recommandé** : intégrateur **séquentiel rebase** dans le worktree parent, ordre = index ; 1er conflit → `[SIGNAL: CONFLICT]` → Gate humain (réutilise `gate_checkpoint` 0.8.6) ; fallback agent-intégrateur. (Octopus rejeté : n'isole pas le coupable.)
- **Statut unifié** `OK / PARTIAL / ERROR` (corrige l'incohérence Batch* notée à l'audit : `PARTIAL` = step `Success` + signal, pas Failed).
- **Échec partiel** : 6/8 OK → PR partielle + signal `PARTIAL` branchable, tickets pour le reste.

### P3 — Run-tree vs run plat (état + UX + recovery)
- **UX** : accordéon **inline récursif lazy-load** (`RunNode` récursif dans `RunDetail.tsx`), PAS de drill-down. Gate **hoisté à la racine** via `run.pending_gate {run_id, breadcrumb}` + fil d'Ariane cliquable (décision sans naviguer). Boucles internes groupées ("itér. 3/5"). Pire statut propagé vers le haut. Endpoint `GET /runs/:id/tree` (sinon N fetchs récursifs).
- **Recovery au boot** : ⚠️ **n'existe PAS aujourd'hui** (gap pré-existant : un crash laisse tout run `Running` à vie). La récursion l'aggrave. Ajouter un orphan-scan d'arbre **bottom-up** : feuilles Running→Failed, parent waiting-on-child terminé→resume sinon Failed, cleanup worktrees enfants (préserver `produced_branches`).

## 6. Ripple (inventaire condensé — ~15 sites)
- **Enum + dispatch** : `StepType` (`models/workflows.rs:714`) ; runner `:597` (dispatch), `:924` (quota — NE PAS mettre en zéro-coût, agréger les LLM enfants), `:1901` (snapshot label), `:1383` (on_failure — **interdire** SubWorkflow en rollback).
- **Validation** : `api/workflows.rs:281` (champ requis), `template.rs:236/259` (refs + `produces_structured` → standardiser l'envelope de sortie du sous-WF).
- **Export/import** : `export_workflow:865` n'embarque que les QP → ajouter `referenced_workflows` + `wf_id_remap` (récursif) ; bump `EXPORT_VERSION`.
- **Frontend** : `generated.ts` (hand-edit, cf. typegen), `WorkflowWizard.tsx` (picker de workflow + badges), `WorkflowDetail.tsx`/`RunDetail.tsx` (RunNode récursif + gate hoisté), `i18n.ts` FR/EN/ES, presets + **tests avec le code**. `StepType.ts:3` est STALE (à régénérer).
- **Architect skill** (`workflow-architect.md`) : section décision-tree "9. SubWorkflow" ; **obligation d'appeler `workflow_list()`** avant de référencer (anti-hallu d'id) ; garde-fous prompt (mou) + **sécurité dure = validation serveur** (cycle/profondeur).
- **Compat ascendante** : `#[serde(tag="type")]` inconnu → casse TOUT le workflow à la désérialisation → prévoir `#[serde(other)] Unknown` ou version min. Migration templates = **NOUVELLE clé de preset** (`feasibility-autopilot-v2`), jamais muter l'existant (runs en cours).

## 7. Phasage recommandé
1. **Phase 1 — `SubWorkflow` simple** : 1 enfant, profondeur bornée, **Gate interdit dans le sous-WF**, SharedBudget, cycle/profondeur (save+runtime), cancel cascade, `child_run_id`, UX accordéon + endpoint tree. → prouve la ré-entrance + le modèle d'arbre sans le merge.
2. **Phase 2 — `BatchWorkflow`** : fan-out N + worktree isolé + merge séquentiel-rebase + statut OK/PARTIAL/ERROR + contrat du `plan` (schéma sous-tâches `{id, scope[], depends_on[], acceptance}`). → **résout le cas EW-7247**.
3. **Phase 3 — Gate imbriqué + récursion profonde** : `waiting_on_child` durable + `claim_child_completion` + resume récursif + boot orphan-scan d'arbre.

## 8. Arbitrages — FIGÉS (2026-06-10)
- **A. ✅ Phase 1 d'abord** (SubWorkflow simple), puis Phase 2. Preuve avant valeur métier.
- **B. ✅ Gate INTERDIT dans un sous-WF en MVP** (save-time, miroir Gate-en-rollback). Bien plus simple ; resume récursif renvoyé en Phase 3.
- **C. ✅ Merge séquentiel-rebase** + Gate humain sur conflit (Phase 2). N PRs séparées rejeté.
- **D. ✅ Tous les fix de fondation AVANT le chantier** — on assainit le moteur actuel (dont le boot orphan-scan des runs, gap pré-existant) avant d'empiler la récursion dessus.

## 9. Contrat du step `plan` (prérequis commun à toutes les phases)
```json
[{ "id": "sub-1", "title": "...", "scope": ["src/auth/"], "depends_on": [], "acceptance": "POST /login renvoie 200" }]
```
`scope` disjoints = anti-conflit en amont (deux items partageant un fichier → sérialisés ou avertis). `depends_on` → fan-out par vagues topologiques. Consommé tel quel par `parse_items_as_objects` (`batch_apicall_step.rs:329`). Émis en `output_format: TypedSchema` (validé post-extract, `OnInvalid::Fail`) — le mécanisme existe déjà.
