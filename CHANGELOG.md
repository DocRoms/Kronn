# Changelog

All notable changes to Kronn will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

## [0.8.2] - 2026-05-13

**Audit drastique + boucle audit → AutoPilot + worktree discoverability.**
Release centrée sur la qualité de l'audit IA et la fermeture de la
boucle "audit → tickets → AutoPilot → PR". L'audit ne se contente plus
de produire des constats : il a une baseline mandatory non-skippable,
une anti-répétition (slug-matching + reconciliation pass + two-tier
Status), un dispatch par kind (Security / Docker / Performance / A11y /
Database / ApiDesign / Custom) avec cluster detector qui recommande la
prochaine spécialisation, et une table `audit_runs` qui donne au badge
santé sa sparkline + delta. Côté workflow : un bouton "Continuer avec
l'AutoPilot" apparaît après la validation, qui pré-remplit le wizard
sur le ticket le plus ancien du tracker (GitHub / GitLab / Jira) avec
detection du repo. Côté Exec : nouveau `exec_setup_command` (composer
install / npm ci / etc.) avec preset dropdown, plus le fix du
docker-in-docker volume mismatch (self-mount + cwd translation pour les
worktrees), plus un meilleur signaling de "ta commande tourne dans un
worktree git". WebSocket `WorkflowRunUpdated` ajouté pour que la
transition vers un Gate s'affiche live sans refresh quand on arrive
d'un autre onglet.

### Added

- **Audit baseline mandatory checklist (Step 9)** — 4 checks
  non-skippables (auth, persistence, external input, secrets) qui
  émettent une TD baseline même quand le scan dimensionnel n'a rien
  trouvé. Les audits ne reviennent plus "vides" sur du code qui mérite
  au moins un signalement.
- **Audit cap relaxation** — 15-20 → 30 TDs max par run, Critical/High
  exempts (jamais omis). Sur les gros repos l'audit ne s'arrête plus
  artificiellement après Medium 15 en ignorant des Highs.
- **Audit anti-repetition** — trois protections : (1) slug-matching sur
  TDs existantes (un nouveau scan ne crée plus de doublon avec un slug
  légèrement différent), (2) reconciliation pass qui marque les TDs
  obsolètes comme `Resolved` au lieu de les laisser orphelines, (3)
  two-tier Status (`Active` / `Reopened`) pour distinguer une vraie
  régression d'un faux positif. Le slug-churn (le pire anti-pattern
  d'audit) est désormais bloqué par construction.
- **AuditKind enum + per-kind dispatch** — `Full` reste la base, plus
  `Security`, `Docker`, `Performance`, `Accessibility`, `Database`,
  `ApiDesign`, `Custom`. Chaque kind a son prompt système dédié et son
  set de checks baseline. Un audit Security n'est plus un audit Full
  avec un peu de focus sécu.
- **Cluster detector + AuditRecommendation** — Step 10 du Full audit
  inspecte la distribution des TDs et recommande la prochaine
  spécialisation à lancer (ex : 4+ TDs Security → "lance un audit
  Security"). Surfaceé en chip cluster dans le health badge.
- **`audit_runs` table + health badge cluster** — chaque audit crée une
  row avec `started_at`, `ended_at`, `duration_ms`, `td_critical/high/
  medium/low/total`, `td_resolved_since_last`, `td_new_since_last`,
  `td_carried_over`, `health_score` (0-100). Source de vérité pour le
  badge santé du dashboard.
- **AutoPilot CTA after audit validation** — bouton "Continuer avec
  l'AutoPilot" qui apparaît sur la discussion de validation une fois
  l'audit clôturé. Pré-remplit le wizard de workflow sur le ticket le
  plus ancien du tracker du projet (GitHub / GitLab / Jira), avec
  detection automatique du repo (`parseRepoUrl` +
  `inferTrackerSlugFromRepoUrl`). En un clic : audit → TDs → ticket →
  AutoPilot prêt à tirer.
- **Exec `exec_setup_command` + `exec_setup_args`** — phase setup avant
  la commande principale d'un step Exec, avec preset dropdown
  (`composer install`, `npm ci`, `pnpm install --frozen-lockfile`,
  `yarn install`, `poetry install`, `pip install -r requirements.txt`).
  Indispensable pour que la commande principale (tests / build) trouve
  ses dépendances dans un worktree fraîchement créé.
- **WS `WorkflowRunUpdated` event** — broadcast à chaque transition
  d'étape + flip de status du run. Le frontend rafraîchit la liste des
  runs quand on ouvre la page d'un workflow en cours depuis un autre
  onglet, sans devoir F5. La transition vers un Gate apparaît live.
- **Per-step token badge in WorkflowDetail** — le compteur de tokens
  n'est plus seulement au niveau du run, il est aussi affiché par
  step. Plus de surprise sur quelle étape consomme.
- **Authoritative `step.started_at` timestamp** — chaque `StepResult`
  capture l'heure wall-clock de démarrage côté backend (plus d'estimate
  côté frontend basé sur la somme des durées précédentes). La durée
  vraie d'un step est désormais persistée et survit aux reloads.
- **Gate pause duration tracking** — le `duration_ms` d'un step Gate
  reflète maintenant la vraie durée de la pause (now - started_at)
  quand l'opérateur valide. Avant : ~0ms (temps de rendu), maintenant :
  le temps que l'humain a mis à décider.
- **`effectiveLiveRun` cross-tab persistence** — quand on navigue vers
  un workflow en cours depuis un autre onglet, on synthétise un état
  "pseudo-live" à partir du dernier run non-fini de la liste. Plus de
  "page collapsée vide" qui fait croire que le run est bloqué.
- **Tracker hint banner on ProjectCard** — surface l'URL du tracker
  détectée (`parseRepoUrl(project.repo_url)`) avec un dismissible
  localStorage flag, pour amorcer la conversion repo → AutoPilot.
- **`buildOldestIssueRequest` helpers** — switch par tracker
  (`github` / `gitlab` / `jira`) qui produit la bonne requête HTTP pour
  récupérer le ticket ouvert le plus ancien. 9 tests unitaires.
- **Exec step worktree discoverability hints** — hint dédié pour Exec
  step au premier rang (fresh worktree) vs steps suivants (sees
  previous changes), plus warning visible quand `project_id` est null
  (commande tourne dans le CWD de Kronn, pas de worktree).
- **Audit elapsed time counter** — ticker côté client (1s) qui affiche
  le temps écoulé depuis le démarrage de l'audit en cours, calé sur le
  `started_at` du serveur. Plus d'incertitude pendant les 10-20 min
  d'un audit Full.
- **Volume mounts for non-standard CLI paths in Docker** — `cargo`,
  `bun`, `~/.rustup`, plus un `/host-bin/extra` escape hatch. Auto-
  detection dans le `Makefile` qui écrit `.env` si les répertoires
  existent. Couvre les ~20% d'users qui n'ont pas leurs outils dans
  `/usr/bin` ou `~/.local/bin`.
- **GitHub Community Standards files** — `CODE_OF_CONDUCT.md`
  (Contributor Covenant 2.1), `SECURITY.md` (private advisory route,
  SLA, scope), `.github/ISSUE_TEMPLATE/{bug_report,feature_request,
  config}.{md,yml}`, `.github/pull_request_template.md`.
- **README EN + FR section 5 & 6 rewrites** — la section "Audit your
  codebase with an AI that doesn't forget" reformulée pour couvrir les
  6 hardenings 0.8.2 (Mandatory baseline, Anti-repetition, Two-tier
  Status, Specialized kinds, Health badge cluster, Community-standards
  gate). Nouvelle section "Close the loop: audit → tickets →
  AutoPilot → PR".

### Changed

- **CSS extraction for `ActiveRunsPopover`** — déplacé hors de
  `pages/WorkflowsPage.css` vers un fichier co-located
  `components/workflows/ActiveRunsPopover.css`. Avant : le popover des
  runs actifs (rendu depuis Dashboard, donc visible sur tous les
  onglets) apparaissait unstyled quand on cliquait dessus depuis
  Discussions tant que WorkflowsPage n'avait pas été monté au moins
  une fois.
- **Docker volume mounting strategy** — self-mount + cwd translation
  `/host-home/` → `${KRONN_HOST_HOME}/` pour les worktrees git
  créés sur le host et lus depuis le container. Le path parity est
  désormais préservé inside/outside container, prérequis pour les
  steps Exec qui touchent des worktrees.
- **`RUSTUP_HOME` propagation** — le container reçoit la même valeur
  que le host pour que les shims `cargo` / `rustc` trouvent leur
  toolchain. Mount du dossier `~/.rustup` au même chemin absolu.
- **Tracker MCP detection precedence** — `repo_url > project-scope >
  global` au lieu de `is_global > everything else`. Empêche un Jira
  global de masquer un GitHub spécifique au repo.

### Fixed

- **CSS missing on live-WF box when arriving from another tab**
  (TD #248) — le popover des runs actifs apparaissait sans style sur
  les onglets Discussions/Projects/Settings tant que WorkflowsPage
  n'avait pas été mounté.
- **Live Gate transition without page refresh** (TD #247) — la
  transition d'un run vers un Gate (status `Running` → `WaitingApproval`)
  ne se voyait pas live quand le panel était ouvert depuis un autre
  onglet : la SSE est tab-local, l'autre tab ne recevait rien. Le WS
  `WorkflowRunUpdated` mirror les transitions sur tous les clients.
- **Docker-in-docker volume mismatch for worktree Exec steps**
  (TD #249) — un step Exec qui tournait sur un worktree créé côté host
  voyait un `work_dir` invalide à l'intérieur du container (le path
  host n'existait pas), faisant échouer toute commande qui faisait du
  `find` ou de l'IO. Self-mount + traduction de chemin garantissent
  que le `cwd` est valide des deux côtés.
- **GitHub API 422 on `buildOldestIssueRequest`** — User-Agent manquant
  sur le reqwest builder. Ajout de `.user_agent(concat!("Kronn/",
  env!("CARGO_PKG_VERSION")))`.
- **bash + `["make test"]` foot-gun** — validator catché à la
  sauvegarde du workflow, avec message actionnable qui explique de
  splitter `["-c", "make test"]` ou d'utiliser directement `make`
  comme binaire.
- **Per-disc sendingMap leak on batch fan-out** — `BatchRunProgress`
  inclut maintenant le `discussion_id` de l'enfant qui vient de
  terminer pour que le frontend puisse clear son indicateur local
  (les enfants de batch n'ont pas de consommateur SSE).
- **Cargo `rustup` shim toolchain lookup** — les shims ne trouvaient
  pas la toolchain dans le container parce que `~/.rustup` n'était pas
  monté au même chemin absolu. Mount + `RUSTUP_HOME` env propagation.

### Tests

- 2 round-trip serde tests pour `WsMessage::WorkflowRunUpdated`
  (variant complète + variant `current_step=None`).
- 8 validator tests pour `validate_exec_steps`
  (`bash`-multi-word foot-gun + `exec_setup_command` allowlist +
  path-separator + shell-vs-bin distinction).
- 9 tests `buildOldestIssueRequest` (GitHub / GitLab / Jira shapes).
- Mock `useWebSocket` ajouté à `WorkflowsPage.test.tsx` +
  `WorkflowsPage.qp-launch.test.tsx` (le hook réel essayait d'ouvrir
  une WS dans jsdom).
- Suite complète au vert : 1870 tests backend, 1161 tests frontend.

## [0.8.1] - 2026-05-12

**Custom API plugin + AI helpers UX refactor + tech-debt prominence + doc rebrand.**
Release de "vraies features qui débloquent du monde" : N'importe quelle
API REST peut maintenant être pilotée par Kronn (plus uniquement
Chartbeat/Adobe/Jira), les helpers IA ouvrent direct sur le chat (plus
de modal séparé pour choisir l'agent), la dette technique est visible
en un coup d'œil sur chaque projet, et toute la terminologie
"AI documentation" passe en "project documentation" (le pivot
`ai/` → `docs/` du 0.7.1 est désormais complet jusque dans les UI strings
et les agent prompts).

### Added

- **Custom API plugin** — sentinel `api-custom` dans `core/registry.rs`,
  pinnée en tête du drawer "Add plugin". Picking it swap le panneau de
  droite vers un éditeur freeform (Name + Base URL + Describe + Docs
  link + N {Label, Value} fields). Le backend matérialise un fresh
  `McpServer` (id `custom-{slug}-{nano}`, source = `Manual`, transport
  `ApiOnly`) avec `ApiSpec` construite depuis le payload. Auth = `None`
  par design : l'agent lit la description + docs URL + fields et figure
  out l'auth lui-même. Helpers `slug_env_key` (slugifier
  `Bearer Token` → `BEARER_TOKEN`) + `materialize_custom_server` +
  `name_slug`. 5 tests Rust + 2 tests vitest. Couvre tous les use cases
  "j'ai une API interne / Salesforce / Stripe / autre vendeur non listé".
- **Custom API AI helper bubble (`CustomApiAiHelper.tsx`)** — chat
  éphémère qui pré-remplit le formulaire Custom API depuis un curl, un
  lien doc ou une description libre. Mirror du pattern
  `ApiCallAiHelper` (KRONN:APPLY blocks, ephemeral discussion,
  agent dropdown). System prompt dédié qui extrait
  `{name, base_url, description, docs_url, fields[]}`. Apply merge
  intelligent : préserve les valeurs utilisateur déjà saisies, accepte
  les nouveaux labels de l'agent. 16 unit tests pinent le wire
  contract + le rendu.
- **AI helper UX refactor (option B)** — passe `ApiCallAiHelper` de 3
  phases (closed/picking-agent/chatting) à 2 (closed/chatting). Click
  trigger → bulle ouverte direct avec le 1er agent installé. Header de
  bulle accueille un dropdown agent (avatar + nom + chevron) qui
  permet de switcher au milieu d'une conversation (reset le chat, prime
  une nouvelle discussion avec le même system prompt). Context chip
  remonté en haut de la bulle (sous le header) pour qu'on voie ce que
  l'agent sait avant le scroll. Welcome state avec 3 starter chips
  cliquables (pré-remplissent l'input avec un template) à la place de
  l'agent qui s'auto-fire à l'ouverture — économise ~200 tokens par
  helper-open. Tests mis à jour. CSS extraite dans
  `frontend/src/components/aiHelper.css` pour que les styles chargent
  aussi sur McpPage (le bug qui rendait la bulle non-stylée sur
  d'autres pages).
- **Tech-debt count badge on ProjectCard** — nouvelle field
  `Project.tech_debt_count: u32` peuplée par `scanner::count_tech_debt`
  qui compte les TD-* uniques (union dédupliquée des fichiers sous
  `docs/tech-debt/` + des lignes `| TD-` dans
  `docs/inconsistencies-tech-debt.md`). Affichée comme badge orange
  `⚠ N TD` sur la ligne du titre du projet. Click → ouvre la card si
  elle est fermée + déplie la section docs + deep-link
  `initialExpandFolder='docs/tech-debt'` qui auto-sélectionne le
  premier TD-*.md. README/TEMPLATE.md exclus du compte (scaffolding).
  4 tests Rust dont un dédié à la régression de double-comptage.
- **"Régler ce problème" CTA on TD files** — quand l'AiDocViewer
  affiche un fichier `docs/tech-debt/TD-*.md`, le bouton
  "Discuss this file" devient "Régler ce problème" (warning-tone,
  bouton bold). Même action sous-jacente (lance une discussion avec le
  fichier en contexte) mais le prompt est résolution-oriented : ask
  l'agent un plan court, exécuter les modifs, mettre à jour le
  TD-*.md (statut résolu) et la ligne d'index. Détection via regex
  permissive `/tech-debt/.*TD-*.md` — symétrique avec
  `count_tech_debt` côté backend.
- **Docs viewer always-visible + state banners** — la section
  "Project documentation" sur la ProjectCard n'est plus gatée sur
  `audit_status === 'Validated'`. Elle s'ouvre quel que soit l'état
  d'audit. Une bannière contextuelle dans le viewer guide vers la
  prochaine étape :
  - `NoTemplate` / `TemplateInstalled` : "Lance un audit IA pour
    (re)documenter intégralement le projet…"
  - `Bootstrapped` : "Bootstrap terminé. Lance l'audit complet…"
  - `Audited` : "Valide l'audit pour avoir une documentation à jour…"
  - `Validated` : pas de banner (état "propre")
  Auto-fix : quand on clique le badge TD sur une card fermée, la card
  s'ouvre + déplie la section docs (avant on cliquait dans le vide).
- **AI audit Step 9 (tech-debt) enrichi** — `ANALYSIS_STEPS[8]` dans
  `backend/src/api/audit/mod.rs` passe de 7 dimensions à **10** :
  ajout d'**Accessibility** (form labels, contrast 4.5:1, ARIA,
  keyboard-nav, focus traps, semantic HTML), **Observability**
  (logging hot paths, error tracker, health endpoints, SLI metrics),
  **Documentation drift** (cross-check des 8 fichiers `docs/` que
  l'agent vient d'écrire contre le code source — détecte
  contradictions type "coding-rules.md dit X mais aucun linter ne
  l'enforce"). Le detail file gagne 3 champs : **Status**
  (Draft / In progress / Blocked upstream / Mitigated),
  **Effort** (S/M/L/XL), **Blast radius**
  (local / module / cross-cutting). Calibration de la severity
  avec exemples concrets (Critical = data leak / SQL injection,
  High = test suite red / build broken, Medium = test suite >30s
  / N+1, Low = cosmetic) pour limiter la sur-classification en
  Medium. Nouvelle règle "tickets dedup" : si un MCP tracker
  (Jira/Linear/GitHub) est configuré, l'agent fait une recherche
  read-only avant de créer un TD pour éviter de dupliquer un ticket
  existant. Tests audit (13) toujours verts. Compatible 100% backwards :
  les TDs déjà créés avec l'ancien format restent valides.
- **Persistent AI audit section dans le README** — nouveau §5 dans
  "What you can do" qui détaille les 8 fichiers générés
  (`docs/AGENTS.md`, `glossary.md`, `repo-map.md`, `coding-rules.md`,
  `testing-quality.md`, `architecture/overview.md`,
  `operations/debug-operations.md`, `operations/mcp-servers.md`) +
  le status flow `NoTemplate → TemplateInstalled → Bootstrapped →
  Audited → Validated` + le drift detection granulaire par section.
  Sells "Kronn = knowledge persistence layer, not just a prompt
  launcher".
- **`AiDocViewer` props `initialExpandFolder` + `banner`** — slots
  optionnels qui ne cassent aucun consumer (props
  `?`). `initialExpandFolder` déplie tous les prefixes du folder en une
  seule passe + pre-sélectionne le premier fichier qui matche.
  `banner` est un React node libre, le caller contrôle icône + ton.
  Helper `findFirstFileUnder` ajouté.
- **Custom API helper E2E spec** (`custom-api-helper-bubble.spec.ts`) —
  smoke Playwright qui couvre les ouverture de la bulle, les starter
  chips, l'agent dropdown, et la fermeture. Vérifie
  `getComputedStyle(bubble).position === 'fixed'` comme proxy pour la
  régression CSS qui avait initialement motivé l'extraction
  `aiHelper.css`.
- **README + dark screenshots EN/FR** — 8 PNG en thème sombre (4 ×
  EN + 4 × FR) pour le dashboard, Quick Prompts, QP launch
  (compare-agents avec 7 chips), workflow wizard. Banner
  `Kronn_Hero.png` + 4 SVG diagrammes (decomposition + data-flow, FR/EN)
  dark-only pour cohérence visuelle avec le logo. Script
  `scripts/seed-demo-fixtures.sh` reproductible + page
  `docs/operations/screenshot-sandbox.md` qui documente le workflow.
  Section "Any REST API works" ajoutée pour expliquer le Custom API
  flow.

### Changed

- **Doc rebrand `ai/` → `docs/` complet** — passe sur tous les
  `.md` du repo Kronn lui-même (~30 refs dans `docs/AGENTS.md`,
  `glossary.md`, `decisions.md`, `repo-map.md`,
  `architecture/overview.md`, `operations/mcp-servers/drawio.md`).
  Tooling ne lit plus jamais `ai/` (la migration shippée en 0.7.1 est
  désormais complète en surface ET en profondeur). Les refs
  historiques type "legacy `ai/` directory was migrated to `docs/` in
  0.7.1" sont gardées comme notes historiques.
- **Terminology "AI documentation" → "project documentation"** —
  13 strings i18n × 3 langues (FR/EN/ES) plus les hardcoded JSX
  badges sur `ProjectCard.tsx`. Le badge "AI context" devient
  "Project docs". Les agent prompts (`audit.validationPrompt` ×3,
  ~1k tokens chacun) sont récrits pour pointer vers `docs/` (au lieu
  de `ai/`) — l'agent va donc maintenant écrire dans le bon dossier
  après le pivot.
- **Templates de bootstrap** — `templates/docs/AGENTS.md` :
  "Modify business code when the task is only about AI context" devient
  "...only about project documentation". `templates/docs/architecture/
  overview.md` : "Architecture (AI context)" → "Architecture". Tout
  nouveau projet bootstrappé naît avec la nouvelle terminologie.
- **Sandbox screenshot pipeline** — em-dashes nettoyés du
  `scripts/seed-demo-fixtures.sh` (préférence user : "we never do that"),
  3 phrases bancales (après suppression em-dash) rephrasées pour rester
  grammaticales. CSS shared move vers `frontend/src/components/aiHelper.css`
  (avant : `WorkflowsPage.css`) — corrige le bug qui rendait la bulle
  helper non-stylée sur McpPage.

### Fixed

- **Workflow trigger: variables non-déclarées auto-détectées** —
  user-reported sur "autoBot" workflow : step 1 utilise `{{issue}}`
  dans le prompt mais `Workflow.variables` était vide → le launch
  modal était skippé → step fire avec literal `{{issue}}`. Fix :
  nouveau helper `lib/workflowVariables.ts` qui scanne TOUS les
  champs templated d'un workflow (`prompt_template`, `api_endpoint_path`,
  `api_query`/`api_headers`/`api_body`, `notify_config.url`/
  `body_template`/`headers`, `exec_args`, `batch_items_from`) +
  retourne les `{{var}}` non-runtime. `handleTrigger` merge
  declared + auto-detected, ouvre le modal s'il y a quelque chose à
  saisir. Change connexe : `isRuntimeToken` (apiCallPlaceholders.ts)
  filtre désormais UNIQUEMENT les `ns.X` multi-segments — un
  `{{batch}}` bare est maintenant traité comme user-var (avant : eaten
  silently). 12 tests neufs dans `lib/__tests__/workflowVariables.test.ts`
  dont une régression dédiée `autoBot {{issue}} regression`.
- **`docs_migration` re-runs rewrite pass sur AlreadyMigrated** —
  user-reported : projets déjà migrés vers `docs/` gardaient des refs
  `ai/...` stales dans le contenu de leurs `.md` parce que le early
  return `AlreadyMigrated` skippait `rewrite_internal_refs` +
  `rewrite_root_redirectors`. Fix : variant devient
  `AlreadyMigrated { refs_rewritten: usize }`, les deux rewriters
  (idempotents) sont appelés systématiquement, le compteur retourné
  dans la réponse HTTP pour que l'opérateur voit "12 refs cleaned"
  quand il re-clique sur "Migrer". `MigrateDocsResponse.refs_rewritten`
  désormais peuplé même pour `status: "already_migrated"`. 1 test neuf
  `already_migrated_cleans_stale_ai_refs` qui prouve qu'un repo déjà
  à `docs/` avec des `ai/X` refs résiduelles sort propre après
  re-trigger.
- **`count_tech_debt` double-counting (régression flaggée user)** —
  avant : 5 fichiers + 7 lignes index = 12 sur le badge alors que
  l'utilisateur ne voit que ~7 unique TDs dans la doc. Maintenant
  dédupliqué par ID (extrait du `file_stem` côté fichiers + du
  premier token `TD-...` côté lignes). Sur Kronn lui-même : 12 → 7
  (cohérent). Test dédié `count_tech_debt_dedupes_file_and_index_pair`
  pin la régression.
- **E2E `custom-api-helper-bubble.spec.ts` count-before-visible** — le
  test échouait en CI parce que `expect(toBeVisible)` s'exécutait
  avant le check `skip if no agents installed`. Ordre inversé + tick
  de settle DOM ajouté. Skip cleanly maintenant quand le sandbox CI
  n'a pas d'agents installés.
- **TD badge click + card fermée** — avant : clic sur `⚠ 12 TD`
  appelait `setExpandedTab('docAi')` mais la card étant fermée, le
  body n'était pas rendu → l'utilisateur cliquait dans le vide.
  Maintenant : `if (!isOpen) onToggleOpen()` ajouté avant le
  setExpanded. Un seul click suffit pour passer de "card fermée" à
  "viewer ouvert sur le premier TD".
- **`docs/architecture/overview.md` heading `(AI context)`** —
  cohérence avec le rebrand global, ce reliquat se balladait.

### Tests

- Backend : **1614 tests** (1613 + 1 nouveau test `count_tech_debt`
  pour la régression dédup). `cargo clippy --lib -- -D warnings` clean.
- Frontend : **1128 tests** (1112 + 16 nouveaux `CustomApiAiHelper`).
  `pnpm tsc --noEmit` clean. `pnpm lint` : 0 errors, 100 warnings
  (toutes pré-existantes).
- E2E : nouveau spec `custom-api-helper-bubble.spec.ts`.

### Docs

- `docs/architecture/overview.md` : nouveaux paragraphes
  Custom API plugins + AI helper bubble (UX 0.8.1, shared CSS,
  TD-helpers-unify noté).
- `docs/operations/screenshot-sandbox.md` : nouveau, ~45 lignes,
  documenté + référencé depuis `CONTRIBUTING.md`.
- README.md + README.fr.md : new section "Any REST API works", new §5
  Persistent AI audit, 0 em-dashes (préférence user).

## [0.8.0] - 2026-05-11

**Stabilisation tier 1 + 2 — fin des chantiers legacy**.
Grosse passe d'audit : bug RTK Gemini fixé, pastilles « update available »
pour RTK + tous les agents, sweep a11y form-labels, 3 chantiers à 80%
poussés à 100% (Ollama default-model picker, QP chain DnD reorder +
`{{previous_qp.output}}` template var, bootstrap dev-kickoff CTA). Inclut
aussi le compare-agents shippé en cours de cycle + le gros nettoyage MCP
cross-agent. 2 entrées TD fermées (`rtk-last-agent-button`,
`multi-agent-disc-finished-toasts`).

### Added — Tier 2 (boucle des chantiers à 80%)

- **Ollama default model picker** — la liste des modèles installés
  dans `OllamaCard` est maintenant cliquable (radio-style). Clic →
  POST `/api/config/model-tiers` immédiat avec
  `ollama.default = <name>`. Pastille « défaut » sur le modèle actif,
  rollback optimiste sur erreur réseau. Backend : nouveau champ
  `ModelTierConfig.default` (`Option<String>`, serde-skipped si
  `None` → backcompat parfaite), lu par `resolve_model_flag` pour
  le tier `Default` avant le built-in `llama3.2`. +1 unit test
  (`resolve_model_flag_default_tier_honors_user_override`).
- **QP chain — DnD reorder (Phase 3)** — les pills de chaîne dans
  `WorkflowWizard BatchQuickPrompt step` sont draggables (HTML5 natif,
  pas de nouvelle lib) + boutons `↑`/`↓` pour clavier/a11y. Drop sur
  une pill réorganise via `splice`/`splice`. Boutons grisés en bout
  de chaîne. Pas de touch support pour le moment — desktop-first.
- **QP chain — `{{previous_qp.output}}` (Phase 4)** — chaque QP
  chaîné peut consommer la réponse de l'agent précédent via cette
  variable dans son template. Use case killer : « brief → plan →
  tickets ». Substitution faite dans
  `spawn_agent_run_with_chain` avant insertion du message User. Pure
  helper `render_chain_qp_prompt` extrait + 6 unit tests (substitution
  vide quand pas de previous, regression guard contre smuggling via
  `batch_item`, etc.). Hint i18n enrichi sur les 3 locales.
- **Bootstrap dev-kickoff CTA** — nouvelle action « 🚀 Start dev on
  issue #1 » sur le banner `KRONN:ISSUES_READY/CREATED`. Plombé via
  `onSetDiscPrefill` (symétrie Projects → Discussion existant). Prompt
  prefillé en FR/EN/ES qui demande à l'agent d'ouvrir l'issue #1,
  lire la description, implémenter pas à pas, mettre à jour le
  tracker MCP + commit logiquement. Unlocked → l'utilisateur peut
  tweaker avant d'envoyer.

### Added — Tier 1 (stabilisation)

- **Pastilles « update available » pour RTK + agents** — backend :
  nouveau module `core/versions.rs` avec table `LATEST_RTK_VERSION` +
  `latest_known_agent_version(AgentType)`, comparateur semver souple
  (strip `v`, ignore pre-release/build, zero-pad). Endpoint
  `GET /api/rtk/version`. `AgentDetection.latest_version` populée à
  la détection. Frontend : mirror TS dans `lib/version.ts` (même 8
  cases de tests qu'en Rust). Pastille cliquable dans
  `CompressionSection` (RTK) et sur chaque ligne `AgentsSection`
  (agents) → modal avec commande update copy-pasteable. Versions
  captées 2026-05-11 dans la table : RTK 0.37.2, claude-code 2.0.51,
  codex 0.62.0, gemini-cli 0.18.0, vibe 0.0.16, copilot 0.0.346,
  ollama 0.4.7. À bumper à chaque release Kronn.
- **A11y form-labels sweep** — 12 `aria-label` ajoutés sur les
  formulaires les plus traffickés : `SettingsPage` ×7 (scan-path,
  scan-ignore, skill form ×4, directive form ×6, domain),
  `NewDiscussionForm` ×5 (title, prompt, branch-name, base-branch,
  file-attach), `WorkflowWizard` ×4 (name, project, agent, prompt
  textarea sur Step 0+1). 1668 clés i18n triple-localisées.

### Fixed — Tier 1

- **RTK Gemini hook « Enable on the 1 remaining » fix** — deux bugs
  combinés faisaient que cliquer le bouton retournait `success: true`
  sans changer l'état de l'agent. (1) Matrice backend pour Gemini
  manquait `--auto-patch` → RTK demandait `Patch settings.json? [y/N]`
  sans stdin → défaut N → `~/.gemini/settings.json` jamais patché.
  (2) Détection scannait `~/.bashrc/.zshrc` mais RTK 0.37 n'écrit
  jamais dans le shell rc pour Gemini → toujours `false`. Fix :
  matrice = `["init","-g","--gemini","--auto-patch"]`, détection
  check `~/.gemini/hooks/rtk-hook-gemini.sh` (primaire) + `GEMINI.md`
  + `settings.json`. Uninstall matrice mirroir avec `--auto-patch`.
- **OllamaCard `canirun.ai` placement** — le lien sizing-hardware
  était sous l'indication « comment démarrer Ollama », souvent skippé
  par les users qui pensent que leur machine est trop faible (alors
  que canirun.ai existe exactement pour répondre à ça). Déplacé dans
  une box info juste sous le titre « Ollama », visible dans tous les
  états (y compris `not_installed`). Doublon en bas supprimé.
- **Lien compte Vibe** — `console.mistral.ai/usage` (404) →
  `admin.mistral.ai/organization/workspaces` (la vraie page workspace
  admin où voir la conso + plan).

### Tests

- **E2E `batch_quick_prompt` avec WS mocké** — 2 nouveaux tests dans
  `workflows/batch_step.rs` couvrant le pipeline complet :
  `_fire_and_forget_full_pipeline` (template render → QP load →
  `create_batch_run` → fan-out spawn → output PENDING) et
  `_wait_for_completion_with_mocked_ws` (watchdog tokio qui scrute la
  DB pour le child run id puis émet un `BatchRunFinished` synthétique →
  step retourne avec output OK). Ferme le dernier TODO du
  `project_batch_workflows`.
- Backend : **1787 tests** pass (1781 → +6 chain render Phase 4).
- Frontend : 1110 tests pass (no regression).

### Internal

- **2 entrées TD fermées** : `TD-20260510-rtk-last-agent-button`
  (résolu par le fix Gemini ci-dessus), `TD-20260510-multi-agent-disc-finished-toasts`
  (résolu par useToast dedup window 1.5s en début de cycle). Détail
  files supprimés.

---

### (Suite — Compare-agents (🤝) + per-agent MCP hardening, shipped en cours de cycle 0.8.0)

Suite de l'audit. Nouvelle feature « comparer un QP sur N agents » +
gros nettoyage de la couche MCP cross-agent — paths cross-container,
config Gemini, hint d'erreur agent-aware, et **détection +
masquage** des MCPs dont la config est incomplète pour qu'ils ne
cassent plus le boot des agents.

### Added — Compare-agents (🤝)

- **`POST /api/quick-prompts/:id/compare-agents`** — fan-out d'un
  QP sur une sélection d'agents installés (1 prompt × N agents).
  Réutilise `create_batch_run` avec `agent_override` par item.
  Cap à 50 agents max, dedup côté backend.
- **`WorkflowsPage` chip selector** — bouton 🤝 ouvre le formulaire
  avec une liste de chips pré-cochées (1 par agent installé). Clic
  toggle, lien Tout/Aucun. Le CTA `🤝 Comparer (N)` se met à jour
  dynamiquement et désactive si N=0. Pour les QPs sans variable, le
  formulaire s'ouvre quand même pour permettre la sélection.
- **Fan-out runs** — chaque disc enfant reçoit son propre `POST
  /run` (même pattern que `handleBatchLaunch`). Pre-fix seul le
  premier disc auto-runait via `onNavigateDiscussion`, les autres
  restaient dormants. `onBatchLaunched(discIds, runId)` route
  ensuite vers Discussions + `setFocusBatchId` pour auto-expand le
  📦 dans la sidebar.
- **Auto-expand du dossier batch en sidebar** — `DiscussionSidebar`
  force-déplie le project folder ET le batch folder qui contient le
  disc actif. Pre-fix l'utilisateur landait sur `disc1` mais voyait
  un `📦` collapsed cachant les frères → assumait « 1 seul agent a
  tourné ».

### Fixed — MCP cross-agent

- **`kronn-internal` MCP path cassait les CLI host** — Kronn écrivait
  `/app/scripts/disc-introspection-mcp.py` (chemin container-only)
  dans `.mcp.json` / `.kiro/settings/mcp.json` / `.gemini/settings.json`
  / `~/.codex/config.toml`. Sur le host, `kiro-cli` (et autres CLIs
  lancés directement) plantait avec `Broken pipe (os error 32)` sur
  chaque init MCP. Fix : `disc_introspection_mcp_path_for_shared_config()`
  qui lit l'env `KRONN_INTROSPECTION_PUBLIC_PATH` (auto-set par
  `docker-compose.yml` via self-mount `./backend/scripts:${PWD}/backend/scripts:ro`)
  → le path résout des deux côtés au même string absolu. Si pas de
  path partagé, on **skip** + clean les entrées stale plutôt que
  d'écrire un command cassé.
- **Gemini CLI 0.32 ignore `apiKey` dans `~/.gemini/settings.json`**
  — exige `GEMINI_API_KEY` env var. Quand l'utilisateur s'était auth
  via `gemini auth login` (qui écrit settings.json) sans définir l'env,
  Kronn ne propageait pas la clé → Gemini répondait
  `MCP issues detected. Run /mcp list for status. ⚠ Network error.
  Unable to reach the API.`. Fix : `read_gemini_settings_api_key()`
  fallback dans `get_api_key`, lit le champ `apiKey` du settings.json
  quand env + token store sont vides.
- **Préfixe `MCP issues detected.` pollue le transcript Gemini** —
  Gemini préfixe sa réponse de cette ligne dès qu'UN MCP rate son
  handshake (auth périmée, network bloqué, etc.) même quand la
  réponse réelle est OK. `parse_token_usage(GeminiCli)` strip
  désormais le marker (newline OU inline) + les lignes
  `Server '…' supports tool updates`, `[MCP error]`,
  `[WARN] Skipping unreadable…`. Les sauts de paragraphe du vrai
  contenu sont préservés.
- **Tous les error hints pointaient vers `status.anthropic.com`** —
  même quand l'agent qui plantait était Gemini (status Google),
  Codex (status OpenAI), Copilot (status GitHub). Fix :
  `detect_agent_error_hint(output, agent_type)` route maintenant
  vers la bonne page de status par provider :
  - ClaudeCode → status.anthropic.com
  - Codex → status.openai.com
  - GeminiCli → status.cloud.google.com/products/google-ai-studio
  - CopilotCli → www.githubstatus.com
  - Vibe → status.mistral.ai
  - Kiro → kiro.dev
  - Ollama → hint local "check `ollama serve`"

### Added — MCPs incomplets : détection + skip + UI

- **`McpIncompleteConfig`** — nouveau modèle exposé via
  `McpOverview.incomplete_configs`. Liste les MCPs dont les
  `env_keys` déclarés sont vides ou non-décryptables (clé du cipher
  changée). `find_incomplete_configs(configs, servers, secret)`
  walk les configs, décrypte, retourne la liste.
- **Skip à la sync** — `sync_project_mcps_to_disk` ne **n'écrit
  plus** les MCPs incomplets dans les fichiers project-level. Les
  agents au boot ne tentent plus de handshake avec un MCP cassé →
  plus de `Connection closed`, plus de stall de 5 min sur un OAuth
  invalid_client.
- **Banner UI dans la page Plugins** — `mcp-warning-banner` liste
  les MCPs non-opérationnels avec leurs clés manquantes et un
  raccourci pour ouvrir leur config. i18n FR/EN/ES.

### Added — Tests

- **`per-agent-mcp-introspection.spec.ts`** — PW E2E réel pour
  chaque agent introspection-capable (Claude, Kiro, Gemini, Copilot).
  Crée un disc avec un fact unique, prompt l'agent à appeler
  `mcp__kronn-internal__disc_get_message(0)`, vérifie que le
  counter SQL bump + le badge se rend dans le transcript + la
  réponse contient le fact verbatim. Skip-if-not-installed +
  skip-if-bailout (subscription / rate limit upstream → pas un bug
  Kronn).
- **Régressions backend** —
  `find_incomplete_configs_flags_missing_keys`,
  `find_incomplete_configs_flags_decrypt_failure`,
  `inject_kronn_internal_path_resolution`,
  `gemini_strips_mcp_issues_prefix` (×3 cas),
  `rate_limit_for_gemini_points_at_google_status` (+ Codex / Claude
  / Copilot / Ollama variants).

### Fixed

- **Race conditions « double-clic » sur les boutons asynchrones**
  — `disabled={busy}` est closure-stale entre deux clics
  synchrones avant que React n'ait re-rendu. Patch consistant à
  base de `useRef` synchrone sur 9 sites :
  - `WorkflowsPage` : `handleLaunchQP`, `handleLaunchQA`,
    `handleBatchLaunch`, `handleBatchLaunchQA`, `handleTrigger`
  - `ProjectCard` : briefing button, `handleFullAudit`,
    `startPartialAudit`
  - `NewDiscussionForm` : `handleCreate`
  - `ApiCallAiHelper` : `sendMessage`
  - `DiscussionsPage` : `handleEditMessage`
  - `QuickPromptForm` / `QuickApiForm` / `WorkflowWizard` :
    `handleSave`
  - `SetupWizard` : `handleComplete` (premier lancement —
    double-clic créait des projets dupliqués)
- **Tour guidé étapes 11/12 (`waitForClick`)** — les boutons
  Précédent/Suivant étaient cachés ET les helpers `next`/`prev`
  bailaient sur `waitingForClick`. Conséquence : impossible de
  revenir en arrière, lag perçu de 400 ms après le clic, skip qui
  laissait `kronn:tour-step` dirty parce que `setTimeout` n'était
  jamais annulé. Fix : Prev/Next visibles + fonctionnels en
  `waitForClick`, délai post-clic 400 → 150 ms, `pendingAdvanceRef`
  annulé proprement par `cleanupClickListener`, `start` initialise
  directement à `resumeStep` (plus de flash de l'étape 0).
- **Popover `:emoji` — seuls les 2 premiers items sélectionnables
  au clavier** — `onKeyUp` rappelait `refreshEmojiQuery` qui
  terminait par `setEmojiIndex(0)`. Conséquence pour ArrowDown :
  keydown → 1, keyup → 0. L'item 1 atteignable le temps d'un
  microflash, items 2+ jamais. Fix : `onKeyUp` skip Up/Down quand
  la popover est ouverte (Left/Right/Home/End restent actifs car
  ils déplacent vraiment le caret).
- **Saut de scroll à la fin du stream** — `cleanupStream` flippait
  `sending=false` AVANT que `reloadDiscussion` n'ait rapatrié le
  message persisté. Le DOM perdait la hauteur de la bulle de
  streaming, le navigateur clampait `scrollTop` UP au dernier
  message utilisateur, puis l'auto-scroll smooth animait DOWN au
  bas du nouveau message. Fix : promotion optimiste du buffer
  `streamingMap[discId]` en message Agent dans `loadedDiscussions`
  AVANT le flip de `sending`. La bulle de streaming démonte au
  même render où la bulle persistée monte — pas de gap.
- **Bootstrap MCP override du choix utilisateur** — l'effet
  d'auto-pick listait `bootstrapRepoMcp` dans ses deps, donc
  choisir « pas de repo » (value=`""`) re-fire l'effet, le guard
  `!bootstrapRepoMcp` repassait, et le dropdown re-snappait sur le
  premier MCP. Fix : `useRef` one-shot par modal-open.
- **`NewDiscussionForm` Ctrl+Enter** — insérait un `\n` dans le
  textarea ET soumettait le formulaire, parce que le `onKeyDown`
  card-level n'appelait pas `e.preventDefault()`. Le prompt
  envoyé contenait un retour-ligne parasite.
- **`NewDiscussionForm` profils auto-sélectionnés** — l'effet
  `prefill` faisait `setNewDiscProfileIds(['architect',
  'tech-lead', 'qa-engineer'])` pour TOUT prefill, pas seulement
  les audits de validation. Les utilisateurs partaient en chat
  unrelated avec les 3 profils validateurs sélectionnés. Fix :
  bound to `prefill.locked === true`.
- **`NewDiscussionForm` wedge après échec submit** — `onSubmit`
  typé `=> void` non await, `setCreating(true)` jamais reset,
  bouton Create disabled à vie. Fix : `await Promise.resolve(onSubmit())`
  + `try/catch/finally` + `creatingRef`.
- **`ChatInput` hauteur du textarea** — `updateChatInput('')` à
  l'envoi vidait la valeur mais laissait `style.height = '120px'`,
  composer restait à 4-5 lignes vides après chaque message.
  `updateChatInput` re-snappe désormais la hauteur via le même
  calcul que `onChange`.
- **IME composition non gardé sur les 4 textareas de discussion**
  — Enter pressé pour valider une candidature IME envoyait le
  message à mi-mot. Fix : `!e.nativeEvent.isComposing` sur
  `ChatInput`, `NewDiscussionForm`, `MessageBubble` edit,
  `ApiCallAiHelper`.
- **Tour backdrop click marquait le tour terminé** — un clic à
  côté du tooltip persistait `kronn:tour-completed = "true"` pour
  toujours. Backdrop devient passif ; seuls les boutons explicites
  + Escape comptent comme dismissals.
- **MCP delete config sans confirmation** — `mcp-btn-action` rouge
  appelait `deleteConfig` sur le clic, sans `confirm()` ni toast.
  Une mauvaise click détruisait clés env + projets liés +
  contextes custom sans signal. Fix : confirm natif + toast
  succès/erreur.
- **`AgentsSection` model_tiers init incomplet** — la boucle
  d'initialisation listait 5 agents sur 7 (`copilot_cli` et
  `ollama` absents). Leurs dropdowns restaient vides même quand le
  backend avait des valeurs sauvegardées, et le prochain save
  écrasait l'autre tier. Fix : 7 agents.
- **Workflow trigger race** — `disabled={triggering === wf.id}`
  closure-stale, double-click → 2 `triggerStream` en parallèle
  (donc 2 runs du même workflow). Ajout `triggeringRef`.
- **Voice countdown setInterval leak** —
  `ChatInput.tsx` ouvrait un timer 1 Hz sans return cleanup.
  Quitter mid-countdown laissait l'interval tourner et `setState`
  sur un composant démonté. Cleanup ajouté.

### Added

- **Confirmations de suppression manquantes** — workflows
  (carte + delete-all-runs + run individuel), contacts. Ajout des
  i18n keys `wf.deleteWorkflowConfirm`, `wf.deleteRunConfirm`,
  `wf.deleteAllRunsConfirm`, `contacts.deleteConfirm` en FR/EN/ES.
- **E2E spec pour le tour guidé** — `e2e/specs/guided-tour.spec.ts`
  couvre auto-launch, skip persistance, navigation, Escape,
  replay via le bouton « ? », resume mid-flow.
- **Tests de régression** — 12 nouveaux specs unit dont
  `WorkflowsPage.qp-launch.test.tsx` (race + multi-var batch),
  `Dashboard.bootstrap-mcp.test.tsx` (override MCP),
  `ProjectCard.briefing-race.test.tsx` (double-click briefing),
  delete-workflow-confirm dans `WorkflowsPage.test.tsx`.

### Internal

- `vitest.config.ts` exclut désormais `node_modules_old/**`
  (cache pnpm renommé manuellement par certains devs en local).
  `pnpm test` reste vert même quand ce dossier de 100+ MB existe.
- **Lint baseline level-up** — passé de `57 errors / 125 warnings`
  à `0 errors / 102 warnings`. Les corrections clés :
  - `eqeqeq: ['error', 'always', { null: 'ignore' }]` — autorise
    le `== null` idiomatique pour le check null-or-undefined sans
    relâcher `===` ailleurs.
  - 6 `consistent-type-imports` — `import('../types/...').X` →
    imports nommés depuis le bundle généré (`api.ts`,
    `WorkflowDetail`, `DiscussionsPage`, `McpPage`,
    `SetupWizard.test`).
  - 6 `no-empty` catch — commentaires explicites sur ce que le
    catch silencieux protège (incognito storage, blip réseau, …).
  - 4 `no-unused-expressions` — ternaires conditionnels qui
    renvoyaient une valeur jetée → `if/else` propres.
  - 8 `no-non-null-assertion` dans `ProjectCard` (drift status) +
    6 dans `GitPanel` (projectId optional) → narrowing via
    `&& condition && (...)` ou `if (!x) return`.
  - 16 `no-explicit-any` dans `MessageBubble.mdComponents` →
    interface `MdProps` typée (children/href/className).
  - 6 `no-explicit-any` sur les signatures `t: (...args: any[])` →
    `t: (...args: (string | number)[])` partout.
  - Règles strictes React 19/20 (`react-hooks/purity`,
    `immutability`, `refs`, `set-state-in-effect`,
    `preserve-manual-memoization`) demoted `error → warn` le
    temps de la migration. À traiter par dossier dans
    `docs/tech-debt/`.
- **Backend `cargo clippy` filtré** — nouveau target
  `make lint-backend-local` qui pipe `cargo clippy --all-targets`
  à travers awk pour masquer les 270 warnings ts-rs « failed to
  parse serde attribute » (upstream issue, ts-rs 9.x ignore les
  combinaisons d'attributs comme `skip_serializing_if =`,
  `deserialize_with =` qu'il ne sait pas parser — ne change rien
  à la TS générée). Le target Docker `make lint-backend` reste
  intact pour CI.
- **Mise à jour packages + audit sécurité** :
  - Frontend : `pnpm update` sur les patch/minor sûrs — vitest
    4.1.4 → 4.1.5, react/react-dom 19.2.5 → 19.2.6, eslint
    10.2 → 10.3, typescript-eslint 8.58 → 8.59,
    eslint-plugin-react-hooks 7.0 → 7.1.
  - **Tous les majors aussi appliqués** dans cette même passe :
    - **`typescript` 5.9 → 6.0** — TS 6 active strict-mode sur
      les side-effect imports (`import './foo.css'`). Ajouté
      `src/vite-env.d.ts` avec `declare module '*.css'` (+ `*.svg`,
      `*.png`, etc.) pour réintégrer ces imports proprement.
      Aucune erreur d'inférence introduite par le bump.
    - **`lucide-react` 0.441 → 1.14** — la 1.x supprime les
      brand icons (`Github`, `Gitlab`, …). Remplacé l'unique
      usage (`<Github />` dans le bouton "Report Bug" de
      `DebugSection`) par `<ExternalLink />`, qui est plus
      sémantiquement correct pour une CTA qui ouvre une URL.
    - **`vite` 6.4 → 8.0** + **`@vitejs/plugin-react` 5 → 6** —
      vite 8 utilise rolldown au lieu de rollup. La forme objet
      `manualChunks: { name: [...] }` n'est plus supportée ;
      converti en form fonction qui regarde l'`id` du module.
      Build temps : 7.9s → **1s** (-87 %), gros gain en local.
      Le warning « chunk > 500 KB » est conservé sur le Dashboard
      mais c'est cosmétique — `rolldown` recommande
      `build.rolldownOptions.output.codeSplitting` pour aller plus
      loin (à faire dans une passe dédiée).
    - **`react-router-dom` 6.30 → 7.15** — découvert via grep que
      la lib n'est utilisée nulle part dans `src/`. Dead dep
      retirée du `package.json` (et du `manualChunks`
      `vendor-react`).
    - **`@huggingface/transformers` 3.8 → 4.2** — affecte
      uniquement `lib/stt-worker.ts` (Whisper STT). Les types
      `pipeline`, `AutomaticSpeechRecognitionPipeline`,
      `AutomaticSpeechRecognitionOutput` sont conservés à
      l'identique. Tests pass, type-check propre.
  - **0 package outdated** maintenant. Tout au dernier semver.
  - `pnpm overrides` ajoutées pour 4 vulnérabilités transitives :
    `postcss >=8.5.10` (XSS via vite), `flatted >=3.4.2`
    (proto-pollution via eslint), `brace-expansion >=5.0.5`
    (DoS via eslint), `protobufjs >=7.5.5` (RCE critique via
    `@diffusionstudio/vits-web → onnxruntime-web`). Chaque entry
    est commentée avec son GHSA pour qu'on sache quand la
    retirer (= quand l'upstream direct accepte la version
    patchée). `pnpm audit` est désormais clean.
  - Backend : `cargo update` sur les semver-compat — tokio 1.50
    → 1.52, rustls 0.23.37 → 0.23.40, tower-http 0.6.8 →
    0.6.10, plus 35 autres patches.
  - Desktop tauri : `cargo update` aligné sur le même cycle.
  - 1850 tests Rust verts, 1070 tests vitest verts, 35 tests
    Playwright E2E verts, build vite OK (Dashboard chunk
    949 → 77 KB après code-split par tab).
  - **Code-split du Dashboard (vite 8 rolldown)** — `Dashboard.tsx`
    importait directement `McpPage`, `WorkflowsPage`, `SettingsPage`,
    `DiscussionsPage`, ce qui empilait 949 KB dans le chunk d'entrée.
    Converti en `React.lazy()` + `<Suspense>` autour de chaque tab.
    Résultat : Dashboard 77 KB, McpPage 32 KB, WorkflowsPage 234 KB,
    SettingsPage 113 KB, DiscussionsPage 480 KB. Plus aucun chunk
    au-dessus du seuil 500 KB de rolldown. Premier switch d'onglet =
    fetch ~100 ms du chunk concerné, ensuite caché par le nav.
  - **`useAsyncGuard` hook** — extraction du pattern `useRef` +
    `try/finally` partagé par 13 sites de garde de re-entrée. Hook
    `useAsyncGuard(asyncFn)` retourne un callback qui short-circuite
    la 2e invocation tant que la 1re est in-flight. 4 tests verts.
    Pattern documenté dans `feedback_race_guards` memory et utilisé
    dans `DiscussionSidebar.handleContactAdd`,
    `AgentsSection.handleInstallAgent`, `ProjectSkills.handleToggle`.
  - **3 nouvelles gardes de re-entrée** (en plus des 9 déjà
    fixées) :
    - `DiscussionSidebar.handleContactAdd` — Enter rapide ou
      double-clic sur Add créait des contacts dupliqués (POST
      `/api/contacts` non-idempotent).
    - `AgentsSection.handleInstallAgent` — `disabled` lit
      `installing !== null` qui est closure-stale ; deux clics
      rapides lançaient deux installs en parallèle.
    - `ProjectSkills.handleToggle` — toggle d'un skill à un
      double-clic sur la même chip envoyait deux POST
      `setDefaultSkills` avec un état incohérent.
  - **Bug HTML : `<button>` imbriqué dans `<button>`** — la carte
    projet (`ProjectCard.tsx`) avait `dash-card-header` (un
    `<button>` qui toggle l'expand) qui contenait
    `dash-drift-update-btn` (un autre `<button>` pour relancer un
    audit partiel). Invalid HTML, warning React en dev, et le clic
    sur l'inner button bubblait au header sur certains navigateurs.
    Fix : header converti en `<div role="button" tabIndex={0}>`
    avec `onKeyDown` Enter/Space explicites. 5 tests d'accessibility
    pinnent le contrat (focus, Enter, Space, autres touches no-op).
  - **4 bugs UTF-8 « slice par byte index »** — pattern
    `&s[..N]` qui panique si l'octet N tombe au milieu d'une
    séquence UTF-8 (très réel sur du français « été », emoji,
    noms de fichiers accentués). Sites identifiés et fixés :
    - `api/disc_git.rs::format_tool_log` → tronquait les commandes
      Bash longues à 80 octets ; agent qui logguait
      `git commit -m "feat: été pré-prod 🚀…"` panique.
    - `workflows/steps.rs:159` → tronquait l'output d'un step à
      2000 octets pour le repair-prompt si validation foirait.
    - `api/workflows.rs:1051` → tronquait `rendered` à 200 octets
      pour le message d'erreur "items_from resolved to empty".
    - `api/discussions/streaming.rs:696,1083` → tronquait
      `stderr_text` agent à 500 octets dans les logs d'erreur.
    Fix uniforme : `s.chars().take(N).collect::<String>()`.
    12 tests dont une regression spécifique pour byte 80 = milieu
    d'emoji et 280 octets / 160 chars de français. Pattern
    documenté dans `feedback_rust_str_slicing` memory.
  - **Tests E2E Playwright stabilisés** — 4 tests
    `guided-tour.spec.ts` flakaient à cause de :
    1. Le `TourProvider` est mounté DANS le Dashboard, donc
       l'auto-launch (timer 800 ms) part seulement après le
       render du Dashboard, qui peut prendre 2-5 s avec >1000
       discussions en DB. Timeout test 2.8 s → trop court.
       Fix : `waitForDashboardMounted()` puis timeout 4.8 s.
    2. Le `addInitScript` du `beforeEach` re-supprimait le flag
       `kronn:tour-completed` à CHAQUE navigation, donc après un
       Skip + reload le tour réapparaissait. Fix : `freshTourState()`
       qui fait `goto('/') + evaluate(removeItem) + reload()`
       — un seul nettoyage avant le test, pas un init script qui
       re-tourne.
  - **E2E QP launch double-click** — nouveau spec
    `qp-launch-double-click.spec.ts` : navigue Workflows → Quick
    Prompts, intercepte `POST /api/discussions` (résolu
    manuellement), dispatch deux `MouseEvent('click')` synchrones
    sur le bouton Launch via `page.evaluate` (les browsers ne
    dispatchent pas le click sur `<button disabled>`, mais on
    veut tester le pattern bug pré-fix où la 2e click arrive
    avant le re-render du disabled). Assert exactly 1 hit. Skip
    si la dev DB n'a aucun QP.
  - **Backend majors aussi appliqués dans cette passe** (15
    crates) :
    - **`ts-rs` 9 → 12** — fix ~270 warnings « failed to parse
      serde attribute » qui empoisonnaient `cargo build`. Reste
      1 warning isolé sur `deserialize_with` (combo non supportée
      upstream, pas bloquant).
    - **`thiserror` 1 → 2**, **`directories` 5 → 6**, **`which`
      6 → 8**, **`cron` 0.12 → 0.16**, **`governor` 0.8 → 0.10**,
      **`tokio-tungstenite` 0.26 → 0.29** — bumps semver propres,
      0 changement requis dans le code.
    - **`sha2` 0.10 → 0.11** — `Digest::finalize()` retourne
      désormais un `hybrid_array::Array<u8>` qui n'implémente
      plus `LowerHex`. Patch sur 2 sites (`core/checksums.rs`,
      `api/themes.rs`) : encodage manuel via
      `result.iter().map(|b| format!("{:02x}", b)).collect()`.
    - **`reqwest` 0.12 → 0.13** — la feature `rustls-tls` est
      renommée `rustls`, et `form` est désormais une feature à
      part. Ajusté dans `Cargo.toml`.
    - **`toml` 0.8 → 1.1** — gros breaking : `parse::<toml::Value>()`
      ne marche plus pour parser un document complet (Value est
      réservé aux primitives). Migré 4 sites
      (`core/mcp_scanner.rs`, `core/host_mcp_discovery.rs`,
      `api/mcps.rs`, `core/mcp_scanner_test.rs`) vers
      `parse::<toml::Table>()`.
    - **`tower-http` 0.5 → 0.6** — déjà appliqué via cargo
      update, aucune incompat.
    - **`axum` 0.7 → 0.8** — 3 breaking changes traités :
      - **Path param syntax** : `:id` → `{id}`, `:run_id` →
        `{run_id}`, etc. 84 routes converties dans `lib.rs` +
        1 dans `workflows/notify_step.rs` (test).
      - **`Message::Text` wraps `Utf8Bytes`** au lieu de
        `String`. Patch dans `api/ws.rs` :
        `Message::Text(json.into())`.
      - **`Option<T>` en extractor exige
        `OptionalFromRequest{Parts}`**. axum 0.8 ne l'implémente
        plus pour `Query` ni `ConnectInfo`. Fixes :
        - `discussions::list` : remplacé
          `Option<Query<PaginationQuery>>` par
          `Query<PaginationQuery>` avec `page=0` comme sentinelle
          « pas de pagination » (cf. doc-comment de
          `PaginationQuery`).
        - `ws::ws_handler` : remplacé `Option<ConnectInfo<…>>`
          par `Option<Extension<ConnectInfo<…>>>` (Extension
          implémente `OptionalFromRequestParts`).
    - **`calamine` 0.26 → 0.34**, **`pdf-extract` 0.7 → 0.10** —
      bumps propres, pas de changement de code.
    - **`rusqlite` 0.31 → 0.39** — drop le support de `u64` dans
      `ToSql`/`FromSql` (SQLite stocke en i64 de toute façon).
      Patch sur `db/quick_apis.rs` (3 sites
      `qa.api_timeout_ms` : cast `Option<u64>` ↔ `Option<i64>`
      à la frontière SQL).
    - **`zip` 2 → 8** : skip — la 9 est encore en pre-release et
      la 2.4.2 est la dernière stable. On reste sur 2.
  - **Desktop tauri** également bumpé sur axum 0.8 +
    tower-http 0.6 pour matcher le backend.
  - **1838 tests Rust verts** après chaque bump (test runs
    intermédiaires entre chaque crate). 0 régression.

---

## [0.7.1] - 2026-05-07

**Convention pivot, host-sync resilience, install fixes** — la 0.7.1
boucle ce qui restait de 0.7.0 (jamais livrée) et y ajoute la pivot
de la convention de doc agent (`ai/` → `docs/AGENTS.md`), un cycle
complet de durcissement du host-sync MCP (race condition, concurrent
writes, gate workflow-run) et la fermeture des bugs d'install bloquants
remontés par les premiers utilisateurs macOS / WSL2.

### Added

- **Pivot `ai/` → `docs/AGENTS.md`** — la convention multi-agent
  `AGENTS.md` à la racine devient la norme : `ai/index.md` est
  renommé `docs/AGENTS.md`, `ai/templates/` → `docs/templates/`, le
  loader Tier 1/2/3 est préservé exactement (token economy
  intacte). Les redirecteurs racine (`CLAUDE.md`, `GEMINI.md`,
  `.cursorrules`, `.windsurfrules`, `.clinerules`,
  `.kiro/steering/instructions.md`, `.vibe/instructions.md`,
  `.github/copilot-instructions.md`, `.cursor/rules/repo-instructions.mdc`)
  sont raccourcis à des stubs qui pointent tous vers
  `docs/AGENTS.md`. Helpers `core::scanner::detect_docs_dir` /
  `detect_docs_entry` rendent tout le backend path-agnostique
  (`docs/` > `doc/` > legacy `ai/`).
- **Migration `ai/ → docs/`** — `POST /api/projects/:id/migrate-docs`
  exécute un `git mv` propre (préserve l'historique), renomme
  `index.md` → `AGENTS.md`, réécrit les liens internes et les
  redirecteurs racine, et option de symlink `ai → docs` pour la
  rétro-compat. Détecte automatiquement les conflits de fusion
  (même nom de fichier, contenu différent) et abort avec une liste
  précise. Bouton "Migrer vers docs/" sur `ProjectCard` quand le
  projet est encore sur `ai/`.
- **`docs/index.md` humain** — landing page lisible pour les humains
  qui ouvrent `docs/` sur GitHub, à côté de l'AGENTS.md destiné aux
  LLM. Auto-générée par `core::docs_migration::ensure_docs_index`
  en fonction des sous-dossiers réellement présents
  (`conventions/`, `gotchas/`, `people/`, `architecture/`,
  `operations/`, `tech-debt/`, `decisions/`, `templates/`). Backfill
  silencieux pour les projets migrés AVANT cette feature.
- **Cross-project user context (`~/.kronn/user-context/*.md`)** —
  fichiers markdown auto-injectés dans le system prompt de TOUS les
  agents, peu importe le CLI ou le projet. Pont sur les gaps des
  conventions per-tool (`~/.claude/CLAUDE.md`, `~/.codex/...`).
  Auto-bootstrap d'un README au premier lancement. Éditeur inline
  dans la page Settings ("Mes contextes") — CRUD complet sans
  ouvrir un terminal.
- **Anti-secret filter sur les écritures `docs/`** — `core::docs_write_filter`
  passe chaque écriture d'agent dans `docs/` à travers une combo
  (denylist regex + détecteur d'entropie + Bloom-style sensitive-substring
  filter contre `~/.env`, `*.pem`, `*.key`, etc.). Les écritures
  rejetées sont auto-revertées via `git checkout` ou supprimées si
  untracked. Audit post-step automatique dans `workflows/runner`.
- **`HostMcpSync` trait + `run_host_sync` driver** — le boilerplate
  de sync vers les configs MCP des CLIs (Codex `config.toml`,
  Copilot `mcp-config.json`, Claude `~/.claude.json` scope-aware,
  Gemini `~/.gemini/settings.json`) est maintenant centralisé. Une
  5ème CLI = un struct + une ligne dans le slice. Code :
  `backend/src/core/mcp_scanner.rs::HostMcpSync`.
- **Watchdog stale-stream (TD-20260504)** — `Dashboard.tsx` détecte
  via `lib/stream-watchdog.ts::detectStaleStreams` (helper pur,
  unit-testé) les discussions dont le spinner spinne plus de 5 min
  sans nouvel chunk. Auto-recovery : clear spinner + refetch des
  messages persistés + toast warning. Plus jamais de F5 obligatoire
  après un `docker compose restart`.
- **Auto-resync MCP host-config gated par les workflow runs**
  (TD-host-sync-workflow-race) — `db::workflows::has_running_run`
  bloque l'écriture des configs `~/.claude.json` / `~/.gemini/settings.json`
  pendant qu'un agent est mid-spawn. Évite les race entre Kronn qui
  réécrit la config et l'agent qui la lit au démarrage.
- **Concurrent-writer guard sur les host syncs** (TD-host-sync-flock)
  — `atomic_write_checked` snapshote la mtime avant la lecture et
  abort le rename si un tiers (Claude Code lui-même, Gemini CLI, …)
  l'a touché entre-temps. Préserve les edits concurrents au lieu de
  les clobber.
- **`kronn doctor`** — commande de diagnostic CLI : détecte les
  fichiers root-owned uid-0 sous `~/.cache` et `~/.local/share`
  (héritage pré-`APP_UID`, source silencieuse de `uvx Permission
  denied` côté hôte), valide `uvx` / `glab ≥ 1.59` / `npx` dans le
  PATH, vérifie que Docker tourne. Exit non-zéro sur souci. Doc :
  `docs/operations/host-mcp-runtime.md`.
- **Warning sync-time sur binaires hôte manquants** —
  `mcp_scanner::warn_missing_host_binaries` scanne les configs en
  cours de sync et log un warn par command stdio (`uvx`, `glab`,
  `npx`, …) absent de `/host-bin/*`. Surface "uvx pas dans le PATH"
  au moment où l'opérateur sauve la config, pas au moment où l'agent
  hôte essaie de spawn le MCP six heures plus tard.
- **Tooling restant accumulé depuis 0.6.0** : workflow Loop / Exec /
  Rollback, RTK gain analytics, sélecteur de thème
  (light/dark/system), intégration Ollama locale,
  génération de fichiers (PDF/DOCX/XLSX/CSV/PPTX) via sidecar
  Python, step `ApiCall` qui remplace les MCPs read-only,
  template AutoPilot, tour guidé d'onboarding, batch quick-prompts,
  data profiles, glossaire global, OAuth2 Tauri, export/import ZIP
  cross-OS, helpers Chartbeat API, favoris discussions, fix scan
  path Windows WSL depuis Tauri.

### Fixed

- **Install macOS premier lancement** (issue #84, paul-horcholle) :
  RTK pin `0.37.1 → 0.39.0` (release supprimée upstream),
  arm64 target `aarch64-unknown-linux-musl → -gnu` (variant musl
  plus shipée), Makefile refuse explicitement quand Kronn est
  cloné directement sous `$HOME` (collision avec le mount RO
  `/host-home`), backend healthcheck `start_period 5s → 60s`
  (cold start ~30 s avec migrations + sidecars + tool installs).
- **Install WSL2 + agents** (issue #81, smarguin) : self-mount
  `${HOME}/.local/share` au même chemin host pour résoudre les
  symlinks absolus d'installeurs (Claude / Vibe), volume nommé
  `claude-sessions` pour isoler les PIDs du daemon hôte (silent
  hang fixé), `ws.onopen` envoie maintenant `Presence` immédiatement
  (le backend reste plus tolérant aux Pings pré-Presence en plus,
  pour les reconnects post-suspend).
- **HOME override pour les agents CLI** : `agents/runner.rs`
  n'écrase plus `HOME` pour Claude / Codex / Vibe / Gemini /
  Kiro-CLI / Copilot. Leurs configs sont montées à
  `/home/kronn/<agent>` et l'override les renvoyait vers
  `/home/<host-user>/<agent>` qui n'existe pas dans le container —
  silent hang en attendant un token d'auth jamais trouvé. Les
  binaires inconnus gardent l'override (besoin éventuel d'un HOME
  host-rooted).
- **Tech-debt fermées au passage** :
  `TD-20260427-host-sync-trait` (trait `HostMcpSync`),
  `TD-20260427-host-sync-flock` (mtime CAS guard),
  `TD-20260427-host-sync-workflow-race` (gate via
  `has_running_run`), `TD-20260504-ws-reconnect-stale-ui` (watchdog
  + handshake tolérant), `TD-20260427-host-sync-backup-rotation`
  (rotation `.1`→`.5` automatique).

### Changed

- **Path-agnostique partout** dans le backend : `count_ai_todos`,
  `core::checksums` (read/write `checksums.json`), `core::mcp_scanner`
  (`ensure_redirectors`), `api::projects::resolve_briefing_notes`,
  `api::discussions` (briefing read), `api::mcps` (checksum
  invalidation), `api::workflows` (mcp hints), `api::ai_docs`
  (list/search/read), `api::audit` (compute_audit_info_sync,
  cleanup), `inject_bootstrap_prompt`, `install_template`. Plus
  aucun `ai/` hardcodé en code de production.
- **Templates restructurés** : `templates/ai/` → `templates/docs/`
  (git mv préserve l'historique), `templates/docs/AGENTS.md` au
  lieu de `index.md`, tous les redirecteurs racine raccourcis à
  ~3 lignes pointant sur `docs/AGENTS.md`. `templates/.env.mcp.example`
  référence `docs/operations/...`.
- **Auto-bootstrap subfolders** : `docs/conventions/`,
  `docs/gotchas/`, `docs/people/` créés au bootstrap avec un
  README explicatif, agent-writable.
- **Project model** : nouveau champ `needs_docs_migration: bool`
  (calculé par `enrich_audit_status`, non persisté en DB). Drive
  l'apparition du banner de migration sur `ProjectCard`.
- **`backend/src/models/mod.rs` éclaté** (TD-20260417-models-monolith,
  ✅ FIXED) : 3272 L → 151 L. 12 sous-modules par domaine
  (`agents`, `db`, `discussions`, `git`, `mcp`, `multiuser`,
  `ollama`, `projects`, `quick`, `setup`, `stats`, `workflows`)
  re-exportés via `pub use sub::*`. Aucun call site touché —
  `use crate::models::Foo` continue de résoudre. Bindings ts-rs
  identiques à l'octet près sauf `Project.ts` (champ
  `needs_docs_migration` qui attendait juste un `make typegen`).
- **`backend/src/api/projects.rs` éclaté** (TD-20260417-projects-monolith,
  ✅ FIXED) : 2038 L → 7 fichiers (`mod.rs`, `crud.rs`, `bootstrap.rs`,
  `clone.rs`, `template.rs`, `git.rs`, `migrate.rs`). Les 4 helpers
  `pub(crate)` réutilisés par `api::audit` (template install /
  bootstrap prompt) restent re-exportés via `pub use template::*`.
  Aucune route à toucher dans `lib.rs`, surface externe identique.
- **`backend/src/api/audit.rs` éclaté** (TD-20260417-audit-monolith,
  ✅ FIXED) : 1988 L → 8 fichiers (`mod.rs` avec les constantes
  `PROMPT_PREAMBLE`/`ANALYSIS_STEPS`/`AUDIT_REDIRECTOR_FILES` +
  `helpers.rs`, `run.rs`, `info.rs`, `drift.rs`, `validate.rs`,
  `full.rs`, `briefing.rs`). Hypothèse initiale d'une abstraction
  `AuditEngine` requise avant split a été levée — le couplage est
  via `AppState.audit_tracker`, chaque handler est self-contained.
  `pub(crate) use helpers::{check_ai_dir_permissions, detect_project_skills}`
  pour les 2 helpers consommés par `api::projects::*`.
- **`backend/src/api/discussions.rs` éclaté** (TD-20260328-discussions-backend,
  ✅ FIXED) : 3222 L → 7 fichiers (`mod.rs` avec constantes
  + `TERMINAL_SIGNALS` + `detect_terminal_signal` /
  `truncate_after_signal` / `message_matches_silent_crash` +
  `AgentStreamEvent` enum + tests, puis `crud.rs`, `messaging.rs`,
  `runtime.rs`, `streaming.rs`, `orchestration.rs`, `context.rs`).
  Le couplage SSE/streaming/cancel évoqué dans le TD original
  (« separate session needed ») se résout par une visibilité
  `pub(super)` propre — `make_agent_stream`, `run_agent_streaming`,
  `run_agent_collect`, `AgentStreamMeta`, `AgentRunResult`,
  `maybe_generate_summary` sont consommés par les modules siblings
  via `super::streaming::*` / `super::orchestration::*`. Aucune
  signature publique modifiée, callers (`workflows::batch_step`,
  `src/api_tests.rs`) intacts.

---

## [0.6.0] - 2026-05-03

**Modularité unitaire des workflows** — trois briques pour factoriser :
réutiliser un Quick Prompt / Quick API depuis un step au lieu de
dupliquer la config, et alimenter un workflow batch sur une liste
figée sans monter d'API. La désagentification poussée d'un cran : le
runtime sait maintenant charger une config canonique au run-time, le
wizard la pointe en deux clics.

### Added
- **`StepType::JsonData` — source de données déterministe** — émet un
  payload JSON littéral comme envelope Structured. Zéro token, zéro
  réseau. Use case canonique : alimenter un `BatchQuickPrompt` /
  `BatchApiCall` sur une liste figée (10 hosts en dur, 5 régions, 3
  envs) sans monter une API juste pour tenir la liste. Aussi : fixture
  de dev — on construit le pipeline sur du JsonData puis on remplace
  par un `ApiCall` quand la vraie source est prête. Validation au save
  (payload non-null + ≤ 1 MiB). Pas de templating runtime — la valeur
  est retournée littéralement, ce qui élimine l'ambiguïté
  "est-ce que `{{var}}` a été substitué ?". Code : `backend/src/workflows/json_data_step.rs`.
- **`quick_prompt_id` sur `StepType::Agent`** — référence vers un Quick
  Prompt sauvegardé. Le runner charge `prompt_template`, `tier` et
  `skill_ids` du QP via `quick_prompt_hydrate::hydrate_step_from_quick_prompt`.
  Per-field override : si le step a son propre `prompt_template`
  non-vide, il gagne. Pas de variables au niveau step — les `{{var}}`
  du QP sont résolus avec le `TemplateContext` du workflow (launch
  variables / state / previous_step / steps.X). Pattern : un QP
  canonique réutilisé dans N workflows. Wizard expose un picker
  "Depuis un Quick Prompt existant" + bandeau "🔗 Hérité de {QP}" dans
  la step Agent card.
- **`quick_api_id` étendu à `StepType::ApiCall` single** (initialement
  disponible uniquement sur `BatchApiCall`). Le runner appelle
  `quick_api_hydrate::hydrate_step_from_quick_api` au début de
  `execute_api_call_step_with_db` ; per-field override identique au
  pattern batch. Wizard `ApiCallStepCard` expose le picker QA + bandeau
  d'héritage. Validation : un step ApiCall accepte SOIT
  `quick_api_id`, SOIT (`api_plugin_slug` + `api_config_id` +
  `api_endpoint_path`).
- **Préset `daily-host-audit`** (`v07-presets.ts`) — démontre la combo
  `JsonData → BatchQuickPrompt → Notify` avec 5 hosts pré-câblés.
  L'utilisateur édite la liste + picke son QP audit.

### Changed
- Le helper d'hydratation QA est extrait dans
  `backend/src/workflows/quick_api_hydrate.rs` (auparavant inliné dans
  `batch_apicall_step.rs`). Réutilisé par single + batch sans
  divergence.
- `workflow-architect.md` mis à jour — nouvelles entrées dans le
  decision tree (`JsonData` en n°1, mention `quick_prompt_id` /
  `quick_api_id` dans les sections Agent / ApiCall / BatchApiCall).
- **Présets `AUTO_DEV` et `PR_GATE`** — le step `fetch_issue` démarre
  maintenant en `JsonData` (fixture avec un ticket exemple) au lieu
  d'un `ApiCall` blank. Le préset tourne immédiatement après création,
  sans plugin tracker installé. Description du step explicite : *"Édite
  le payload, ou switch en `ApiCall` quand tu auras un plugin tracker."*
  La variable launch `issue_key` est retirée du préset par défaut (pas
  utilisée en mode JsonData) ; le scanner live-warning du wizard la
  recrée automatiquement si l'utilisateur swap en ApiCall et tape
  `{{issue_key}}`.
- **Picker QP / QA dans le wizard** — quand un Quick Prompt /
  Quick API est référencé, le bandeau d'héritage est enrichi
  (preview du prompt template + variables QP, ou plugin/method/endpoint/extract
  du QA) et les fields override sont enroulés derrière un disclosure
  "✏️ Personnaliser pour ce step". Le disclosure s'ouvre auto si un
  override est déjà rempli ; un badge `🔓 override actif` apparaît dans
  le bandeau. Évite la confusion "il faut tout remplir" sur les fields
  qui ne sont pas pertinents quand un QP/QA fournit déjà la config.
- **Boutons step type homogénéisés** — `BatchQuickPrompt` (violet),
  `BatchApiCall` (bleu info), `Notify` (vert succès), `JsonData` (cyan)
  ont maintenant un variant sélectionné coloré, comme `Agent` /
  `ApiCall` / `Gate` / `Exec` qui en avaient déjà un. Le user voit
  clairement quel type est actif sur chaque step.
- **Audit + nettoyage des présets** : description shared `runTestsDesc`
  explique l'adaptation Rust/Node/Python/Make ; `DEPLOY_ROLLBACK`
  `exec_allowlist` réduit à `['make']` (cargo/npm/kubectl orphelins
  retirés). Le tableau de résultats du batch Quick API affiche
  maintenant le résultat complet sur clic d'une ligne (déplie en
  pre-wrap + JSON pretty-print, plusieurs lignes ouvertes en parallèle
  pour comparer).

### Fixed
- **CI lint** — la règle `no raw Command::new in prod code` ignorait
  les commentaires. Faux positifs sur `exec_step.rs` (doc comment
  qui mentionne `Command::new`) et `models/mod.rs` (idem). La règle
  CI skip maintenant les lignes commentaires (`^[0-9]+:[[:space:]]*//`)
  et les commentaires inexacts ont été reformulés (le code utilise
  `cmd::async_cmd`, pas `Command::new`).
- **Quick APIs — onglet Automatisation** — l'onglet `quickApis`
  affichait les boutons d'action de l'onglet Quick Prompts (ternaire
  buggé) et son bouton "Créer" était dans le contenu. Header aligné
  sur les deux autres onglets ; le bouton vit dans le header, masqué
  tant qu'aucun plugin API n'est câblé.
- **Wizard "Create" silencieux** — le predicate `disabled` du bouton
  Create n'avait pas été synchronisé avec les nouveaux step types : un
  step `JsonData` sans `prompt_template` tombait dans le bras
  `else if (!s.prompt_template) return true` → bouton désactivé sans
  feedback. Predicate refait avec un cas par step type (`JsonData` :
  payload non-null ; `ApiCall` / `BatchApiCall` : `quick_api_id` OU
  triplet plugin/config/endpoint ; `Agent` : `prompt_template` OU
  `quick_prompt_id`). En complément : `handleSave` affiche maintenant
  un bandeau d'erreur rouge (au lieu d'un `console.warn` invisible)
  quand le backend rejette un workflow — l'utilisateur voit le message
  exact (ex : *"L'étape 'fetch_issue' est en output_format: FreeText"*).
- **Validator backend `validate_step_references`** — ne reconnaissait
  un producer Structured que pour `Agent` avec `output_format:
  Structured`, alors que tous les autres step types (`Notify`,
  `ApiCall`, `Gate`, `Exec`, `BatchApiCall`, `BatchQuickPrompt`,
  `JsonData`) émettent toujours une envelope Structured au runtime.
  Refactor avec un helper `produces_structured(step)` qui débloque les
  workflows qui consommaient `{{steps.fetch_issue.data}}` après un
  step JsonData.
- **JsonData step affichait le label "Claude Code"** dans le détail du
  workflow — `isAgentLike` était calculé en négation et n'excluait pas
  les nouveaux step types (`BatchApiCall`, `JsonData`). Refactor en
  whitelist explicite (`isAgentLike = step.step_type.type === 'Agent'`)
  pour rendre le bug par omission impossible. Ajout de badges dédiés
  pour `BatchApiCall`, `Gate`, `Exec`, `JsonData` dans le récap workflow.

### Tests
- Backend : **1369 tests** (+11 net : 5 hydrate QA, 6 hydrate QP, 5 JsonData, recouvrements).
- Frontend : **934 tests passent**.
- Pas de migration SQL — les nouveaux champs `quick_prompt_id` et
  `json_data_payload` sont `Option<...>` avec `serde(default)`, les
  workflows existants se rechargent inchangés.

---

## [0.5.1] - 2026-04-25

Polish + playful additions on top of 0.5.0: the light theme got a proper
expert-led rework, a hidden-unlock system was added for early-access
testing, three secret themes ship with a small interactive layer, agents
can now generate PDF / DOCX / XLSX / CSV / PPTX files directly from the
conversation without the user installing anything, three user-facing
hygiene wins land (in-nav "active runs" popover, first-class RTK
integration cutting agent shell outputs by ~90%, toast refactor for
copyable sticky errors), and the release **kicks off the désagentification
push** with a first-class `StepType::ApiCall` that runs a whole API-backed
workflow step — request, extract, pipe — from the Rust engine, zero
agent tokens consumed.

### Added
- **`StepType::ApiCall` — désagentification's first vertical** — a
  workflow step that hits a Kronn-configured API plugin directly from
  the Rust engine and pipes the extracted JSON to the next step
  (Agent / BatchQuickPrompt / Notify / another ApiCall). The agent
  stops doing `Bash curl` on APIs we already know how to call; a 5-step
  workflow that used to burn 40k tokens on parsing can now drop that
  phase to 0. Backend layout in `backend/src/workflows/`:
  - `api_call_step.rs` — pure JSONPath extraction via `serde_json_path`
    (RFC 9535) with size-1 unwrap so `$.total` yields `42` rather than
    `[42]`; pagination shape detection (Jira `startAt`+`total`
    offset, Cloudflare GraphQL `pageInfo.endCursor`, Stripe/HubSpot
    `has_more` page, Jira v3 `nextPageToken` cursor — the migration
    trap); hard-cap `DEFAULT_MAX_PAGES = 50` against runaway walks
  - `api_call_security.rs` — three guards: (a) `assert_host_matches_base`
    rejects subdomains, scheme downgrades, cross-host redirects; (b)
    `assert_public_ip` blocks RFC 1918, loopback, `169.254.*` (the
    AWS metadata SSRF trampoline), link-local v6, ULA fc00::/7 — with
    an IP-literal fast path so `[::1]` blocks even on IPv4-only hosts;
    (c) `ResolvedAuth` with manual `Debug` impl that redacts bearers /
    api-keys via `looks_like_secret_key`, plus `redact_url_query` for
    log-safe URL echoes
  - `api_call_executor.rs` — `execute_api_call_step_core` orchestrates
    template rendering (walking `serde_json::Value` so string leaves
    get substituted without enabling JSON injection), auth resolution
    (ApiKeyQuery / ApiKeyHeader / Bearer / OAuth2ClientCredentials
    reusing the `core::oauth2_cache` contract), URL composition with
    percent-encoded query pairs, `send_with_retry` with exponential
    backoff on 5xx + 429 only (never 4xx — retrying a client error is
    a foot-gun); `SecurityPolicy` explicit type so tests can hit
    wiremock on loopback while production stays locked; wrapper
    `execute_api_call_step_with_db` loads the decrypted env via the
    existing `collect_active_api_plugins`
  - Two endpoints (`backend/src/api/workflows.rs`):
    `POST /api/workflow-steps/test-extract` (pure, no net, no DB) and
    `POST /api/workflow-steps/test-api-call` (real HTTP with
    production security policy — localhost targets fail here too so a
    user can't design a workflow that'll refuse to run in production)
  - Dispatch wired in `workflows::runner::execute_run` — `StepType::ApiCall`
    takes its own arm, no more fallthrough to the Agent executor
- **ApiCall step wizard card** (`frontend/src/components/workflows/ApiCallStepCard.tsx`)
  — plugin + endpoint pickers cascading from the project's installed
  `api_spec` plugins, query-param editor (key/value rows, template-
  aware placeholder `{{steps.X.data}}`), **Test** button firing a real
  request through `/test-api-call` with live response panel, a
  clickable `JsonTreeViewer` (each leaf + each array marker `[N]`
  generates the matching `$.path` — size-1 paths don't wrap in
  brackets, wildcard arrays land as `$.items[*]`), extract panel with
  3 radios (data/status/summary), live preview debounced 150 ms via
  `/test-extract`, 3 example-path quick buttons
  ("Tous les IDs" / "Premier élément" / "Nombre total"),
  collapsible advanced options (timeout, retries, output_var,
  fail_on_empty toggle). Empty-state "aucun plugin API configuré
  sur ce projet" when there's nothing to pick from
- **Next-step validation banner** — when the step immediately after
  an ApiCall is a `BatchQuickPrompt`, the wizard checks the resolved
  `preview.value_type` and flags a mismatch inline: green `✓ Compatible`
  if it's an array, amber warning echoing the actual type ("Data est
  `number`") otherwise. Silent on Agent / Notify / ApiCall next steps
  — those accept any shape, the banner would be noise
- **"Chartbeat top 5 → Résumé IA → Slack" starter template** — the
  désagentification aha-moment, cloneable in one click from the
  wizard creation screen when the Chartbeat plugin is configured.
  Three pre-wired steps (ApiCall → Agent → Notify), extract path
  `$.pages[*].title` so the array flows cleanly through the chain,
  `cloneTemplateSteps()` helper deep-copies and stamps the user's
  `api_config_id` into the matching ApiCall step
- **Path parameters editor for `{owner}/{repo}`-style endpoints** —
  ApiCall plugins like GitHub expose endpoints with placeholders
  (`/repos/{owner}/{repo}/issues`). The wizard now auto-detects
  `{name}` tokens in the endpoint, renders one input per unique
  placeholder below the endpoint picker, and stores the values on a
  new `WorkflowStep.api_path_params: HashMap<String, String>` field.
  At request time the executor's `resolve_path_params` substitutes
  each token (with percent-encoding for path-segment safety + `{{var}}`
  template expansion on the value, so `{owner}` = `{{steps.X.data}}`
  works for chaining). Tokens with no value stay literal — the
  request 404s, which is more diagnostic than silently dropping the
  segment. Round-trip safe: re-loading a saved workflow shows BOTH
  the template (`/repos/{owner}/{repo}`) AND the concrete values, so
  the user can change one parameter without retyping the whole path.
  A live "URL résolue" preview below the inputs flags unresolved
  tokens in amber. Backend tests in `api_call_executor::path_params*`,
  frontend tests cover detection, write-through, and absence-of-tokens
  hide path. Mask-and-restore approach disambiguates `{{var}}` from
  `{key}` without an external regex dep.
- **ApiCall / Notify steps no longer demand `prompt_template` at save** —
  the wizard's last-step validator was checking `!s.prompt_template`
  unconditionally, so saving a workflow whose only step was an
  ApiCall fired `Prompt missing for "main"` even though the step
  hits an API directly with zero LLM involvement. Validator now
  branches per `step_type`: `ApiCall` requires `api_plugin_slug` +
  `api_config_id` + `api_endpoint_path`; `Notify` requires
  `notify_config.url`; the prompt check applies only to Agent /
  Custom steps. Three new error keys (`wiz.errorApiNoPlugin`,
  `wiz.errorApiNoEndpoint`, `wiz.errorNotifyNoUrl`) × FR/EN/ES.
- **`Test the call` always shows the FULL JSON response** — when an
  extract path was set on the step (e.g. `$.toppages[*].path`), the
  backend `/test-api-call` would apply it and return the extracted
  array, breaking the click-to-pick UX once the user came back to
  the step (they'd see only the previously-extracted value, not the
  full body). Fix is one line: the wizard sends a copy of the step
  with `api_extract: null` to the test endpoint. The live preview
  on the right panel still uses `/test-extract` against the cached
  raw response, so the user keeps seeing the resolved value as they
  type the path.
- **Headers editor promoted out of "Advanced options"** in the ApiCall
  wizard — the top-bug from real-world GitHub usage was "I applied a
  User-Agent header suggestion, chip says ✓ Appliqué, but the wizard
  shows nothing changed". The fix: the Headers editor is now rendered
  inline below the Query Params editor, always visible regardless of
  the Advanced toggle state. Real APIs need headers more often than
  not (User-Agent for GitHub, X-API-Version for Adobe, Accept for
  custom mime types) — hiding them was friction. Body / Method /
  Output var / Timeout / Retries stay in Advanced (genuinely
  power-user fields). The Advanced auto-expand still applies for
  these remaining fields: open by default when any of them carries
  a value, sticky on manual collapse, the false → true transition
  is the only auto-fire. Toggle renamed to "Options avancées
  (body, méthode, timeout, retries, output var)". A small accent dot
  on the collapsed toggle still signals "something is set in here".
- **Multi-config plugin support in the ApiCall step picker** — when a
  user has multiple credentials configured for the same plugin (e.g.
  a personal GitHub PAT *and* an org-bound Euronews PAT both pointing
  at `mcp-github`), the wizard's plugin picker now keys options on
  `config.id` rather than `server.id` so each entry is selectable
  individually. The picker writes both `api_plugin_slug` and
  `api_config_id` on change; legacy workflows pre-dating this fix
  fall back gracefully to the first matching server. Stops the
  silent "always pick the first config" trap that was invisible
  whenever two GitHub or two Jira instances coexisted.
- **Auto-sync registry → DB on backend startup**
  (`db::mcps::sync_registry_servers_to_db`, called from `main.rs`
  after the orphan scan) — re-mirrors `api_spec`, `description`, and
  `transport` from `builtin_registry()` onto every existing
  `mcp_servers` row whose id matches a definition. Fixes the
  "I configured GitHub last week, now Kronn enriched the registry
  with `api_spec`, but my workflow wizard's plugin picker doesn't
  see GitHub" gap — without forcing the user to click "Refresh
  registry" in Settings → APIs by hand. Only updates EXISTING rows
  (new registry entries the user hasn't added stay registry-only).
  Backend test (`sync_registry_refreshes_api_spec_on_existing_rows_only`)
  locks the contract: stale rows pick up the new spec, never-
  configured plugins don't sneak into the DB.
- **Test coverage pass over the désagentification track** — 5 new
  test modules / blocks land alongside the features they protect:
  `rtk_args_for` + `rtk_uninstall_args_for` matrix tests
  (Claude/Codex/Gemini argv shape + unsupported set, prevents the
  `--codex --auto-patch` regression from coming back),
  `apply_step_snapshot` per-step-kind tests in `runner.rs` (Agent
  records agent / ApiCall records plugin+endpoint / Notify+Batch
  record kind only — locks the run-history shape), `apiCallAuth`
  test file covering all five `ApiAuthKind` variants including
  Basic (Jira's email+token wire shape) and the case-insensitive
  header-strip, `RunDetail` tests for the snapshot badges + the
  `LiveStepStatus` elapsed estimator (Date.now mocked, started_at
  + completed durations math). Backend now at 1297 lib tests,
  frontend at 856.
- **Jira `/search` migrated to `/search/jql`** — Atlassian removed
  `/rest/api/3/search` in April 2025 (CHANGE-2046, returns 410 Gone)
  in favour of `/rest/api/3/search/jql` with cursor-based pagination
  (`nextPageToken`) instead of offset (`startAt`/`total`). The
  Kronn registry now exposes the new endpoint by default and adds
  `POST /rest/api/3/search/approximate-count` (split out by Atlassian
  to keep the JQL query fast). The plugin tips registry was also
  re-keyed under `mcp-atlassian` (matching the actual `server.id`)
  so the AI helper now actually surfaces the Jira lore — including
  an explicit warning about the deprecated endpoint, so the agent
  flips an existing workflow to the new path on its own when it
  sees a 410. The previous `jira` slug stays as an alias for
  back-compat. Backend regression test updated to assert
  `/search/jql` IN + `/search` OUT.
- **Jira (Atlassian Cloud) as an ApiCall plugin (hybrid: MCP + REST)** —
  the existing `mcp-atlassian` definition gained an `api_spec` and the
  same `JIRA_USERNAME` + `JIRA_API_TOKEN` (already configured for the
  MCP server) now also drives REST API workflow steps. Two backend
  prerequisites had to land first: (1) **`ApiAuthKind::Basic` variant**
  — composes `Authorization: Basic <base64(user:password)>` from two
  env keys, the right shape for Jira Cloud auth where Bearer wouldn't
  work; (2) **templated `base_url`** — the executor interpolates
  `{ENV_KEY}` placeholders against the encrypted plugin env, so a
  workspace-scoped URL (`https://acme.atlassian.net`) lives in the
  config rather than in the plugin spec, and one Atlassian plugin
  serves every Kronn user (no fork-per-workspace). Unresolved
  placeholders surface a "go fill JIRA_URL in Settings → APIs" error
  rather than firing a half-composed request. 11 curated endpoints:
  `/myself` (sanity), `/search` (JQL — the killer endpoint),
  single-issue + comments + transitions, project search + single +
  components + versions, `/field` (custom-field id mapping for
  `customfield_10016` style stuff), saved-filter search.
  Same-token reuse means the user pastes the API token once, both
  Quick Prompt (MCP) and ApiCall workflows light up. Plugin tips
  registry already has the Jira lore (401/403/404/400 semantics,
  pagination v2 vs v3, customfields, JQL URL-encoding). Backend
  regression tests:
  `atlassian_builtin_has_basic_auth_templated_base_url_and_jira_search`,
  `resolve_auth_basic_encodes_user_and_password_to_authorization_header`,
  `execute_basic_auth_attaches_base64_authorization_header`,
  `execute_templated_base_url_resolves_from_env`,
  `execute_templated_base_url_unresolved_placeholder_fails_clearly`.
- **GitHub as an ApiCall plugin (hybrid: MCP + REST)** — the existing
  `mcp-github` definition gained an `api_spec` so the same
  `GITHUB_PERSONAL_ACCESS_TOKEN` powers both Quick Prompts (via the
  Stdio MCP transport) and désagentified ApiCall workflow steps (via
  Bearer auth on `https://api.github.com`). 13 curated endpoints
  shipped: `/user` (sanity), repos (`/user/repos`,
  `/repos/{owner}/{repo}`), issues + single issue + cross-repo
  `/search/issues`, pull requests + single PR + diff files, commits,
  Actions runs, releases, notifications. Path placeholders
  (`{owner}/{repo}/…`) are filled by the user in the wizard's
  endpoint combobox: the static `<select>` was swapped for
  `<input + datalist>` so picking a template seeds the input and the
  user can type over the placeholders inline — Chartbeat's stable
  paths still work as before because the datalist surfaces as a
  dropdown on focus. Plugin tips registry gained a `mcp-github`
  entry warning the AI helper about the placeholder pitfall, the
  issue/PR list overlap (PRs are issues), `q=` URL-encoding for
  search, and the SAML-SSO 403 trap. Backend regression test
  (`github_builtin_has_bearer_api_spec_alongside_mcp_transport`)
  locks the contract: Stdio transport stays for MCP, Bearer auth on
  REST reuses the same env key, key endpoints survive future trims.
- **AI helper bubble inside the ApiCall card** — discovering the
  right endpoint and JSONPath on a 4-page Jira/Cloudflare API doc
  used to be the friction wall for new users. Click "Aide config IA"
  in the card header, pick any locally-installed agent, and a
  bottom-right floating chat opens. The agent receives the API spec
  + a **fresh "current context" snapshot rebuilt on every user
  message** (endpoint / method / query / headers / body / extract +
  last test result, success body or 4xx/5xx error verbatim) — so
  asking "pourquoi j'ai un 400 ?" or "j'ai changé l'extract, regarde"
  Just Works without the user having to re-explain state. Display
  and payload are decoupled: the chat history shows what the user
  typed, the agent sees the prepended context block. A "📎 context
  chip" above the textarea makes the attached info transparent
  (API name · endpoint · last test ✓/❌). The agent answers in ≤3
  lines and emits structured `KRONN:APPLY` blocks (`endpoint` /
  `method` / `query` / `headers` / `body` / `extract`) — the UI
  parses them, hides the raw fenced JSON, and renders an inline
  "Appliquer" button per suggestion. The conversation is **ephemeral**:
  a fresh discussion is created on agent select and deleted on close /
  unmount, no localStorage, no carry-over between step edits.
  `project_id: null` is accepted (helper doesn't require a project).
  Field allowlist is enforced client-side (`applyToStep`) so a
  hallucinating agent cannot rewrite `prompt_template`, `agent`, or
  any non-API field. **Auth slots are auto-stripped**: when the
  plugin declares `ApiKeyQuery` / `ApiKeyHeader` / `Bearer` /
  `OAuth2ClientCredentials`, the matching key is removed from
  every suggestion (no more `apikey: 'VOTRE_API_KEY'` placeholders
  shadowing the user's real configured key) — and the system
  prompt tells the agent up-front not to suggest them. Single-
  installed-agent shortcut skips the picker; multi-agent popover
  uses `agentColor` dots.
  (`frontend/src/components/workflows/ApiCallAiHelper.tsx`,
  `apiCallAuth.ts`, 24 i18n keys × FR/EN/ES, 17 unit tests)
- **Auth-managed slots panel in the ApiCall wizard card** —
  green-tinted read-only fieldset above the query editor surfaces
  every credential the backend already injects at runtime
  (Chartbeat `apikey` query, Jira/Bearer `Authorization` header,
  OAuth2 token). Each row shows `<param-name>` · `••••••••` · 👁
  reveal-toggle that swaps the dots for `(env: <ENV_KEY>)` so the
  user can see exactly which env var is wired without exposing the
  secret. Stops the "I configured my key in Settings → APIs but
  the wizard asks for it again" confusion in its tracks.
- **Workflow runner derives `project_id` from the config when the
  workflow has none** — same fallback the wizard's "Test the call"
  uses: if the workflow isn't bound to a project, the runner reads
  the picked config's `project_ids[0]` (or marks the call as global
  when `is_global = true`) and decrypts the env from there. Stops
  the previous "ApiCall step requires a project-scoped workflow"
  hard-fail on global plugins or workflows the user wanted scoped
  per-trigger rather than per-project. The error is reserved for
  the genuinely-broken case (config linked to no project at all)
  and points the user to Settings → APIs to fix.
- **Wizard step 0 explains why "Next" is disabled** when the
  workflow name is empty — was previously silent (button just
  greyed). Now: required-marker `*` on the label, amber border on
  the input, helper text `wiz.nameRequired` below ("Donne un nom à
  ton workflow pour passer à l'étape suivante"), and a `title`
  tooltip on the disabled Next button repeating the same message.
  Triple-channel signaling so the user can't miss it.
- **Wizard summary resolves plugin slug → human name** —
  ApiCall step recap used to show `(mcp-github · /user)` (internal
  slug). Now resolves through the loaded `availableApiPlugins`
  catalog → `(GitHub — Perso · /user)` (server name + config
  label). Falls back to the slug only when the catalog lookup
  misses. Two-config-of-same-plugin case (perso vs Euronews) is
  now distinguishable in the recap.
- **Wizard summary surfaces the right metadata per step type** —
  the recap row used to show `1. API main (Claude Code)` for an
  ApiCall step, displaying a meaningless agent label even though
  the call hits an API directly with no LLM in the loop. The
  recap now branches per `step_type`: ApiCall shows
  `(plugin-slug · /endpoint/path)`, Notify shows
  `(webhook-host)`, BatchQuickPrompt keeps `(QP name)`, only
  Agent / Custom show the agent label. The step name color also
  drops the agent-color tint when the step doesn't run an agent
  (uses `--kr-text-faint` for ApiCall / Notify / Batch).
  `wiz.notifyMissingUrl` i18n key × FR/EN/ES.
- **Run history is now editing-proof — `StepResult` snapshots
  agent / plugin / endpoint at execution time**. Editing a workflow
  between runs (swap the agent, retarget the API plugin, change the
  endpoint) used to silently re-write the run history with the
  *current* config when the page rendered. Now the runner snapshots
  `step_kind` (`Agent | ApiCall | Notify | BatchQuickPrompt`),
  `step_agent` (for Agent steps), `step_api_plugin_slug` and
  `step_api_endpoint_path` (for ApiCall) onto each `StepResult` row
  before pushing it. RunDetail displays per-step badges built from
  the snapshot, not from the current workflow definition: `🔌 API
  mcp-github · /user`, `📤 NOTIFY hooks.slack.com`, `Codex`. Legacy
  rows (pre-snapshot) fall back gracefully — the field is optional
  on serde + ts-rs, no schema migration needed.
- **Live step status with elapsed counter + activity hint** — the
  static `running...` placeholder for the in-flight step felt frozen
  ("is it stuck or thinking?"). New `LiveStepStatus` component in
  `RunDetail.tsx` ticks every second, estimates `step_start =
  run.started_at + sum(durations of completed steps)`, and surfaces a
  step-type-aware activity label ("L'agent réfléchit", "Appel HTTP en
  cours", "Envoi du webhook", "Fan-out en cours") next to a
  tabular-num elapsed counter. Solves 90% of the "is it frozen?"
  anxiety without re-plumbing SSE end-to-end. 4 i18n keys ×
  FR/EN/ES.
- **Workflow detail page surfaces the right metadata per step**
  (`WorkflowDetail.tsx` + `RunDetail.tsx`) — `step_type === ApiCall`
  → `🔌 API` badge with plugin slug + endpoint subtitle, no
  agent label, no Test button (the wizard's "Test the call" is the
  real-call test; the detail-page Test was a dry-run mock that
  doesn't apply). `step_type === Notify` → `📤 NOTIFY` with the
  webhook host (full URL would risk leaking a secret in a
  screenshot). `step_type === BatchQuickPrompt` → existing batch
  card. Only Agent / Custom show the agent label. Card border-left
  colour-coded per type so the list is scannable at a glance.
- **Step output viewer fixed for the light theme** — the
  `.wf-step-output-code` block uses `--kr-bg-code` (intentionally
  dark in both themes — IDE convention) but the text was bound to
  `--kr-text-secondary` (dark-on-light in light theme), producing
  illegible noir-sur-noir on every step error. Pinned to
  `--kr-text-on-dark` so the contrast is always > 14:1 regardless of
  theme.
- **Wizard "Create" button kept disabled silently for ApiCall-only
  workflows** — fixed alongside the validator. The button's `disabled`
  predicate had its own copy of the validation logic which only knew
  about `BatchQuickPrompt` and `prompt_template`, so an ApiCall-only
  step (no prompt by design) left the button greyed out forever even
  after the visible validator turned green. Predicate now branches per
  `step_type` to match: ApiCall → plugin/config/endpoint, Notify →
  webhook URL, Agent / Custom → prompt_template. Stops the dead-button
  limbo where the user gets no feedback.
- **RTK badges refresh after activate / deactivate** — the
  `onActivated` parent refetch is now deferred 200ms via setTimeout
  AND moved to the `finally` block, so it fires regardless of
  success/error and gives RTK's filesystem writes (`AGENTS.md`,
  `settings.json`, shell rc) time to flush before
  `agentsApi.detect()` re-reads them. Without the delay,
  re-detection raced the writes and the badge stuck on the old
  state until a manual refresh.
- **RTK activation matrix fixed for Codex / Gemini** + **uninstall
  endpoint** — the previous `rtk init -g --codex --auto-patch` was
  rejected by RTK with `--codex cannot be combined with --auto-patch`
  (the `--auto-patch` flag answers a Claude-only TTY prompt; the
  Codex / Gemini flows write a dedicated config file with no prompt
  to auto-answer). Matrix now: Claude → `--auto-patch --hook-only`,
  Codex → `--codex` alone, Gemini → `--gemini` alone. New
  `POST /api/rtk/deactivate` endpoint mirrors `activate` with
  `--uninstall` appended (per-agent: `rtk init -g --codex --uninstall`,
  etc.) so the user can back out without manually editing
  `settings.json` / `AGENTS.md` / shell rc. Frontend `CompressionSection`
  gains a discreet "Désactiver RTK (N)" outline button visible
  whenever ≥1 agent is RTK-configured; 4 new i18n keys × FR/EN/ES.
- **Plugin tips registry for the AI helper** (`apiCallPluginTips.ts`)
  — per-slug debugging lore injected as a `### TIPS PLUGIN` block in
  the system prompt. Chartbeat (host must match Settings → Sites
  exactly, 404 on `/live/*` = host or no traffic, recommend
  `/historical/*` to isolate auth), Jira (401/403/404/400 semantics,
  pagination v2 vs v3, customfields), Cloudflare (Bearer only,
  GraphQL datetime trap, 7d max), Adobe Analytics (rsid casse-
  sensitive), Google Search/GSC (quota, siteUrl format strict). Each
  plugin's `docs_url` (or fallback) is exposed so the agent can
  redirect when stumped. The system prompt also gained an explicit
  `# Rôle` / `# Ce que tu peux et ne peux PAS faire` (no MCP, no
  fetch) / `# Méthode de debug` (6-step ordered procedure) /
  `# Style` (the `***` masking is display-only — real key IS sent)
  structure to stop the agent from doubting the auth in a loop.
- **AI helper trigger disabled when no API selected** — without
  `selectedServer` the agent has no spec, no endpoints list, no auth
  context: the button is hard-disabled with an explanatory tooltip
  rather than letting the user open an empty helper that just asks
  "go pick an API".
- **Smart JSONPath suggestions chips** in the extract panel
  (`apiCallSuggestions.ts`) — auto-derived from each test response.
  Rebuilt fresh whenever the response changes; up to 6 chips ranked
  by usefulness: `Tous les "<scalar>"` (priority field id/key/name/
  title/path/url/slug/email/pseudo, fall back to first scalar),
  `Itérer sur les N éléments` (wildcard for fan-out), `Le 1er
  élément` (handy "test before fan-out"), counter detection by name
  (`total`, `count`, `totalCount`, `total_count`, `length`). Each
  chip shows the resolved sample inline (`"fr.euronews.com/"`) so
  the user spots the right one without trial-and-error. Replaces
  the static "All IDs / First / Total count" examples that only
  worked for users who already knew JSONPath.
- **Click-to-pick everywhere in the JSON tree, with DWIM wildcard
  on keys** — every renderable atom is now clickable: object keys,
  array index buttons (`[0]`, `[1]`, …), array count markers
  (`[N]`), and leaf values. The killer feature: clicking a KEY
  inside an array item promotes the closest enclosing array's
  index to `[*]` automatically — `path` clicked under `toppages[0]`
  generates `$.toppages[*].path` (all items) instead of
  `$.toppages[0].path` (just the first). Clicking a leaf VALUE
  keeps the specific `[N]` index because the user is pointing at
  *that* item. Mental model: *clé → tous les éléments ; valeur → cet
  élément précis ; [N] → itérer ; [i] → cet objet précis*. Wired in
  `JsonNode` via dual `pathSegments` / `wildcardSegments` props.
  Regression tests lock both directions.
- **Deep one-line preview** for the resolved JSONPath value
  (`previewString` in `ApiCallStepCard.tsx`) — the chip used to show
  `Array(5)` / `Object`; now walks one level deep:
  `["fr.euronews.com/", "fr.euronews.com/voyages/2026…", … (+3)]`
  for arrays of strings, `{id: 42, title: "Hello", meta: {…}}` for
  objects. Depth capped at 1 to keep it on one line — the JSON tree
  on the left handles deeper inspection.
- **Visual pulse on the path input** when populated — 600ms accent-
  coloured `@keyframes wf-apicall-pulse` fires on every path change,
  giving immediate "your click landed" feedback without forcing the
  user to scan the form.
- **Test button derives `project_id` from the plugin config** —
  when the wizard's project field is empty, `handleTest` falls back
  to `config.project_ids[0]` (the first project the plugin is
  already linked to in Settings → APIs). The opaque "projectId
  missing" was replaced by `wf.apicall.testNeedsProjectLink` ("Cette
  config plugin n'est liée à aucun projet — va dans Settings → APIs
  et coche au moins un projet pour pouvoir tester") which surfaces
  only when there's truly no link.
- **Fresh context block re-injected on every helper message**
  (`buildContextBlock`) — display vs payload decoupled. The user
  types "pourquoi 400 ?", the chat shows that, the agent receives
  `### CONTEXTE COURANT` (snapshot of endpoint/method/query/headers/
  body/extract + last test result with the verbatim error or a
  1.5 kB JSON excerpt) followed by `### QUESTION DU USER` + the
  typed text. A `📎 context chip` above the textarea echoes what's
  attached (API · endpoint · last test ✓/❌). Stops the "I changed
  the extract path five messages ago, why doesn't the agent see it"
  staleness problem.
- **Integration runner dispatch** — `StepType::ApiCall` branches to
  `execute_api_call_step_with_db` in `workflows::runner`, using the
  parent workflow's `project_id` (not the run row's — `WorkflowRun`
  doesn't carry it). Production `SecurityPolicy` enforced: localhost
  / RFC1918 targets block even in live runs
- **Active-runs popover in the nav** — When one or more workflow runs
  are in flight, clicking the Automatisation tab (which now spins a
  Loader2 icon + counter badge) no longer navigates to the list but
  opens a popover listing every live run with its project name, live
  elapsed timer (front-computed so it ticks every second without
  hitting the network), a one-click `⏹ Arrêter` button, and a
  "Voir tous les workflows" footer for the classical route. Stop
  feedback is immediate (button swaps to `⏳ Arrêt…` and disables),
  no modal confirm — the UX designer + a lambda-user persona both
  validated that friction-free kill is the whole point. `Esc` +
  click-outside close the popover; a tiny `stopPropagation` on the
  nav button's `mousedown` prevents the opening-click-closes-itself
  race. Runs-in-flight polling stays at 3 s (existing behavior),
  the popover reads from the same `workflowsApi.list()` cache.
  Already-on-the-page fallback: a matching `⏹ Stop` button inline on
  every `.wf-card` whose `last_run.status === Running | Pending`
- **RTK (Rust Token Killer) integration** — Kronn now detects, wires,
  and reports on RTK, a Rust shell-output compressor that cuts ~89%
  of tokens on commands like `git`, `cargo`, `ls`, test runners. The
  integration ships in three layers:
  - **Detection** (`backend/src/core/rtk_detect.rs`): extends
    `AgentDetection` with `rtk_available` (binary on PATH) and
    `rtk_hook_configured` (per-agent hook file scan). Paths come
    from RTK's own docs — Claude Code = `~/.claude/settings.json`,
    Codex = `~/.codex/AGENTS.md` (not `config.toml`, caught on the
    first iteration), Gemini = shell-rc scan (bash/zsh/fish/profile),
    rest = `None` (Kiro, Copilot CLI, Vibe, Ollama are not in RTK's
    supported list — the badge explicitly reads "Non pris en charge
    par RTK")
  - **Activation** (`POST /api/rtk/activate`): takes an `agents[]`
    body, filters to RTK-compatible types, and spawns **one**
    `rtk init -g <flag> --auto-patch [--hook-only]` per agent. The
    matrix was extracted from the RTK README and defensive-fixed
    after two iterations caught: (a) `rtk init -g` alone only covers
    Claude Code, not Codex / Gemini; (b) `--hook-only` can't be
    combined with `--codex` or `--gemini` (they *are* the hook).
    `--auto-patch` is mandatory for non-interactive — without it the
    command waits on a TTY prompt the backend can't answer and
    exits 0 having done nothing ("RTK activated" lying toast)
  - **Savings counter** (`GET /api/rtk/savings`): parses
    `rtk gain --all --format json`, digs into `summary.total_saved`
    / `summary.avg_savings_pct` / `summary.total_commands`. Real-
    payload test embedded as a regression guard — the first parser
    looked at non-existent top-level keys and systematically
    returned zero. Tolerant to JSON reshape: falls back on
    generic-name keys (`tokens_saved`, `ratio`, `sample_count`),
    `available: false` when anything goes wrong so the UI hides
    cleanly rather than showing a misleading "0 tokens saved"
- **CompressionSection component** — a single card at the top of
  Settings → Agents rendering "Mode économique" (rebranded from
  "RTK" for non-technical users, attribution kept in the
  footer: "Propulsé par RTK (open source)" → GitHub). Three states
  drive the CTA copy:
  - **0/N configured** → amber card, "Activer sur les N agents
    compatibles"
  - **partial** → neutral, "Activer sur les X restants"
  - **all configured** → green, no CTA, savings counter visible
  Plus an "Install RTK" modal when the binary isn't on PATH (copy-
  paste curl command + link to GitHub to reassure the tech
  colleague). A **(?) info button** next to the title reveals a
  sobriety-numérique note in italics — *"L'usage le plus sobre
  reste de ne pas utiliser d'IA. Si vous en utilisez, RTK
  compresse..."* — because claiming "eco mode" without caveats
  doesn't match the product's values
- **Per-agent RTK badge** in the agent list row: 🟢 `RTK actif`
  / 🟡 `RTK — hook non configuré` / ⚪ `RTK non installé` / italic
  `Non pris en charge par RTK` for Kiro / Copilot CLI / Vibe /
  unsupported agents. Each badge is a link to the RTK repo so the
  user can read what the thing actually does
- **"Détails" expand on the savings counter** — 3 stat cards
  (Tokens économisés, Ratio moyen, Commandes compressées) pulled
  from the same `GET /api/rtk/savings` call, hidden behind a chevron
  toggle so the minimal UI stays a one-liner until a user wants the
  breakdown
- **Dockerfile — RTK bundled in the backend image** — pinned 0.37.1,
  same `dpkg --print-architecture` pattern used for `glab` / `bun`
  / `uv`. Adds both `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl` targets — the image is still
  single-arch at publish time but the `case` switch is arm64-ready
  for the first user on Apple Silicon self-host. New pre-created
  directory `/home/kronn/.config` chowned to the app user so RTK's
  own config writes don't trip a cross-uid permission wall
- **docker-compose RTK bind-mounts** — `~/.config/rtk`
  and `~/.local/share/rtk` mount into the container rw so
  `rtk gain` inside reads the same SQLite the user's shell wrote
  to on the host. Without these, the savings counter reports zero
  even with thousands of host-side compressions

- **Document generation — Kronn Docs** — Agents can now
  produce five file formats from a discussion without any external
  tooling on the user's side. Ships as a Python sidecar
  (`backend/sidecars/docs/`) spawned at backend startup: WeasyPrint for
  PDF, python-docx + BeautifulSoup for DOCX (HTML → Word mapping of
  headings / paragraphs / lists / tables / inline formatting), XlsxWriter
  for XLSX, stdlib `csv` for CSV, python-pptx for PPTX (Title+Content
  layout, bullets preferred over content-split). Sidecar binds to a
  random loopback port and prints `KRONN_DOCS_READY <port>` to stdout
  for deterministic startup. Rust side exposes
  `POST /api/docs/{pdf,docx,xlsx,csv,pptx}` and
  `GET /api/docs/file/:disc/:filename`; all five handlers go through a
  single `proxy_to_sidecar()` helper so adding a format = one arm.
  Filename sanitization (alphanumeric + `-_ ` only, UUID suffix,
  extension forced) + canonicalize check defend against path traversal.
  Graceful "Document sidecar unavailable — run `make docs-setup`" error
  when the venv isn't installed (the sidecar is opt-in, not a hard
  dependency). New skill `kronn-docs.md` tells the agent about the two
  fence conventions and the direct-API fallback. Auto-activation: the
  skill ships with `auto_triggers.common/fr/en/es` regex buckets that
  detect "génère un rapport PDF" / "create a presentation" / "exporta
  hoja xlsx" etc.; matched skills auto-inject into the system prompt
  (user can opt out per-skill in Settings → `auto_triggers.disabled`)
- **DocPreview component (HTML-based formats)** — when the agent wraps
  a full HTML document in a ```` ```kronn-doc-preview ```` fenced code
  block, the frontend intercepts it in `MessageBubble`'s
  `MarkdownContent` and renders a sandboxed iframe (empty `sandbox=""`
  — no scripts, no same-origin, no forms) with two export buttons
  below: 📄 PDF and 📝 DOCX. The same HTML is the payload for both
  endpoints — one preview, two formats. Per-format state with
  independent loading / ready / error rows so the user can export
  both and get two distinct download links
- **DocDataExport component (structured formats)** — a second fence
  ```` ```kronn-doc-data ```` carries a JSON payload with a `format`
  discriminator (`csv | xlsx | pptx`). No iframe — a spreadsheet or
  slide deck in an iframe looks worse than the real app — just a
  compact header (format + summary: row count / sheet count / slide
  count) and a single "Export" button. Malformed JSON or unknown
  format discriminator falls back cleanly to a regular `<pre>` so a
  broken message doesn't blow up the chat
- **Auto-trigger opt-out** — Settings → Skills gains a per-skill toggle
  backed by `POST/DELETE /api/auto-triggers`. Disabled skills stay
  visible but stop contributing to prompt injection even when their
  regexes match, letting the user neutralize a noisy auto-trigger
  without removing the skill itself
- **Secret unlock system** — A hidden area in Settings (only revealed
  after the Konami code `↑ ↑ ↓ ↓ ← → ← → B A` is entered on the page)
  exposes an input that accepts short codes. Codes are hashed
  server-side (`SHA-256` committed in `BUILT_IN_UNLOCK_HASHES`) and
  resolve to one or several unlocks — a code can unlock a theme, a
  profile, or **bundle both in one shot**. Shared with testers
  out-of-band. Operators can also add local plaintext overrides in
  `~/.config/kronn/config.toml` `[secret_themes]` for quick testing
  without a release. Generic `invalid code` error on miss so probers
  can't enumerate configured unlocks. Theme unlocks persist in
  `localStorage`; profile unlocks persist in
  `AppConfig.unlocked_profiles` (written to `config.toml` on success)
  so the profile survives restarts and shows up in `GET /api/profiles`
- **Three secret themes (hidden until unlocked)** — each adds a full
  `:root[data-theme="<name>"]` palette override:
  - **Matrix** — CRT phosphor aesthetic (near-black bg, phosphor green
    accent, green-biased text hierarchy, glow shadows). Ships with a
    JS decoding effect: titles scramble (katakana + digits + ASCII)
    then settle one-shot on mount and on occasional global pulses
    (8-14 s jitter, 15 % chance per title per pulse). The user's last
    message scrambles briefly when a discussion is opened
  - **Sakura** — pastel pink/purple blossom palette (warm white bg,
    hot-pink accent, dark-plum text). Ships with 6 falling `🌸`
    petals, each randomized on init (duration / drift / size /
    spin / opacity). Mouse proximity (≤ 90 px) pushes petals away
    with a proportional force — feels like breath on the sakura
  - **Gotham** — deep navy-noir bg with bat-signal yellow accent.
    Ships with an ambient bat-signal radial gradient drifting across
    the viewport (30 s round-trip) and 3 `🦇` that fly right-to-left
    with staggered delays so one appears every ~5-10 s
- **Batman profile — first secret built-in profile** — hidden until
  unlocked via the same input as themes (bundle code also unlocks the
  Gotham theme, so one unlock = profile + palette).
  `backend/src/profiles/batman.md` defines the persona in French: "le
  plus grand détective du monde", methodological investigator that
  collects physical evidence first, consults all configured MCPs +
  APIs, cross-checks against other repos via GitHub MCP / Context7,
  delegates to sub-agents as expert witnesses, and signs every report
  *"Je suis Batman."*. Surfaces `SECRET_PROFILE_IDS` in
  `core/profiles.rs` + visibility gating in `api/profiles.rs` so
  locked secret profiles 404 identically to missing ones —
  brute-forcing IDs reveals nothing
- **Light theme — properly accessible rebuild** — a full expert-led
  rework after the initial hasty refactor had the accent lime leak
  onto pale lime tints ("jaune sur vert clair" illegibility). New
  palette: teal-700 `#0f766e` accent (5.5:1 AA, chromatic cousin of
  the dark-theme lime on the cyan-yellow axis — same "électrique"
  tension without the contrast fail), cool-gray ramp
  (`#f6f7f9` / `#eceef2` / `#ffffff` with 11 % luminance delta so
  cards visibly lift), hovers bumped to `rgba(16,24,40,0.08+)` for
  WCAG 1.4.11 conformance on raised surfaces, shadow ramp doubled in
  thickness (two-layer with tight + wide), `--kr-text-on-accent`
  flipped to white in light (so black-on-dark-teal illegibility
  disappears). Every semantic hex flipped to Tailwind 600/700 range.
  `text-faint` merged with `text-muted` and `text-ghost` raised to
  4.5:1 to fix a fail flagged by the A11y audit where 10-11 px labels
  fell below AA normal text. Focus ring gets a
  `box-shadow: 0 0 0 4px rgba(accent, 0.22)` halo (WCAG 2.4.11 Focus
  Not Obscured Minimum)
- **`config.appearance` picker — unlocked themes appear inline** —
  once a secret theme is unlocked it slots in alongside system / light
  / dark with its own icon (Terminal for Matrix, Heart for Sakura,
  `🦇` for Gotham). The picker transparently re-uses the existing
  `set-choice-btn` styling
- **Theme-effects overlay infrastructure** — `<ThemeEffects />` mounts
  once at the app root and renders the right decorative layer based
  on the current theme. `pointer-events: none` + `aria-hidden="true"`
  on every sprite so clicks pass through and screen readers ignore
  them. `@media (prefers-reduced-motion: reduce)` disables all
  sprites + sidecar effects so the user keeps the palette without the
  motion
- **`useMatrixDecode` / `<MatrixText>`** — hook + wrapper component
  that replace a target string with a one-shot scramble animation
  (~470 ms) when the matrix theme is active, falling back to plain
  text otherwise. Applied to page `<h1>` headers, discussion titles
  in the sidebar, and the chat header title. Listens to the global
  `matrix:pulse` event for occasional re-scrambles so long-lived
  pages stay alive
- **Global `matrix:pulse` scheduler** — emitted from `<ThemeEffects />`
  every 8-14 s (jitter) while the matrix theme is active. Every
  `<MatrixText />` instance rolls a 15 % dice per pulse to decide
  whether to re-scramble — with ~20 visible titles, ~3 scrambles per
  pulse → sensation of aliveness without synchronized robotic mass
  updates
- **`useKonamiCode(onUnlock)` hook** — sequence-matching keyboard
  listener (accepts both lowercase and uppercase B/A, resets on wrong
  key but a fresh `↑` re-starts as step 1). Skips when the event
  target is `INPUT` / `TEXTAREA` / `contenteditable` so typing with
  arrow keys never advances the sequence accidentally. 6 unit tests
- **`kronn:profiles-changed` window event** — fired by the Secret Code
  submit handler whenever a profile unlock lands. The 4 consumers of
  `GET /api/profiles` (ProfilesSection, NewDiscussionForm,
  DiscussionsPage, WorkflowWizard) listen and refetch — Batman
  appears everywhere in real time without a page reload

### Changed
- **Toast system — errors are now persistent and copyable by
  default** — `useToast` gains a third argument
  `options?: { persistent?: boolean; copyable?: string }` and the
  defaults now differ per type: `success` = 3 s auto-dismiss, `info`
  = 5 s auto-dismiss, **`error` = sticky with a mandatory X close
  button**. When a `copyable` payload is passed, it renders below
  the title in a monospace `<pre>` (selectable, scrollable, max
  240 px) with a Copy button that swaps its icon to a Check for
  1 s on click. Matches a validated UX-expert + lambda-user pair:
  asymmetric treatment because a success confirms an action the
  user already took, whereas an error interrupts flow and needs
  diagnostic time. Hook file moved from `useToast.ts` →
  `useToast.tsx` (JSX + new dependencies `Copy` / `Check` / `X`
  from `lucide-react`), consumer API retro-compat
- **`POST /api/themes/unlock` response shape — `{ unlocks: [{kind, name}, ...] }`**
  (was single `{ theme }`). A single code may now match multiple rows
  in `BUILT_IN_UNLOCK_HASHES`, enabling bundles like
  kronnBatman → profile + theme together. Frontend `themes.unlock()`
  returns the array unchanged and the `ThemeContext` dispatches per
  kind. Operator-local plaintext theme overrides still resolve via
  `config.secret_themes`; profiles always go through built-in hashes
- **`AppConfig` gained `unlocked_profiles: Vec<String>`** + `secret_themes:
  HashMap<String, String>`, both `#[ts(skip)]` so codes never leak to
  the TypeScript bundle. `default_config()` initializes them empty.
  Zero migration — absence → default
- **Dark theme unchanged** — every light-theme fix is gated under
  `:root[data-theme="light"]`, the dark variables are untouched (the
  lime `#c8ff00` + black-on-lime contract stays 14:1 AAA)

### Fixed
- **ApiCall step — `jmespath-rs` crate unmaintained** — picked
  `serde_json_path` (RFC 9535) instead. Tech lead's JMESPath preference
  lost to the factual 2022-unmaintained bus factor; end-user syntax
  `$.issues[*].key` also happens to match the docs of every target API
  (Jira, Cloudflare, Chartbeat) — a nice side-effect
- **Codex RTK hook path was `config.toml`, RTK writes to `AGENTS.md`**
  — first-pass detector reported "hook not configured" forever for
  Codex. Fixed in `rtk_detect.rs` with a regression test
  (`codex_reads_agents_md_not_config_toml`) asserting AGENTS.md is the
  source of truth even when `config.toml` happens to contain the word
  "rtk"
- **ApiCall Docker loopback trap during tests** — `assert_public_ip`
  rightly blocks `127.0.0.1`, but wiremock serves from loopback, so
  the happy-path integration tests deadlocked on Security errors.
  Added `SecurityPolicy::allow_loopback_for_tests()` (explicit, no
  global cfg(test) bypass) alongside `SecurityPolicy::production()`;
  the SSRF regression test keeps using `production()` so the guard
  is actually exercised
- **RTK activation in Docker** — three successive bugs caught
  while iterating with a real user:
  1. First spawn passed no `--auto-patch` → `rtk init` waited on a
     TTY, exited 0, nothing happened. Frontend reported "RTK
     activated" falsely
  2. Second spawn overrode `HOME=$KRONN_HOST_HOME` (the *host*
     path e.g. `/home/priol`) which doesn't exist inside the
     container. RTK tried `mkdir /home/priol/.claude` and errored
     with "failed to create directory". The correct move is to
     leave HOME alone — the container already bind-mounts the
     right `.claude` / `.codex` / `.gemini` dirs from the host
  3. Third spawn added `--hook-only` to every agent. RTK rejects
     `--codex --hook-only` / `--gemini --hook-only` with
     "cannot be combined" because those flows *are* the hook. The
     flag now only applies to the Claude default command
- **Codex RTK detection path** — the first-pass detector looked
  in `.codex/config.toml`, RTK actually writes to
  `.codex/AGENTS.md`. Fixed with a regression test
  (`codex_reads_agents_md_not_config_toml`) that asserts
  AGENTS.md is the source of truth even when config.toml happens
  to mention RTK
- **`/api/rtk/savings` returned zero even with thousands of
  compressions** — the parser looked for `tokens_saved` at the
  JSON root but RTK 0.37 nests everything under
  `summary.{total_saved, avg_savings_pct, total_commands}`. The
  zero-returning path triggered the counter-hiding branch in the
  UI, so the section looked empty. Fix: navigate to `summary.*`
  with fallbacks on legacy keys + a regression test embedding the
  real user-provided JSON payload
- **Light theme — black text on dark accent illegibility** —
  `var(--kr-accent)` is now dark teal in light, so
  `color: var(--kr-text-on-accent) = #111` on it gave 3.5:1 (FAIL AA).
  User caught the regression on the pending-files badge and the
  "Créer avec IA" workflow CTA. Flipped `--kr-text-on-accent` to
  `#ffffff` in light, preserving 5.5:1 on the teal background while
  keeping 14:1 on the lime dark-theme fill
- **Light theme — sidebar blue cast** — the second-pass ramp
  (`#e3e4eb` for raised) read as "bleu" because `B=235 > R=223`.
  Neutralized to equal-channel gray `#e3e3e3` first, then the
  third-pass expert rework landed on `#eceef2` (3-pt cool tint,
  perceptually neutral)
- **Light theme — hovers indistinguishable from idle** —
  `rgba(0,0,0,0.06)` on raised gave 1.13:1 (FAIL WCAG 1.4.11 which
  requires 3:1 for state indicators). Bumped to
  `rgba(16,24,40,0.08)` on idle / `0.14` on strong / `0.18` on active
  — all pass 3:1 now
- **Light theme — bubble agent white-on-white** — white bubbles on
  near-white base (`#f5f6f8`) had no visual separation. Base darkened
  to `#f6f7f9` with 11 % delta to surface; agent bubble border
  tightened to `--kr-border-strong` + explicit `box-shadow` so the
  bubble now crisply lifts off the page
- **Light theme — active discussion almost identical to idle** —
  `.disc-item[data-active="true"]` used `rgba(accent, 0.06)` which
  gave near-imperceptible tint in light. Bumped to `0.18` + added
  `font-weight: 500`. Dark theme kept at `0.18` too (18 % lime tint
  on `#0e1117` reads distinctly without being harsh)
- **Light theme — flat neutrals `#ededed`/`#e3e3e3` read "dated"** —
  the R=G=B neutrals that were picked to kill the blue cast felt
  clinical per the UI audit. Final palette retains 3-pt cool tint
  (matches Linear / Vercel / Stripe dashboards without any "blue
  cast") — imperceptible at first glance but gives backgrounds a
  subtle alive feel

### Security
- **RUSTSEC-2026-0104 — `rustls-webpki` 0.103.12 → 0.103.13** in
  both `backend/Cargo.lock` and `desktop/src-tauri/Cargo.lock`.
  Reachable panic in CRL parsing; pulled transitively via `rustls`
  (→ `reqwest`, `hyper-rustls`, `tokio-tungstenite`, `quinn`). No API
  surface change
- **Direct `rand` dependency removed** — crypto.rs was the only
  consumer (`OsRng.fill_bytes()` for nonce + key generation), and
  `aes-gcm` already re-exports the required `RngCore` trait via its
  own `rand_core`. One less path to `rand 0.8` (RUSTSEC-2026-0097
  "unsound with a custom logger using `rand::rng()`"). A transitive
  path via `axum 0.7 → tungstenite 0.24` remains; it's a tolerated
  warning pending an axum 0.8 migration

### Developer experience
- **`make bump` keeps `Cargo.lock` in sync** — after editing the
  workspace-member `version` fields, the target now runs
  `cargo update --workspace --offline` on both backend and
  `desktop/src-tauri` (with an online fallback on cold-cache runs).
  Fixes a case where a fresh bump left `cargo check --locked`
  broken in CI
- **AI docs — secret-themes guide** — `ai/operations/secret-themes.md`
  explains the unlock system, how to add a new theme, how to add a
  new secret profile, and enumerates the security caveats (hash is
  SHA-256 unsalted on purpose so contributors can regenerate with
  `sha256sum`; codes ≥ 12 chars for dictionary resistance). Linked
  from `ai/index.md` Tier 1 table
- **Tests** — net **+35 backend** tests (secret-unlock: built-in hash
  path, bundle unlock persistence, secret-profile filter, locked-id
  404, hash determinism canary + RTK: binary detection, hook paths per
  agent incl. shell-rc scan for Gemini, regression for `.codex/
  AGENTS.md` not `config.toml`, unsupported agents always false,
  real-payload parse of `rtk gain --all --format json`, per-agent
  activation args) + **+79 frontend** (secret-unlock: Konami sequence,
  matrix decode edge cases, theme unlock bundle, tampered localStorage
  defense + workflow: ActiveRunsPopover 9 tests, inline Stop button on
  wf-card + stopPropagation + CompressionSection 16 tests covering
  the 3 states, install modal, sobriety tooltip, toast error stderr
  forwarding) + **+84 more** for désagentification: 16 extract /
  pagination-detect, 22 security guards (SSRF ranges v4+v6, redact
  unicode-safe, subdomain & scheme-downgrade rejection), 20 executor
  (Chartbeat happy path, Bearer header, 4xx no-retry, 5xx retry, SSRF
  block in production policy, templating, NO_RESULTS, no-extract
  pass-through, OAuth2 cache hit + error), 5 `test-extract` handler,
  18 `ApiCallStepCard` (empty state, cascade picker, Test, click-to-
  pick leaf + array wildcard, query-param editor add/remove, live
  preview, next-step banner 4 scenarios, advanced options toggle),
  11 Chartbeat starter template shape contract. Totals: **1262 backend
  lib / 785 frontend** — all green

---

## [0.5.0] — 2026-04-20

Major release: worktree test-mode UX, API plugins (Chartbeat as first), crash-recovery fixes, QP "Analyse de ticket Jira" hardening.

### Added
- **Plugins: kind = MCP | API | hybrid** — `mcp_servers` gains an `api_spec_json` column (migration 035) alongside the existing MCP transport. A plugin can declare a REST API capability via `ApiSpec { base_url, auth, endpoints, docs_url, config_keys }`, optionally alongside its MCP transport. Pure-API plugins use the new sentinel `McpTransport::ApiOnly`. Sync logic was taught to skip `ApiOnly` transports when writing `.mcp.json` / Vibe / Kiro / Gemini configs — their capability surfaces via prompt injection instead of disk files. Plugins UI gains per-card badges (`🔌 MCP` / `🌐 API` / `MCP + API` gradient) and a kind-filter pill row (`All | MCP | API`) next to the category pills. The add-plugin form reads `api_spec.config_keys` to (a) render non-secret keys as plain text with their own placeholders + descriptions, and (b) keep secret fields masked behind the eye toggle. Unlocks a "désagentification" roadmap: future workflow steps will call APIs directly without an agent. 5 new unit tests on the prompt-injection path + a regression guard on Chartbeat's endpoint set. Net ~750 LoC Rust + ~200 LoC React
- **Chartbeat — first API plugin** — full catalog entry `api-chartbeat` with 21 endpoints: Live dashboard API (`/live/dashapi/v4`, `/live/toppages/v4`, `/live/quickstats/v4`, referrers, geo, social, devices, video…) as synchronous GETs, and Historical API (`/historical/traffic/stats/{submit,status,fetch}/`, `/historical/traffic/series/…`, `/historical/dashapi/…`, topreferrers, authors, top_paths, sections, rankings) as 3-step async queries. Dedicated `default_context` explains the submit → status → fetch flow, includes a ready-to-paste polling loop, and warns about host vs sub-domain pitfalls (404 on `/historical/...` is usually a missing async flow, NOT an access/scope issue). Context is written to `ai/operations/mcp-servers/<slug>.md` on install — editable per-project. Auth is `apikey` query param; `CHARTBEAT_HOST` (non-secret) appears as a plain "Host (default)" field with a `domain.tld` placeholder — agents can override per-call when the user asks about a regional edition (e.g. `host=de.example.com`)
- **Adobe Analytics — second API plugin (OAuth2 S2S)** — `api-adobe-analytics` ships as the first plugin using the new `ApiAuthKind::OAuth2ClientCredentials` auth kind. Kronn mints + caches the bearer token automatically (exchange against Adobe IMS `/ims/token/v3`, 24h TTL with a 30s safety margin before refresh) so the agent never sees or handles the OAuth2 flow. 7 endpoints: `POST /reports` (the workhorse for pageviews × dimension × date range), `POST /reports/realtime`, `GET /dimensions`, `GET /metrics`, `GET /segments`, `GET /calculatedmetrics`, `GET /users/me` smoke test. Base URL templates `{ADOBE_COMPANY_ID}` into the path so the agent sees the tenant-scoped URL directly. Two mandatory extra headers (`x-api-key`, `x-proxy-global-company-id`) are surfaced via `OAuth2ExtraHeader.value_template` and interpolated at injection time. Full `default_context` with body-shape examples (pageviews-by-page, trended-by-minute, segment filters), rate-limit hints, and the top 5 pitfalls. Config keys exposed in the add-plugin form: `ADOBE_COMPANY_ID`, `ADOBE_ORG_ID`, `ADOBE_RSID` (non-secret); `ADOBE_CLIENT_ID` + `ADOBE_CLIENT_SECRET` masked. Unlocks Chartbeat × Adobe × Code × Fastly cross-analysis in a single discussion
- **Google Programmable Search — third API plugin** — `api-google-search` wraps the Custom Search JSON API. Simple `apikey=` query auth on the single `/customsearch/v1` endpoint (`https://www.googleapis.com/customsearch/v1`). `GOOGLE_SEARCH_CX` exposed as a non-secret `config_key` so users can duplicate the plugin for multiple Programmable Search Engines (site-scoped vs whole-web). `default_context` covers the full parameter matrix (`q`, `num`, `start`, `dateRestrict`, `siteSearch`, `searchType=image`, `lr`, `gl`…), the response shape (`items[].pagemap.metatags` for OpenGraph enrichment), three pre-composed curl snippets for common SEO use-cases (rank check, 7-day news window, site-scoped search), and warns loudly about the **100 queries/day free tier** + $5/1000 beyond + 10 000/day hard cap per project
- **`ApiAuthKind::OAuth2ClientCredentials` + in-memory token cache** — new auth variant carries `token_url`, `client_id_env`, `client_secret_env`, `scope`, and a `Vec<OAuth2ExtraHeader>` of provider-specific headers with `{ENV_KEY}` interpolation. Cache lives on `AppState.oauth2_cache` as `HashMap<config_id, CachedToken>` under `tokio::sync::Mutex` so concurrent discussion starts on the same plugin share one exchange. On restart the cache is lost (tokens are disposable); one HTTPS round-trip per active plugin on first use. Async resolver in `make_agent_stream` calls `core::oauth2_cache::resolve_token` for every OAuth2 plugin and injects the result under virtual env keys (`__access_token__` / `__token_error__`) — the sync `build_api_context_block` consumes those without knowing about the auth flow. Per-plugin isolation: one bad OAuth2 config doesn't hide other API plugins. 4 unit tests on cache behavior + 3 on the context render path (Adobe regression guard, template interpolation round-trip, token error surfacing). Generalizable to any future OAuth2 API (Google Analytics, Salesforce, etc.)
- **Base URL + header templating** — `ApiSpec.base_url` and `OAuth2ExtraHeader.value_template` now support `{ENV_KEY}` placeholders. Chartbeat's static URL is unchanged; Adobe's `https://analytics.adobe.io/api/{ADOBE_COMPANY_ID}` gets the live company ID substituted at render time. Missing keys render as `<NOT_CONFIGURED:KEY>` so the agent stops rather than firing a half-composed URL. Auth-guidance text adapts: *"Kronn refreshes this token automatically before it expires"* on success vs *"**TOKEN UNAVAILABLE — \<reason>**. Do not attempt API calls; tell the user and stop."* on failure — prevents unauthenticated 401 bursts when credentials are wrong
- **Worktree test-mode flow** — one-click UX wrapper around the existing `worktree-unlock/lock` endpoints. A `🧪 Tester cette version` CTA in the ChatHeader swaps the main repo to the discussion's branch + pauses the agent while the user tries the code in their IDE. A global banner (`TestModeBanner.tsx`) stays pinned at the top of the discussions page whenever any discussion is in test mode, with a single-click `Arrêter le test` button that restores the previous branch, pops the auto-stash, and re-creates the worktree. Triple preflight: (1) worktree clean, (2) main repo clean (opt-in `stash_dirty=true` or commit-first via the preflight modal), (3) HEAD not detached (force=true to override). Rollback on any checkout/stash failure — the user is never left in a half-switched state. New endpoints `POST /api/discussions/:id/test-mode/{enter,exit}` return a tagged envelope (`status: "ok" | "blocked"`) with per-kind details (`WorktreeDirty | MainDirty | Detached | …`). `TestModeModal.tsx` renders an action matrix per kind. Persistent across reboots via migration 034 (`test_mode_restore_branch`, `test_mode_stash_ref`). Dev-friendly subtext keeps git vocabulary visible alongside the user-friendly headline. 11 unit tests on the new worktree helpers + 5 integration + 13 component
- **Isolated-mode git-commit preamble** — `build_agent_prompt` now injects a worktree notice when `workspace_mode == "Isolated"` (localized fr/en/es). Agents running in a worktree get explicit instructions to `git add` + `git commit` at the end of their changes, with the branch name spelled out. Prevents the "agent modified files but the branch is empty" class of bug
- **Git-panel pending-files badge** — small accent-lime counter on the `GitBranch` icon in the ChatHeader shows N uncommitted files in the worktree (Isolated mode). Pulses on first render after an agent reply lands. Caps at `9+`; tooltip shows `"3 fichier(s) en attente de commit"`
- **Analyse de ticket Jira QP — hardened** — after auditing 3 real runs (EW-7223, EW-7141, EW-6071 — 71 messages total), rewrote the prompt template to eliminate ~25% of avoidable friction: mandatory pre-reads (`ai/templates/jira-ticket.md`, `ai/operations/confluence-doc.md`, skills), hard rules (framing-not-implementation, no-write-without-confirmation, `curl` REST v2 not MCP for description updates, valid transitions `To Frame → To Do`, code-reads budget 4-5 files), business-first lens ("quel problème BUSINESS est à résoudre ?" with rétrocompat example), 3-phase method (short tour d'horizon → deep dive → Jira wiki markup refacto)

### Changed
- **Profile injection always fires** — `start_agent_with_config` used to skip the profile prompt when a native `.claude/agents/*.md` file existed, on the assumption Claude Code would auto-load it. That assumption is false in `--print` mode: agent files there are only consulted after an explicit `@agent-name` mention. Result: `translator` profile activated but ignored. Now the compact persona injection always fires for API-capable agents, whether or not a native file exists. Added a regression guard on `build_profiles_prompt_compact` to ensure it always carries the persona name + role
- **Cancel workflow run — force DB status** — `POST /api/workflows/:id/runs/:run_id/cancel` now force-updates `workflow_runs.status = 'Cancelled'` in the DB when the in-memory cancel token is missing (runner crashed mid-await, backend restart, second-click after token already consumed). Returns `run_cancelled: true` as long as the row was actually rescued OR the token fired. Fixes the "nothing happens when I click stop" scenario on orphaned runs; 3 new integration tests
- **Agent stream: stdin for Claude Code prompt** — prompts now travel via `stdin` instead of argv on Claude Code, bypassing the Linux `ARG_MAX` per-argv cap (~128 KiB). Root cause of the "Spawn failed for npx: Argument list too long (os error 7)" on long conversations. `--append-system-prompt` is still argv-based but now truncates gracefully at 100 KiB with a `[... truncated to fit ARG_MAX ...]` marker. Doesn't affect other agents
- **Decoder-loop detection on agent stream** — Claude Opus with extended thinking can leak `</thinking>` tokens and get stuck repeating them (EW-7189 shipped 6349× closing tags into a single partial response). Two-layer defense: (1) parser-level strip of literal `<thinking>` / `</thinking>` tags before they reach `full_response`, (2) detection of ≥ 50 consecutive identical deltas of ≥ 3 non-whitespace chars → kill the child + footer "🔁 Decoder loop detected". Both the main stream loop and orchestration use the same mechanic. Size-cap safety net (2 MB) still in place as last resort
- **`cancel_registry` cleanup via forced row update** — workflow-run cancellation is no longer fragile to runner crashes / backend restarts; see "Cancel workflow run — force DB status" above
- **Plugins form — per-field metadata** — the env-key field in the add-plugin form now consults `api_spec.config_keys` FIRST for placeholders, then falls back to the static `ENV_PLACEHOLDERS` map. Any future API plugin gets meaningful form affordances (label, placeholder, inline description, no mask on non-secret keys) with zero frontend code change. Example: Chartbeat `Host (default)` field shows `domain.tld` placeholder + italic explanation underneath
- **`discussions.rs` — profile-ids regression fix** — 16 call sites that literally construct `Discussion {}` structs were updated for the new `test_mode_restore_branch` + `test_mode_stash_ref` fields via a scripted edit to avoid drift
- **Vendor-neutral tests + fixtures** — tests, placeholder values, and tech-debt notes no longer reference any real organization. All occurrences replaced with generic `example.com` / `acme.com` / `acme-frontend` / `your-company.atlassian.net`. CHANGELOG left as historical record

### Fixed
- **Fastly CLI not found inside Docker on WSL/Linux** — `npm i -g @fastly/cli` installs a JS wrapper script at `/usr/local/bin/fastly` that relative-symlinks to `../lib/node_modules/@fastly/cli/fastly.js`. Kronn mounted `/usr/local/bin` → `/host-bin/global` but NOT `/usr/local/lib`, so the symlink resolved to a non-existent path inside the container → "fastly CLI not found in PATH" when the Fastly MCP tried to shell out. Added `${KRONN_GLOBAL_LIB:-/usr/local/lib}:/host-bin/lib:ro` mount so the relative `../lib/node_modules/...` resolves correctly. Fastly CLI now runs transparently (`HOME=/host-home` picks up the user's `~/.config/fastly/` profile). Companion improvement: Fastly registry `default_context` now has a "FIRST IF fastly CLI not found" troubleshooting block, plus a traffic-correlation playbook for analytics-dip investigations (Chartbeat × Fastly hits vs cache_miss)
- **Chartbeat historical API auth was wrong in default_context** — the initial 0.5.0 Chartbeat context described `/historical/.../submit/` endpoints with `apikey=` query param. Actual Chartbeat API: historical/query endpoints require the `X-CB-AK` HEADER, the modern flow is `/query/v2/submit/page/` → `/status/` → `/fetch/`, and the legacy `/historical/traffic/series/` also accepts the header directly (often synchronously). Rewrote the context block + endpoint list based on the real Chartbeat API responses observed in production use. Also documents the 5-min live-granularity trick for short-window dip analysis (hourly historical misses sub-hour shape)
- **Test-mode modal readability** — initial CSS used undefined tokens (`--kr-bg-secondary`, `--kr-bg-primary`), which silently resolved to `transparent`, making the modal blend into the chat behind it. Replaced with real tokens (`--kr-bg-elevated`, `--kr-bg-overlay`). Primary-button hover lost the accent background because a generic `.test-mode-modal-btn:hover` rule (specificity 0,3,0) beat `.test-mode-modal-btn.primary` (0,2,0) — scoped the generic hover to `.ghost` and made the primary-hover explicit. Result: modal now has opaque `#161b22` background, hover stays on the accent lime
- **QP chain picker (⚡ button) — white-on-white hover** — the popover reused `.disc-mention-popover` / `.disc-mention-item` which rely on `data-highlighted` keyboard-nav state (never set on the QP picker) → no hover feedback at all, and the description text rendered almost invisible against the same bg color. Created dedicated `.disc-qp-picker-{item,icon,meta,name,desc}` classes with explicit `:hover` + `:focus-visible` state (accent tint) and a header row. Icon 20 px fixed column, name + description stacked with ellipsis / line-clamp
- **Orphaned workflow runs after crash / restart** — parent runs stuck at `status = 'Running'` with no in-memory token are now rescued by the second cancel click (see Changed above)
- **Decoder loop on Claude Opus extended thinking** — 76 KB of `</thinking>\n` accumulation no longer possible (see Changed above)
- **ARG_MAX / E2BIG on npx spawn** — stdin pipe fixes this (see Changed above)
- **Profile not applied when synced natively** — see Changed above
- **Test-mode CSS & UX polish** — modal background, hover readability, wording tweaks ("commit-les d'abord, ou demande à l'agent de le faire")

### Developer experience
- **Tests** — 988 backend lib + 157 integration + 648 frontend. Net +58 tests (+ 21 backend, + 13 frontend `TestModeBanner` / `TestModeModal`, + 5 context-injection, + 3 cancel-run, + 5 thinking-strip, + 4 OAuth2 cache, + 3 API-context render, + 1 Adobe regression guard, + 3 OAuth2-plugin isolation / sync-exclusion / multi-plugin token scoping)

---

## [0.4.2] — 2026-04-17

### Added
- **Discussion favorites / pin** — star icon in the ChatHeader (always visible, click to toggle). Pinned discussions appear in a dedicated "Favorites" section at the top of the sidebar, cross-project, sorted by last activity. Small `★` indicator on sidebar items for pinned discussions. Migration 033 adds `discussions.pinned` column. `PATCH /api/discussions/:id` accepts `{ pinned: bool }`
- **Unread badges on group headers** — the sidebar now shows an accent badge with the unread count on every group header (global, org, project), visible whether the group is collapsed or expanded. Previously, unread badges were only on individual discussion items (and clipped by overflow)
- **QP Chain — Phase 2 (workflow engine)** — `batch_chain_prompt_ids: string[]` on `WorkflowStep`. When a `BatchQuickPrompt` step fans out to N discussions, each child now runs the initial QP **then the full chain sequentially** inside the same conversation. The batch progress counter only bumps when the ENTIRE chain finishes for a given discussion. `spawn_agent_run_with_chain(state, disc_id, chain_ids, batch_item)` injects each chain QP's prompt as a User message between runs (author `⚡ <qp.name>`) and **renders the QP template with the same batch item value** (e.g. `"EW-1234"`) that the primary QP received — so `analyse → review → summary` on ticket `EW-1234` propagates the ticket ID through all three QPs. Chain QPs may have up to 1 variable (the first variable gets the batch item). Phase 1 (queue-a-QP-mid-stream in a single discussion) remains available for manual use
- **QP Chain — Workflow Wizard UI** — BatchQuickPrompt step form now has a "Chain more Quick Prompts (optional)" section. Chain QPs appear as ordered pills (`1. ⚡ Name`) with click-to-remove. Candidates = QPs with ≤ 1 variable (excluding the primary QP and already-chained). Hint explains the batch-item-value propagation. Also displayed as a labeled row in `WorkflowDetail` so configured chains are visible when inspecting an existing workflow
- **QP Chain UI — ChatInput picker** — while the agent is streaming, a ⚡ button next to ⏹ opens a popover of chainable QPs (those with no variables). The queued QP shows as a pulsing accent badge that click-to-cancels. Auto-fires on the `sending: true → false` edge. Extracted as the `useQpChain` hook (`frontend/src/hooks/useQpChain.ts`, 7 dedicated tests) — ref-based `onFire` pattern so callers don't need to memoize their send handler
- **rAF-batched stream writer hook** — `useRafBatchedStream` (`frontend/src/hooks/useRafBatchedStream.ts`, 5 tests). Collapses dozens of SSE token deltas per frame into one `setState` call. Extracted from `DiscussionsPage` (was inline there), now reusable for any future stream/chunk consumer
- **Custom skill editing** — Settings → Skills now shows a ✏️ edit button on every custom skill card (previously only delete was available, forcing delete+recreate for a typo fix). Reuses the create-form with prefilled values; submit dispatches to `skillsApi.update()` instead of `create()`. The markdown body is stripped of its frontmatter before populating the textarea so each edit round doesn't nest a new `---` block. New i18n keys `skills.editCustom` + `skills.saveChanges` (fr/en/es)
- **`.mcp.json` freshness guard** — `make_agent_stream` now re-syncs the project's `.mcp.json` to disk RIGHT BEFORE each agent run, plus logs an explicit warning if the file is missing when a project is set. Covers the case where MCPs were toggled/added after the last startup sync (notably hit batch discussions that spawned right after a new MCP config)
- **CLI: Ollama detection** — the CLI now detects Ollama as the 7th agent. Install via `curl -fsSL https://ollama.com/install.sh | sh`. Parallel arrays sized dynamically (no more "unbound variable" on new agents)
- **CLI: API-first hybrid mode** — when the backend is running, `kronn status`, `kronn agents`, `kronn projects` delegate to the REST API (instant, complete). Falls back to local shell detection when offline. New `lib/api-client.sh` wrapper with `kronn_api_available` probe + `kronn_api_show_agents` / `kronn_api_show_status` formatters
- **CLI: project action menu** — selecting a project now opens a sub-menu: Install template, Launch audit, Launch briefing, View MCPs, Open in dashboard. Actions adapt to audit state. Deep-link: "Open in dashboard" scrolls directly to the project card via `#project-<id>` hash
- **CLI: `--debug` auto-tails logs** — `./kronn start --debug` now streams logs automatically after boot. `./kronn logs` shows grep helpers. Help section explains where logs live
- **Dashboard deep-link** — `http://localhost:3140/#project-<id>` auto-expands and smooth-scrolls to the matching project card. Waits for project list to load before scrolling (double-rAF timing). Hash cleaned after consumption

### Changed
- **`discussions.rs` extracted** — the ~3400-line monolith was split: pure agent/text helpers moved to `api/disc_helpers.rs` (9 fns, 15 tests — `agent_prompt_budget`, `auth_mode_for`, `agent_display_name`, `smart_truncate`, `summary_msg_threshold/cooldown`, `is_compact_agent`, `language_instruction`, `estimate_extra_context_len`), and pure prompt builders moved to `api/disc_prompts.rs` (3 fns + `OrchestrationContext`, 9 tests — `build_agent_prompt`, `build_orchestration_prompt`, `build_synthesis_prompt`). `discussions.rs` is now ~2880 lines (**-15 %**, -518 lines). Behaviour unchanged — extraction is pure, tested in isolation, zero runtime diff
- **`DiscussionsPage.tsx` shrunk** — from 1783 → 1736 lines after extracting `useQpChain` and `useRafBatchedStream`. Same behaviour, cleaner separation of concerns
- **Settings → Skills card — pill overflow fix** — long skill names (e.g. `euronews-front-conventions`) used to push the `~XXX tok` badge out of the 220 px card. Header row now wraps gracefully: title stays on top, pill cluster (category + builtin/custom + token estimate) wraps below if space is tight. `overflow: hidden` on the card itself as a belt-and-suspenders
- **CLI: repo status display** — replaced verbose `ai/ 4 redirectors 6 MCPs .claude/` with compact dashboard-like format: `Validated · 9 MCPs` / `Audited · 6 MCPs` / `Template · 4 MCPs`. Color-coded by audit state (green/yellow/cyan/grey)
- **CLI boot reorder** — `./kronn start` now asks "web UI vs CLI" BEFORE running agent detection. The web UI has its own detection (instant via the backend API), so the ~5-10 s CLI sweep is skipped when the user picks web. Pure UX win on the most common path
- **CLI agent detection UX** — live progress line (`Scanning 3/7 — Vibe (Mistral)...`) replaced the frozen terminal. `show_detected_agents` prints every agent line immediately with a ⏳ placeholder, then updates each line in-place (ANSI `\033[NA` / `\033[NB` cursor moves) as the `npm view` update check returns. `check_agent_updates` (slow npm view × N) removed from the default flow — only runs when entering the manage-agents menu. Single-agent rescan after install/uninstall/update instead of full 7-agent sweep
- **CLI: `--version` timeout** — reduced from 5s to 3s with `</dev/null` to prevent agents that read stdin (Copilot, Kiro) from hanging indefinitely

### Fixed
- **Batch focus on sidebar** — clicking a Quick Prompt batch launch now passes the parent `batch_run_id` back to `Dashboard`, which threads it through `setFocusBatchId`. The sidebar auto-expands the project group + the batch group + scrolls to it after the refetch settles. Previously only the first discussion was selected with no batch-group visibility
- **Batch double-run** — `onBatchLaunched` used to set `setAutoRunDiscussionId`, triggering a second agent run for the first child (bug seen 2026-04-10: 7/6 ok on a 6-item batch). Now uses `setOpenDiscussionId` which opens without auto-running
- **Unread badge + pin star clipped by title overflow** — `.disc-item-title` had `overflow: hidden` which clipped flex children (badge, star) on long titles. Title text now truncates in its own `<span>` while badge/star remain outside the overflow zone
- **CLI: `make_args[@]: unbound variable` on macOS** — Bash 3.2 + `set -u` treats an empty array expansion as unbound. Fixed with `${make_args[@]+"${make_args[@]}"}` pattern (same as `remaining[@]` elsewhere)
- **CLI: Copilot hangs during detection** — `copilot --version` reads from stdin indefinitely. Fixed with `</dev/null` + 3s timeout. Agent still detected with version `?`
- **CLI: `_AGENT_LATESTS[$idx]: unbound variable`** — `detect_agents()` reset result arrays to 6 hardcoded elements after Ollama was added as 7th agent. Arrays now sized dynamically from `_AGENT_NAMES` length

### Infra
- **Tech-debt tracking** — three new detailed entries in `ai/tech-debt/`:
  - `TD-20260417-models-monolith.md` — `backend/src/models/mod.rs` (~2225 L, 147 types) — planned split into 15 sub-modules (15 helpers `default_*` scattered, needs dedicated session)
  - `TD-20260417-audit-monolith.md` — `backend/src/api/audit.rs` (~1966 L) — prerequisite: extract an `AuditEngine` abstraction before splitting handlers
  - `TD-20260417-projects-monolith.md` — `backend/src/api/projects.rs` (~1819 L) — sub-directory split (crud/bootstrap/clone/git/…), lowest-risk of the three
  - `TD-20260328-discussions-backend` status updated to partial-progress after the disc_helpers/disc_prompts extraction
- **`ai/` docs refresh** — `repo-map.md` LOC figures, `index.md` Last-updated date + version
- **CLI: project menu clears ANSI ghost lines** — `printf "\033[J"` after `menu_choice` + "Press Enter to continue" pause between action output and menu re-render. Fixes the "text printed on top of the old menu" visual glitch

### Tests
- Backend: **1090** (was 1026 in 0.4.1, **+64**) — +migration 033 coverage, +15 for `disc_helpers`, +9 for `disc_prompts`, +2 for `batch_chain_prompt_ids` (DB roundtrip + serde skip-if-empty)
- Frontend: **629** (was 610, **+19**) — 7 for `useQpChain`, 5 for `useRafBatchedStream`, 6 for `ChatInput` QP-chain picker, 1 for `SettingsPage` custom-skill edit button
- Shell: **195** (was 196 — 1 test removed during repos.sh refactor, 4 added for Ollama)
- Build: `pnpm build` ✅ · `cargo clippy -- -D warnings` ✅ · `tsc --noEmit` ✅

---

## [0.4.1] — 2026-04-15

### Added
- **Chat draft persistence** — the ChatInput textarea now survives tab/page navigation. Drafts are saved per-discussion in `localStorage['kronn:draft:<disc_id>']` (7-day TTL, schema-versioned, throttled 250 ms). On rehydration, a subtle "Brouillon restauré · écrit il y a X" badge shows relative time, auto-hides as soon as the user edits. New helper `lib/chat-drafts.ts` with `saveDraft` / `loadDraft` / `clearDraft` / `purgeExpiredDrafts`
- **Audit/briefing resume on navigation** — the AI audit no longer "disappears" when the user switches tabs. Backend `AuditTracker` gained a `progress` HashMap written by the 3 SSE streams (`run_audit`, `partial_audit`, `full_audit`) at each `start`/`step_start`/`done`/`cancelled`. New `GET /api/projects/:id/audit-status` endpoint. Frontend: `kronn:audit:<project_id>` checkpoint in localStorage, `ProjectCard` polls every 2 s on remount and repaints the progress bar without restarting the audit
- **MCP pulse hint on projects** — when a project has 0 plugins AND hasn't been audited yet (`NoTemplate` / `TemplateInstalled` / `Bootstrapped`), a pulsing `.dash-mcp-hint` callout invites the user to add plugins before launching briefing/audit. Respects `prefers-reduced-motion`
- **Emoji autocomplete in ChatInput** — typing `:ta` mid-sentence opens a ranked suggestion popover (`:tada:` 🎉, `:taco:` 🌮, …). Tab/Enter inserts the Unicode glyph directly (Discord/Slack UX). Mirrors the `@mention` keyboard model. Blocks false positives on timestamps (`12:30`) and URLs (`http://`). New `lib/emoji-autocomplete.ts` helper backed by `node-emoji`
- **Emoji shortcode rendering in messages** — `:shortcode:` in agent output is rendered as the Unicode glyph via `remark-emoji` inside `MarkdownContent`. Unknown shortcodes pass through verbatim (no silent data loss)
- **Syntax-highlighted diff viewer** — `GitPanel` diff view now highlights additions and context lines via `highlight.js` (core + 15 registered languages: TS/JS/Rust/Python/Go/Java/JSON/YAML/TOML/Markdown/CSS/HTML/Bash/SQL/XML). Deletions stay flat red — the point is what's being removed, not re-parsing stale code. Hunk headers, file meta (`diff --git`, `index …`, `+++`/`---`, renames, binary markers) rendered as dim italic chrome. Safe HTML injection via hljs-escaped output
- **In-memory log ringbuffer + live viewer** — every `tracing` event is captured into a 2000-line ringbuffer (`core::log_buffer`) via a custom `BufferLayer`. No file on disk, no Docker socket required. New endpoints `GET /api/debug/logs?lines=N` and `POST /api/debug/logs/clear`
- **Dedicated Debug settings card** — extracted from "Server & Security" into its own card between Server and Database in the Settings nav. Live log viewer (monospace, terminal vibe, 5-char level alignment) with Follow/Pause (auto-refresh 2 s + tail-f auto-scroll, respects user scroll), Refresh, Copy, Clear buttons. "N / 2000 lines" counter in header
- **"LIVE" visual indicator when debug mode is on** — pulsing red badge next to the Debug card title AND pulsing dot next to "Debug" in the Settings sidebar nav. Removes any ambiguity about whether verbose capture is active (the checkbox alone wasn't loud enough). Respects `prefers-reduced-motion`
- **Tracing init self-diagnostic** — first log line after boot now announces the filter in use (`tracing initialized — filter: kronn=debug,tower_http=debug`). Lets users confirm at a glance that debug_mode took effect
- **One-click "Report a bug on GitHub"** — button in the Debug card opens a new tab with a pre-filled GitHub issue (title `[Bug] Kronn v0.4.1 on macOS`, body with env info + agent summary + last 200 log lines in a collapsible `<details>`, `bug` label). Client-side redaction of common secret patterns (`sk-*`, `ghp_*`/`gho_*`/`ghs_*`/`ghu_*`, `AIza*`, `Bearer *`, JSON `password`/`token`/`api_key`/`secret`) before URL construction. Auto-trims log lines to stay under the 6000-char URL budget. Secondary "View existing issues" link to avoid duplicates
- **Debug mode** — `ServerConfig.debug_mode` persisted in `config.toml`. CLI `./kronn start --debug` writes `KRONN_RUST_LOG=kronn=debug,tower_http=debug` into `.env` for the current run (without touching config) AND auto-tails the logs after boot. `make start DEBUG=1` for direct use. `docker-compose.yml` now defaults `RUST_LOG` to `${KRONN_RUST_LOG:-}` (empty = backend picks based on `debug_mode`)
- **Diagnostic logs for cross-platform issues** — tagged `target: "kronn::agent_detect"` and `target: "kronn::scanner"`. `detect_all()` dumps env vars (HOST_OS / HOST_HOME / HOST_BIN / host_label) at sweep start + per-agent summary at end. `find_binary()` logs PATH + host_dirs, PATH hits, macOS skip reasons, final "NOT FOUND". `resolve_host_path()` logs each alias tried + success/failure + final decision. `scan_paths` logs ghost-path filter count
- **macOS APFS firmlink support** in the scanner — `resolve_host_path()` now tries 3 aliases for `/Users/X` paths: raw (`/Users/X`), APFS canonical (`/System/Volumes/Data/Users/X`), legacy (`/private/var/Users/X`). Prevents silent project-drop when a canonicalized path failed `strip_prefix`. New helper `host_home_aliases()`
- **`/api/health` enriched** with `version` (from `env!("CARGO_PKG_VERSION")`) and `host_os` (from `detect_host_label_public()`) for the bug-report flow. Docker healthcheck ignores the body — backwards-safe

### Changed
- **ChatInput remount on discussion switch** — `<ChatInput key={activeDiscussion.id}>` in `DiscussionsPage` forces a fresh mount per discussion. Guarantees the non-controlled textarea can never leak content across discussions (the root cause of the reported "same draft in all discussions" bug). Also resets voice mode / mention popover / emoji popover / draft hint cleanly at switch
- **macOS skip list extracted to `MACOS_HOST_BIN_SKIP` constant** in `agents/mod.rs`. The test `cross_agent_macos_skip_covers_npm_agents` now ENFORCES (no longer just documents) that every npm-installed agent is present — adding a new npm agent without updating the skip list is now a compile-time test failure
- **Emoji insertion format** — picking an emoji from the autocomplete inserts the Unicode glyph (🎉) into the textarea instead of the `:tada:` shortcode. Matches Discord/Slack UX where users see exactly what they picked. Agents still receive the glyph directly; `remark-emoji` handles the reverse direction for agent output using shortcodes

### Fixed
- **macOS — `gemini` never detected** — `gemini` was missing from the macOS skip list in `find_binary()`, so the host's Darwin `gemini` binary was mounted into the Linux container and failed to execute. Now covered by `MACOS_HOST_BIN_SKIP` + entrypoint installs
- **macOS — `gemini` + `copilot` never installed in the container** — `entrypoint.sh` only installed Linux versions of `kiro-cli`, `claude`, `codex`. Now also installs `@google/gemini-cli` and `@github/copilot` via npm when `KRONN_HOST_OS=macOS`
- **Chat draft lost on tab/page navigation** — ChatInput was re-rendered (not remounted) on discussion switch, and the non-controlled textarea kept its DOM value. Fixed by adding `key={activeDiscussion.id}` (see Changed above)
- **Chat draft bleed between discussions** — same root cause as above; the "same message in every discussion" bug is gone
- **`remark-emoji` + `node-emoji` install** — initial `npm install` created a parasitic `package-lock.json` alongside `pnpm-lock.yaml` and left `node-emoji` unhoisted (pnpm strict mode), breaking the Docker build. Both deps now declared explicitly via `pnpm add`
- **Debug viewer silently empty after restart** — `docker-compose.yml` resolves `RUST_LOG=${KRONN_RUST_LOG:-}` to an EMPTY STRING (not unset) when `KRONN_RUST_LOG` isn't defined, and `EnvFilter::try_from_default_env()` parses `""` into a filter that matches nothing — so flipping the debug toggle in Settings + restart produced zero captured events (stdout and BufferLayer both silenced). Fix in `main.rs`: treat whitespace-only `RUST_LOG` as "not set" so the `default_filter` derived from `config.server.debug_mode` kicks in

### Infra
- **`make bump V=x.y.z`** already existed — used to bump all 7 version files consistently (VERSION, Cargo.toml × 2, package.json × 2, tauri.conf.json, README)
- **`make start DEBUG=1`** new target helper (`_apply-debug-flag`) that writes `KRONN_RUST_LOG` into `.env`

### Tests
- Backend: **1054** (1047 lib + 147 integration at session start + ~10 new). New: `MACOS_HOST_BIN_SKIP` enforcement, `gemini`/`copilot` regression, `host_home_aliases` on 4 cases, `resolve_host_path` with aliases, `AuditTracker` progress (6 tests), `/api/projects/:id/audit-status` integration (3 tests), `log_buffer` (10 tests incl. tracing dispatcher end-to-end)
- Frontend: **610** (520 at session start + 90 new). New: `chat-drafts` (16), `ChatInput.draft` regression (8 incl. rerender-without-remount guard), `audit-resume` helper (10), `emoji-autocomplete` (18), `MessageBubble.emoji` (4 regression), `diff-syntax` (16), `bug-report` (18)
- Build: `pnpm build` (Dockerfile pipeline) ✅ · `cargo clippy --lib --tests -- -D warnings` ✅ · `tsc --noEmit` ✅

---

## [0.4.0] — 2026-04-14

### Added
- **Ollama local LLM integration** — new `AgentType::Ollama` for running local models (Llama, Gemma, Codestral, Qwen) at zero cost. HTTP API execution via `/api/chat` with separate system/user roles (model distinguishes MCP context from user question). Streaming output, token tracking (`prompt_eval_count` + `eval_count`). Health check (`GET /api/ollama/health`) with contextual hints per environment (native, Docker WSL, Docker macOS). Model listing (`GET /api/ollama/models`). `reqwest` stream feature added for HTTP response streaming
- **Ollama setup wizard in Settings** — 4-state inline card: not installed (OS-specific install commands + ollama.com link), offline/unreachable (contextual launch instructions for WSL/Linux/macOS), online with 0 models (4 suggested models with `ollama pull` commands + sizes), online with models (list of installed models with sizes). Refresh button for live status
- **Docker Ollama connectivity** — `OLLAMA_HOST` env var in docker-compose.yml. `extra_hosts: host.docker.internal:host-gateway` for Linux Docker. Contextual error message when Ollama listens on 127.0.0.1 only (WSL common issue)

### Changed
- **Ollama execution: CLI → HTTP API** — replaced `ollama run <model>` (single text blob, model confused MCP context with user question) with `POST /api/chat` (separate `role: system` for MCP/skills/profiles/directives context, `role: user` for the actual prompt). Fixes "response à côté de la plaque" issue with small models

### Tests
- Backend: **1187** (1040 lib + 147 integration). +2 Ollama endpoint tests, +5 cross-agent for 7 agents
- Frontend: **520** (41 suites). Cross-agent tests updated for 7 agents

---

## [0.3.7] — 2026-04-14

### Fixed (stability pass)
- **MCP whitelist migration** — `sync_claude_enabled_servers` now replaces the entire `enabledMcpjsonServers` whitelist instead of only adding entries. Fixes all MCPs broken by the `server.name` → `config.label` rename (Jira, GitHub, Slack, etc.). Stale entries cleaned up automatically
- **Project switch in discussions** — serde `Option<Option<String>>` can't distinguish JSON `null` from absent key. Frontend now sends `""` for "unset project", backend treats `""` as unset. Added try/catch on the PATCH call
- **Panic paths removed** — `lock().expect("poisoned")` → `match` + graceful break in agent stderr loop. 2× `unreachable!()` on `MessageRole::System` → returns `"System"`. 2× `disc.expect("is Some")` → match with SSE error response
- **Silent error swallowing** — 2 empty `catch {}` in AgentsSection (toggle, key sync) → error toast. 8 data-loading `.catch(() => {})` → `console.warn` with context. 6× `String(error)` → `userError()` in agent error handlers

### Changed
- **Package upgrades** — React 18→19, Vite 5→6, vitest 4.0→4.1, @vitejs/plugin-react 4→5, eslint 10.0→10.2, typescript-eslint 8.57→8.58, happy-dom 20.8→20.9. Only 2 lines of code changed (`useRef<T>()` → `useRef<T>(undefined)` for React 19 compat)
- **Settings accordion** — Agents, Skills, Profiles, Directives collapsed into a single card with 4 accordion sections (Agents open by default). Reduces vertical scroll by ~3 screens
- **Discussion form accordion** — Skills, Profiles, Directives in the new discussion form are collapsible (mutually exclusive). Selection count badge

### Added
- **Cross-agent regression tests** — 5 backend + 3 frontend parameterized tests that iterate ALL agent types. Auto-fail when a new agent is added without complete config (KNOWN_AGENTS, macOS skip, DB round-trip, color/label)
- **API smoke tests** — skills CRUD, directives list+CRUD, stats (tokens + agent-usage), quick_prompts CRUD, agents detect, disc_git (status + diff route), ai_files, discover_repos. +11 integration tests
- **Component smoke tests** — ChatInput, ProjectCard, WorkflowWizard render without crashing
- **Accessibility** — `aria-label` on ChatInput textarea, NewDiscForm selects (project + agent)

### Tests
- Backend: **1182** (1037 lib + 145 integration)
- Frontend: **520** (41 suites)
- Security: `cargo audit` clean (0 vuln), `pnpm audit` clean (0 vuln)

---

## [0.3.6] — 2026-04-14

### Added
- **Guided tour / onboarding overlay** — 15-step interactive walkthrough for new users, auto-launched on first visit. 3 interactive steps where the user clicks the real UI element (portal-rendered pulse animation, "Next" blocked until click). 5 acts with group labels (Projets → Plugins → Discussions → Automatisation → Config). Ends on Discussions page for action-oriented onboarding. Spotlight via box-shadow cutout, tooltip auto-positioned, mobile bottom-sheet. Keyboard: Escape/arrows. Replayable from "?" nav button or Settings. `kronn:tour-completed` localStorage persistence. Designed by consensus of 3 expert personas (PM Marie, UX Designer, Learning Scientist). 10 unit tests
- **Skill: structured-questions** — teaches agents the `{{var}}: question` format for structured Q&A. Bidirectional protocol: agent asks in `{{var}}: text` format → UI renders form → user replies as `var: value` lines → agent parses correctly. Category: domain
- **Profile: Translator / Teacher (Lin)** — contextual translation with vocabulary explanations. Translates with register awareness, explains idioms and jargon inline, treats each exchange as a micro-lesson. 17 builtin profiles total
- **macOS Docker agent bootstrap** — `entrypoint.sh` installs Linux `claude` + `codex` via npm on macOS hosts (Darwin binaries can't run in Linux container). Agent detection skips host-mounted Darwin binaries for `claude`, `codex`, `copilot`, `kiro-cli`. `~/.npm/bin` mounted via `KRONN_NPM_BIN` env var
- **Gemini CLI Docker mount** — `~/.gemini:/home/kronn/.gemini:rw` added to docker-compose.yml (was missing → Gemini crashed on agent switch with ENOENT on `projects.json`)
- **CI: desktop type-check** — `cargo check` of `desktop/src-tauri/` added to `ci-test.yml` to catch signature mismatches between backend lib and Tauri desktop app
- **Cross-agent regression tests** — 5 backend + 3 frontend parameterized tests that iterate over ALL agent types. Auto-fail when a new agent is added without complete configuration (KNOWN_AGENTS entry, macOS skip, DB round-trip, frontend color/label). Filet de sécurité pour ne plus casser un agent en en touchant un autre

### Changed
- **Settings: accordion for Agents & Skills** — Agents, Skills, Profiles, Directives collapsed into a single card with 4 accordion sections. Agents open by default, others collapsed. Reduces vertical scroll by ~3 screens
- **Discussions: accordion in advanced options** — Skills, Profiles, Directives in the new discussion form are now collapsible (mutually exclusive). Selection count badge. Same visual pattern as Settings
- **Tour step descriptions** — multiline text support (`white-space: pre-line`) for richer explanations. Step "3 façons de commencer" uses line breaks for clarity

### Fixed
- **Desktop Tauri build broken** — `AppState` and `WorkflowEngine::new()` signature updated in `desktop/src-tauri/src/main.rs` to match backend changes (removed `workflow_engine` field, added `cancel_registry`, `WorkflowEngine::new(state)` instead of `(db, config)`). Boot scans (orphan runs + partial recovery) added to desktop — were missing since 0.3.5
- **Project switch in discussions silently failing** — `Option<Option<String>>` serde bug: JSON `null` and absent key both deserialize as `None` (= no change). Frontend now sends `""` for "unset project", backend treats `""` as unset. Added try/catch + console.error on the PATCH call
- **Tour pulse animation invisible** — `box-shadow` on target element was hidden by parent stacking contexts (sticky nav). Pulse is now a separate portal div (`.tour-pulse-ring`) rendered above everything
- **Tour spotlight not cleaned up on step change** — `tour-target-elevated` class was not removed when transitioning to centered steps (welcome/finale). Fixed by calling `cleanupPrev()` before early returns in `useTourPositioning`
- **Tour backdrop blocking clicks on interactive steps** — `pointer-events: none` on backdrop during `waitForClick` steps so clicks reach the real UI element

### Tests
- Frontend: **517** (39 suites). +10 tour, +3 cross-agent consistency
- Backend: **1171** (1037 lib + 134 integration). +5 cross-agent regression (every_type_in_known_agents, definitions_complete, no_custom, macos_skip_covers_npm, db_round_trip_all_types)

---

## [0.3.5] — 2026-04-13

### Added
- **Batch Quick Prompts** — fan-out a Quick Prompt to N tickets/items in parallel. New step type `BatchQuickPrompt` with `batch_items_from` (resolves `{{steps.X.output}}` / `.data` / manual list), `batch_wait_for_completion`, `batch_run_worktrees`. Each child gets its own discussion, optional worktree isolation, aggregated in sidebar groups. Dry-run preview shows eligible items + warnings + per-item rendered prompt + one-click per-item test
- **Partial response recovery** — agent output is checkpointed every ~30s / ~100 chunks into `discussions.partial_response` (+ `partial_response_started_at`). On backend crash/restart, dangling partials are converted into Agent messages with an "⚠️ Réflexion interrompue" footer and `PartialResponseRecovered` WS broadcast. Migrations 031 (partial_response) + 032 (started_at) + 030 (workflow_run parent). `POST /api/discussions/:id/dismiss-partial` force-recovers on demand. `send_message` refuses a new run while a partial is pending (`partial_pending` SSE error) — prevents the 2026-04-13 double-response bug
- **Stop agent** — `POST /api/discussions/:id/stop` triggers a registered `CancellationToken` via `AppState.cancel_registry`. CancelGuard RAII pattern cleans the registry on agent completion. Frontend "⏹" button in chat header
- **Cancel workflow run (cascade)** — `POST /api/workflows/:id/runs/:run_id/cancel` cancels the linear run token AND cascades to every child batch discussion via `workflow_run.parent_run_id`. Child batch runs marked `Cancelled` in DB. Idempotent
- **Dry-run step test tracker** — module-level `activeStepTests` Map with subscribe/notify so in-flight step tests survive tab switches (React unmount). Each StepCard subscribes to its (workflowId, stepName, index) key
- **Workspace toggle always visible** — on new discussion form, the Direct/Isolated toggle is always shown when a project is selected; Isolated is disabled with tooltip when `repo_url` is null
- **UI locale persistence on Tauri WebView2** — backend stores `ui_language`, `stt_model`, `tts_voices` in config. `I18nContext` fetches from backend first with localStorage fallback, fixing the WebView2 localStorage wipe on Windows
- **SSE limits** — new `core/sse_limits.rs` module: global max concurrent SSE streams + per-client limit, configurable via `ServerConfig`
- **Cross-platform cmd helpers** — `core/cmd.rs` (`async_cmd`, `sync_cmd`) applies `CREATE_NO_WINDOW` on Windows. ALL `Command::new` calls routed through these to suppress flash-console windows on Tauri desktop
- **Structured agent questions** — `{{var}}: question` syntax parsed from the last agent message (`lib/agent-question-parse.ts`). When detected, a mini-form (`AgentQuestionForm.tsx`) renders above ChatInput with labeled fields for each variable. Submitting fills values and sends a formatted response. 15 parser tests + 5 component tests
- **Notify workflow step** — `StepType::Notify` with `NotifyConfig` (webhook URL, HTTP method POST/PUT/GET, optional body). Direct `reqwest` from Rust, zero tokens consumed. Template rendering in URL + body (`{{previous_step.output}}`, etc.). Frontend wizard form with method select + body textarea. 5 backend tests
- **5 new agent profiles** (total 16 builtins): Data Analyst (Ren), Data Engineer (Ash), SEO/Growth (Rio), SRE/DevOps (Ops), Staff Engineer (Dex)
- **Add project from local folder** — `POST /api/projects/add-folder` for non-git directories. Auto-detects `.git` if present. 3rd tab "Dossier local" in new project modal. 4 integration tests
- **Global context** — `ServerConfig.global_context` (markdown) + `global_context_mode` (always/no_project/never). Injected into agent prompts. `GET/POST /api/config/global-context` + `GET/POST /api/config/global-context-mode`. Settings UI with textarea + mode dropdown. 1 integration test

### Changed
- **Bootstrap-architect skill** — deeply rewritten for gated validation flow (architecture → plan → issues). +251 lines with clearer stage handoffs
- **Pagination** — `PaginationQuery.page` no longer has a `serde(default = 1)` — `Option<Query<_>>` now correctly falls through to unpaginated mode when no query params are sent. Regression fix for the 50-items silent cap
- **Settings UX** — section reorder (Usage before Server & Database), export warning redesigned (proper CSS class, "tokens d'authentification" consistent wording, clickable link scrolls to Server section)
- **Contrast & accessibility** — all inline `rgba(255,255,255,0.2-0.3)` replaced with CSS tokens (`--kr-text-dim`, `--kr-text-ghost`, `--kr-cancelled`). Token values raised from 0.2/0.3 to 0.35/0.45 for better readability. 8 icon-only buttons gained `aria-label`. Advanced toggle gained `aria-expanded`
- **Error messages humanized** — new `userError()` helper wraps raw `String(e)` in user-friendly messages (network, timeout, 413, generic fallback). 4 `alert()` calls replaced with `toast()`. Covers Dashboard, DiscussionsPage, WorkflowsPage
- **Hints rewritten for non-dev users** — batch worktree, agent question form, global context hints rewritten to explain WHY not HOW (FR/EN/ES)
- **Terminology consistency** — "clés API" vs "token API" confusion resolved in reset confirm dialog (FR/EN/ES). Distinction: "clés des fournisseurs IA" + "token d'authentification"

### Fixed
- **50-items silent pagination cap** — regression test added: creating 60 discussions and calling plain `GET /api/discussions` returns all 60 (not 50)
- **Double agent response after backend restart** — `partial_response_started_at` preserved across checkpoints so the recovered message sits chronologically before any later user message. `send_message` blocks while a partial is pending
- **Dry-run test state lost on tab switch** — module-level tracker owns the AbortController; components re-subscribe on mount
- **i18n placeholder mismatches** — new parity test caught 6 EN keys with dangling `{N}` placeholders (literal `{2}` rendered in UI)
- **Clippy** — `doc_lazy_continuation` in `models/mod.rs`, `manual_pattern_char_comparison` in `workflows/batch_step.rs`
- **macOS Docker: Claude Code not detected** — host-mounted macOS (Darwin) binaries can't execute in the Linux container. `entrypoint.sh` now bootstraps Linux `claude` + `codex` via npm on macOS hosts (same pattern as existing Kiro curl install). Agent detection skips Darwin binaries for all npm agents (`claude`, `codex`, `copilot`, `kiro-cli`). `~/.npm/bin` mounted + added to container PATH via `KRONN_NPM_BIN` env var (auto-detected by Makefile)
- **NewDiscussionForm: Escape + click-outside** — modal now closes on Escape key and overlay click (standard UX pattern)
- **NewDiscussionForm: double-submit prevention** — create button disabled after first click
- **AgentQuestionForm: Ctrl+Enter to submit** — keyboard shortcut + visual hint badge
- **Empty state projects** — text rewritten to guide user toward + button (add folder / clone / bootstrap)

### Tests (robustness pass)
- Backend: **1166** (1032 lib + 134 integration)
- Frontend: **504** (38 suites). New helpers + tests:
  - `src/test/apiMock.ts` — shared `buildApiMock()` factory (all 13 namespaces + 5 flat fns) + completeness test (ns coverage, flat-fn coverage, deep-merge preserves siblings)
  - `src/lib/__tests__/i18n-parity.test.ts` — 9 tests asserting fr/en/es key isomorphism + non-empty values + placeholder-subset invariant
  - `src/components/workflows/__tests__/BatchItemsList.test.tsx` — 6 tests (render, toggle prompt, dry-run forwarding, no-agent hides btn, running disables btn, defensive empty-prompt)
- `dictionaries` + `BatchItemsList` exported from their modules for testability

### DB migrations
- `030_workflow_run_parent.sql` — `workflow_runs.parent_run_id` for batch fan-out linkage
- `031_partial_response.sql` — `discussions.partial_response` (TEXT, nullable)
- `032_partial_response_started_at.sql` — preserves checkpoint start time across updates

---

## [0.3.4] — 2026-04-08

### Added
- **Quick Prompts** — reusable prompt templates with `{{variables}}` and conditional sections `{{#var}}text{{/var}}`. New tab "Quick Prompts" in the Automation page. Launch creates a discussion with rendered prompt and dynamic title. Full CRUD API + DB migration
- **MCP registry: 4 new MCPs** — MongoDB (official), Kubernetes (Red Hat), Qdrant (vector DB), Perplexity (AI search)
- **MCP Microsoft 365** — Outlook, Teams, OneDrive, OneNote via Softeria community server (device code flow auth)
- **MCP env var placeholders** — realistic hints for 30+ env vars + eye toggle on add form
- **Bootstrap++** — enhanced project creation with gated validation. New skill `bootstrap-architect` guides through 3 stages: architecture analysis → project plan → issue creation. Each stage requires user validation via CTA banner. Drag & drop document upload in the bootstrap modal (architecture docs, specs, PRDs). Uploaded files injected as context for the agent
- **WSL project discovery** — Windows Tauri app now auto-discovers WSL home directories for repo scanning

### Changed
- **Page title** — "Workflows" renamed to "Automatisation" (the page now contains Workflows + Quick Prompts tabs)
- **MCP registry** — Puppeteer removed (use Playwright), Google Analytics publisher corrected to "Community", Docker MCPs mention Docker requirement in help
- **MCP category pills** — fixed: filtering by category now works correctly (separated category selection from text search)
- **Setup wizard** — skeleton loader during agent detection, optimistic toggle (no rescan), animated completing state, parallel agent detection + repo scan (tokio::join)
- **Scan button** — loading state + toast feedback ("N new projects detected")
- **Reset config** — confirmation dialog with data loss warning

### Fixed
- **WSL scan paths** — `default_scan_path()` now returns WSL home on Windows native, scan always includes WSL homes
- **Setup wizard completion loop** — fast path for setup/status when already complete

---

## [0.3.3] — 2026-04-07

### Added
- **Export/Import ZIP cross-OS** — export as ZIP (data.json + config.toml sans secrets), import with config merge (pseudo, bio, language, scan_paths), path remapping for invalid project paths, contacts included in export (version 3). Retrocompatible with JSON v2 imports
- **Project path remapping** — `POST /api/projects/:id/remap-path` to fix project paths after cross-OS migration. Invalid paths flagged with warning toast after import
- **Workflow AI Architect** — new builtin skill `workflow-architect` + "Create with AI" button on Workflows page. Opens an interactive discussion where the AI designs, optimizes, and deploys a workflow. Agent emits `KRONN:WORKFLOW_READY` signal → one-click CTA creates the workflow
- **Test individual workflow steps** — `POST /api/workflows/test-step` with dry-run mode (agent reads but doesn't write). Live streaming output in the UI with elapsed timer. "Tester" button on each step card
- **Workflow starter templates** — 6 clickable examples in the simple wizard (Code Review, Changelog, Tech Debt, Test Coverage, Doc Update, Security Scan). Pre-fill name + prompt on click
- **MCP env var placeholders** — realistic hints for 30+ common env vars (Jira, GitHub, Slack, etc.) + eye toggle visibility on the add form
- **Setup wizard improvements** — WSL/Windows host label badge on agents, enable/disable toggle per agent
- **Stale-while-revalidate** — `useApi` hook keeps previous data visible during refetches, new `initialLoading` flag for first-load skeleton

### Changed
- **Export format** — now ZIP instead of raw JSON. Version bumped to 3. Includes `config.toml` (without auth_token/encryption_secret/API key values)
- **Workflow project_id** — can now be changed on existing workflows (was locked)
- **Workflow step prompts** — expandable with "Show more/less" toggle (was truncated to 200 chars)
- **Raw cron editor** — complex cron expressions (e.g. `0 7,10,13,16,19 * * 1-5`) preserved as raw strings instead of being mangled by the simple parser

### Fixed
- **Setup wizard completion loop** — clicking "Go to Dashboard" after skipping repos no longer loops back (setScanPaths with default path)
- **Setup status performance** — fast path skips filesystem scan when setup is already complete (10-30s → <1s on WSL paths)
- **Workflow project_id persistence** — SQL UPDATE was missing project_id column + serde double-Option deserialization fix
- **WSL agent detection fallback** — probe `~/.local/bin`, `~/.kiro/bin` when `bash -lc which` fails (non-interactive shell guard)
- **Flash of empty state** — projects/discussions no longer flash empty during refetches

---

## [0.3.2] — 2026-04-03

### Added
- **MCP default contexts** — new `default_context` field on `McpDefinition`. Registry MCPs can ship pre-filled context files (best practices, token-saving tips) written automatically to `ai/operations/mcp-servers/<slug>.md` on first install. Fastly is the first MCP with a default context (result pagination, JSON format, common commands)
- **MCP setup help i18n** — MCP setup instructions (`token_help`) can now be overridden per-locale via `mcp.help.<id>` i18n keys. Fastly and GitLab have dedicated help texts in fr/en/es
- **Claude Code settings sync** — `sync_claude_enabled_servers()` ensures Claude Code's `settings.local.json` whitelist (`enabledMcpjsonServers`) stays in sync with `.mcp.json`. MCPs added via Kronn are automatically added to the whitelist. Fixes a silent bug where Claude Code ignored MCPs not in its internal whitelist (bug #24657)
- **MCP publisher & origin badges** — new `publisher` (string) and `official` (bool) fields on `McpDefinition`. Registry cards and detail panels show "Officiel — Fastly" (green) or "Communautaire — Anthropic" (orange). All 49 registry entries classified
- **MCP load indicator** — per-project MCP count badge in scope toggles (green 1–5, orange 6–10, red 11+). Helps avoid agent slowdown from too many MCPs
- **MCP alt_packages matching** — new `alt_packages` field on `McpDefinition` allows the registry to recognize alternative package names for the same MCP server (e.g. npm `fastly-mcp-server` → registry `fastly-mcp`). Prevents duplicate `detected:*` entries when users have a different runtime than the registry default

### Changed
- **Fastly MCP → official Go server** — replaced the community npm package (`fastly-mcp-server`, required Bun) with the official Fastly MCP binary (`fastly-mcp`). Auth via Fastly CLI profiles (`fastly profile create`), no env var needed
- **GitLab MCP → official glab CLI** — replaced the archived Anthropic npm package (`@modelcontextprotocol/server-gitlab`, SDK 1.0.1 incompatible with modern Claude Code) with GitLab's official CLI (`glab mcp serve`). Auth via `GITLAB_TOKEN` + `GITLAB_HOST` env vars (stored encrypted in Kronn), supports self-hosted instances
- **MCP plugin detail panel** — setup instructions (`token_help`) and token link (`token_url`) are now displayed separately. URLs in help text are clickable. Setup section shown even for MCPs without env vars (e.g. Fastly, GitLab, Docker)
- **Codex MCP timeout** — npx/uvx-based MCP servers now get 60s startup timeout (was 30s). Fixes cold-start timeouts when packages are downloaded for the first time

### Fixed
- **GitLab MCP broken with Claude Code** — archived Anthropic package (`@modelcontextprotocol/server-gitlab`) uses SDK 1.0.1 which hangs on `notifications/initialized` sent by modern Claude Code. Replaced with `glab mcp serve` (official GitLab CLI)
- **Fastly MCP 401** — community npm package required Bun runtime for `execute` tool. Migrated to official Go binary that works standalone
- **MCP scan duplicate configs** — `match_registry_entry()` and `migrate_detected_to_registry()` now use `alt_packages` for cross-runtime matching (npx vs Go binary). `dedup_configs()` merges configs with same label+server_id (catches post-migration duplicates). Fixes 3x Fastly and 2x GitLab entries after sync
- **Stale project-level Codex config** — removed orphan `front_euronews/.codex/config.toml` that overrode the Kronn-managed global config with stale names and missing MCPs

---

## [0.3.1] — 2026-04-01

### Added
- **Usage dashboard** — new "Usage" section in Settings with summary cards (total tokens, estimated cost, discussions, workflows), provider breakdown bar, per-project horizontal bars, and daily history chart (30 days, stacked by provider). Toggle between token count and USD cost view. Filter by discussions, workflows, or all
- **Per-message cost tracking** — `cost_usd` column on `messages` table (migration 024). Real cost captured from Claude Code's `result` stream event; fallback to static pricing estimation for other providers
- **Static pricing engine** — `core/pricing.rs` with per-provider token pricing (Anthropic, OpenAI, Google, Mistral, Amazon). Used when real cost is unavailable
- **Daily usage history API** — `GET /api/stats/tokens` now returns `daily_history` with per-day token/cost breakdown by provider (last 30 days)
- **Discussion deep-link from Usage** — clicking a discussion name in the Usage top-5 list navigates directly to the discussion page and opens it
- **GitHub Copilot agent** — 7th supported agent (`copilot` CLI). Detected, installed, updated, and uninstalled via both web UI and Kronn CLI. Model tiers: economy (`gpt-4o-mini`), reasoning (`o4-mini`). Auth via `GH_TOKEN`, `COPILOT_GITHUB_TOKEN`, or `~/.copilot/config.json`. Full access flag: `--allow-all-tools`
- **Context files** — upload files (text, xlsx, docx, pptx, pdf, images) as context for discussions. Drag & drop, clipboard paste, or file picker. Extracted text injected into agent prompt. Images saved to project dir for agent vision tools. Max 20 files, 500KB text / 10MB images
- **User bio** — optional bio in Settings > Identity. Injected at the start of the first message in each new discussion so agents tailor responses to the user's profile

### Changed
- **Usage centralized in Settings** — the per-agent "Estimated token usage" section in Config > Agents has been removed. All usage data is now in the dedicated Usage section with richer visualizations
- **`StreamJsonEvent::Usage`** — `cost_usd: Option<f64>` integrated directly into the `Usage` variant; the separate `Cost` variant has been removed

### Fixed
- **Cross-platform audit** — 17 fixes for Windows/macOS/Linux/WSL/Docker compatibility: HOME/USERPROFILE resolution, `.cmd`/`.exe` binary detection, `WSL_DISTRO_NAME` detection, hostname fallback, Makefile BSD compatibility, UNC path normalization, conditional `SHELL` env var
- **First message identity** — Gravatar and pseudo were missing from the first message of a discussion (create handler didn't load identity from config)
- **AppImage removed** — Linux desktop builds now produce only `.deb` (19MB) instead of `.deb` + `.AppImage` (90MB)

---

## [0.3.0] — 2026-03-31

### Added
- **Workflow suggestions from MCP introspection** — `GET /api/projects/:id/workflow-suggestions` matches installed MCPs against a catalogue of 10 workflow templates (orphan PR detection, sprint digest, changelog, stale PRs, bug reports, PR quality, 5xx correlation, sprint brief, perf monitoring, doc sync). Each suggestion includes multi-step prompts, pre-filled trigger, and audience tag (dev/pm/ops)
- **Suggestion panel in workflow wizard** — sparkle button shows contextual workflow suggestions when a project with MCPs is selected. "Activate" (simple mode) or "Import as draft" (advanced mode). Multi-step or advanced suggestions auto-switch to advanced mode
- **Workflow wizard: simple mode** — new 3-step wizard (Infos, Task, Summary) alongside the existing 5-step advanced mode. Toggle at the top of the wizard. Simple mode: one agent, one prompt, manual or scheduled trigger
- **Scheduled trigger in simple mode** — "Manual" or "Schedule" toggle with visual frequency picker (every X minutes/hours/days). Converts to cron behind the scenes
- **System tray (desktop)** — closing the window hides to tray instead of quitting. Backend + workflow scheduler keep running. Double-click tray icon to reopen. "Quit" in tray menu for real exit
- **Wake lock (desktop)** — when cron workflows are active, prevents OS sleep. Windows: `SetThreadExecutionState`. macOS: `caffeinate -w`. Auto-releases when no cron workflows remain
- **MCP audit introspection (step 8)** — audit now calls read-only MCP tools to discover capabilities (tool inventory, project context: Jira projects, GitHub repos, Slack channels, etc.) and documents them in `ai/operations/mcp-servers/`. Generates workflow automation hints table
- **MCP drift auto-detection** — adding/removing/relinking a plugin on an audited project invalidates the `.mcp.json` checksum, flagging drift for step 8 re-run
- **Ad-hoc codesigning for macOS** — CI applies `codesign --force --deep -s -` when no Apple Developer certificate is configured. Release notes include `xattr -cr` instructions

### Changed
- **MCP renamed to "Plugins"** — all user-facing labels (FR/EN/ES), nav tab, page title ("Plugins (MCP / API)"), icons (Server -> Puzzle). Internal code keys unchanged
- **Plugin registry: card grid with category pills** — replaces the flat scrollable list. Cards with icon, name, description (2-line clamp), "Setup required" label. Category filter pills matching Config tab style (border-radius: 20px)
- **Installed plugins: inline expand** — click a plugin card to expand the detail panel in-place (grid-column: 1/-1), no CLS. Shows tokens, scope toggles, project links. Replaces the old accordion-by-server and the above/below detail panel
- **Plugin detail from project page** — clicking a plugin in ProjectCard navigates to Plugins tab and opens the detail panel for that specific config
- **Workflow wizard: advanced options hidden** — concurrency, workspace hooks moved behind "Advanced" toggle in the Config step. Per-step settings (model, retry, stall timeout) were already behind a toggle
- **Audit templates enriched** — `TEMPLATE.md` adds Capabilities table (tools, read-only flag, use-cases) and Project context section. `mcp-servers.md` adds Key capabilities column and Workflow automation hints table

### Breaking (internal)
- **Structured inter-step contract** — new `StepOutputFormat` enum (`FreeText` | `Structured`) on `WorkflowStep`. When `Structured`: engine auto-injects `---STEP_OUTPUT---` envelope instructions, extracts JSON envelope (`{data, status, summary}`) from output, exposes `{{previous_step.data}}`, `{{previous_step.summary}}`, `{{previous_step.status}}` in addition to raw `{{previous_step.output}}`. Includes repair prompt fallback when LLM doesn't comply. Existing workflows unaffected (default = `FreeText`)
- **Catalogue multi-step prompts** — all 10 workflow templates now have 2-4 specialized steps. Collection steps use `Structured` format with explicit data schema in the prompt. Synthesis steps use `FreeText`. Steps reference `{{previous_step.data}}` for structured data instead of raw output

### Fixed
- **Fastly MCP broken** — `fastly-mcp-server` v2.0.x switched to bun runtime. Pinned to v1.0.4 (Node.js) in registry + all 21 `.mcp.json` files across 7 repos. Backend test `pinned_packages_are_respected` prevents regression
- **`PINNED_PACKAGES` dead_code warning** — moved constant into `#[cfg(test)]` module
- **ProjectCard: Server icon → Puzzle** — consistent with Plugins rename

---

## [0.2.2] — 2026-03-29

### Added
- **Contact network diagnostics** — when adding a contact that's unreachable, the API now diagnoses the cause (Tailscale not active, LAN mismatch, peer offline) and returns a machine-readable code. Frontend shows a contextual toast instead of a generic error (i18n FR/EN/ES)

### Fixed
- **Windows: console windows flashing** — every background command (git, agent detection, npx probes, etc.) spawned a visible cmd.exe window on the Tauri desktop app. New `core::cmd` module applies `CREATE_NO_WINDOW` flag to all 50+ `Command::new` calls across the codebase
- **WSL agents not detected** — `wsl.exe -e which` doesn't load the user's login profile, so npm-installed agents (`~/.local/bin/claude`, etc.) were invisible. Now uses `bash -lc` for correct PATH resolution. Version detection also runs via `wsl.exe` for WSL binary paths
- **WSL repositories not scanned** — git commands failed on `\\wsl.localhost\...` UNC paths because Windows `git.exe` doesn't handle them. Git now runs inside WSL via `wsl.exe -e bash -lc "git -C ..."` for WSL filesystem paths. Scan timeout increased from 10s to 30s for WSL paths (9P filesystem is slow)
- **Desktop/self-hosted: "Cannot connect to backend"** — auth middleware relied on `X-Real-IP` header (set by nginx) to detect localhost. In Tauri desktop mode (no nginx proxy), all requests were treated as remote → 401 Unauthorized. Now also checks the direct peer IP via `ConnectInfo`. Startup timeout increased from 5s to 15s. Frontend auto-retries 5 times (2s interval) before showing the error screen
- **macOS CI codesign crash** — empty `APPLE_CERTIFICATE` secret was still exported as an env var, making Tauri attempt to import a null certificate. Signing env vars are now only exported when non-empty
- **Stale installers in CI artifacts** — cargo cache persisted old `.exe`/`.msi`/`.dmg` files across builds. Bundle directory is now cleaned before each build

### Changed
- **Setup wizard: all steps are now optional** — agents and repository detection steps can be skipped (button switches to "Passer cette étape"). Enables non-developer use cases: global discussions without projects, project creation without git repos
- **App icon** — new Lucide Zap lightning bolt icon (`#c8ff00` on `#0a0c10`) matching the web UI. Generated via `cargo tauri icon` from SVG source. Replaces the old generic icon across all platforms (ICO, ICNS, PNG, Windows Store logos)
- **`core::cmd` module** — centralized `async_cmd()` / `sync_cmd()` helpers replace raw `Command::new()` everywhere (agents, scanner, worktree, git ops, workflows, tailscale, checksums, audit). Single place to enforce cross-platform command behavior
- **WSL host label** — agents found via WSL now show "WSL" instead of "Windows" in the setup wizard (new `via_wsl` flag on `BinaryLocation`)

---

## [0.2.1] — 2026-03-28

### Fixed
- **WS security: first message must be Presence** — non-Presence first messages are now rejected, preventing invite code verification bypass (found by multi-agent audit)
- **Tauri desktop: blank page** — `extract_dir` doubled subdirectory paths (`assets/assets/index.js`). Fix: always use root target for path resolution
- **macOS CI build** — removed `|| ''` fallback on Apple signing secrets that caused empty certificate import to fail
- **Localhost exempt documented as tech debt** — `TD-20260328-localhost-exempt` with rotation plan

---

## [0.2.0] — 2026-03-28

### Added
- **Multi-user P2P chat** — share discussions between Kronn instances via WebSocket. Replicated model: each peer stores a full copy, messages sync in real-time
- **`POST /api/discussions/:id/share`** — share a discussion with contacts, broadcasts `DiscussionInvite` via WS
- **`WsMessage::ChatMessage`** — real-time message relay between peers with idempotent insertion (no duplicates)
- **`WsMessage::DiscussionInvite`** — auto-creates local discussion when a peer shares with you
- **Auto-add peers** — unknown but valid invite codes are auto-accepted as pending contacts (no mutual-add required)
- **Host IP detection for Docker** — `KRONN_HOST_IPS` env var, detected at `make start`, passed to container for accurate invite codes
- **Native skill files** — SKILL.md written to `.claude/skills/`, `.agents/skills/` (Codex), `.gemini/skills/` for progressive agent discovery (~95% token savings vs prompt injection)
- **Native agent profiles** — profiles synced as `.claude/agents/`, `.gemini/agents/`, `.codex/agents/` files
- **CSS design system** — `tokens.css` (83 CSS variables), `utilities.css`, `components.css` + per-page CSS files
- **Pagination API** — `?page=1&per_page=50` on discussions list and workflow runs (backward compatible)
- **Auth by default** — auto-generated Bearer token at first launch. Localhost exempt (no lock-out risk). Peers require token. WS auth via invite code
- **Share button** — in chat header, pick a contact to share the discussion with
- **Shared badge** — green Users icon on shared discussions in sidebar
- **Network feedback** — orange "pending" badge + tooltip on unreachable contacts, "offline" label for disconnected accepted contacts

### Changed
- **DiscussionsPage split** — 3254 → 1218 lines + 6 extracted components (ChatHeader, ChatInput, DiscussionSidebar, NewDiscussionForm, MessageBubble, SwipeableDiscItem)
- **SettingsPage split** — 1944 → 990 lines + 3 sections (AgentsSection, IdentitySection, ProfilesSection)
- **WorkflowsPage split** — 1780 → 373 lines + 3 components (WorkflowWizard, WorkflowDetail, RunDetail)
- **Dashboard split** — 1478 → 674 lines + 2 components (ProjectList, ProjectCard)
- **Backend split** — `projects.rs` 3823 → 1396 + `audit.rs` + `ai_docs.rs` + `discover.rs`. `discussions.rs` 3696 → 2322 + `disc_git.rs`
- **Inline styles extraction** — 1157 → 182 inline styles (dynamic only). All static styles moved to CSS
- **Prompt optimization** — native SKILL.md files use progressive disclosure instead of injecting full content. ~25 token reference prompt vs ~800 tokens full injection
- **WS endpoint** — skips auth middleware (invite code verification in ws.rs instead)
- **Tauri desktop app** — frontend files embedded in binary via `include_dir!` (fixes 404 on Windows/macOS installs)
- **Windows Tauri + WSL** — agents detected and executed via `wsl.exe -e` when running on Windows native. Windows paths auto-converted to WSL paths

### Fixed
- **TTS no sound** — added `media-src blob:` to nginx CSP (audio blobs were silently blocked)
- **Tailscale badge** — now conditional on `advertised_host === tailscale_ip` (badge stayed when switching to LAN IP)
- **French accents** — ~120 i18n strings corrected (détecté, sélectionné, créer, réseau, etc.)
- **Spanish accents** — ~90 i18n strings corrected (configuración, validación, código, etc.)
- **Discussion CTA from Projects** — clicking a discussion in ProjectCard now correctly opens it (was missing `onOpenDiscussion(disc.id)`)
- **Discussion visibility on navigate** — `ensureDiscussionVisible` now waits for `allDiscussions` to load before expanding sidebar groups
- **Test stability** — added `act()` flush in `wrap()` helper across 4 test files to reduce flaky failures

---

## [0.1.2] — 2026-03-25

### Added
- **Worktree unlock/lock** — manual button next to the branch badge to release/re-create the worktree. Lets you `git checkout` the branch in your main repo for testing without archiving the discussion
- **Auto re-lock** — when resuming a discussion whose worktree was unlocked, the worktree is automatically re-created (blocks if the branch is still checked out in the main repo)
- **API endpoints** — `POST /discussions/:id/worktree-unlock` and `POST /discussions/:id/worktree-lock`
- **Git signoff by default** — all commits now include `-s` (Signed-off-by), good practice at zero cost

### Changed
- **Worktrees in project directory** — worktrees are now created in `.kronn-worktrees/` inside the repo instead of `/data/workspaces/` in the Docker container. Visible from the host IDE (PHPStorm, VS Code, etc.)
- **Relative gitdir paths** — worktree cross-references use relative paths so they work both inside Docker and on the host
- **Startup migration** — existing worktrees at `/data/workspaces/` are automatically migrated to the new location on startup

### Fixed
- **GPG sign crash** — `--no-gpg-sign` is now passed when the user does not enable `-S`, preventing failures when `commit.gpgsign=true` is set in the git config but the signing key is missing
- **Worktree gitdir broken on host** — `.git` files in worktrees contained Docker-internal absolute paths (`/host-home/...`), now rewritten to relative paths
- **Branch checkout conflict** — clear error message when the branch is already checked out in the main repo instead of a cryptic git error

---

## [0.1.1] — 2026-03-25

### Added
- **MCP: draw.io** — official jgraph server added to registry (49 built-in servers)
- **MCP popover search** — filter + max-height scroll when > 6 MCPs (Discussions page)
- **MCP context file** — `ai/operations/mcp-servers/drawio.md`
- **Installation guide** — `docs/install.md` (Linux, macOS, Windows/WSL2)
- **ErrorBoundary per zone** — each Dashboard page (Projects, MCPs, Workflows, Discussions, Settings) has its own error boundary with inline retry
- **WorkflowStep metadata** — new `step_type` (Agent/ApiCall) and `description` fields on workflow steps, visible in wizard and summary. Prepares for future de-agentification of mechanical steps
- **Shell completions** — bash and zsh autocompletion for `kronn` CLI commands, auto-installed on first run
- **`make bump V=x.y.z`** — centralized version bump across all files (VERSION, Cargo.toml, package.json, tauri.conf.json, README)
- **CHANGELOG.md** — this file

### Changed
- **orchestrate() refactor** — extracted `run_agent_streaming()` and `run_agent_collect()` helpers, reducing orchestrate from ~625 to ~427 lines
- **Version centralized** — single `VERSION` file at repo root; shell, Rust (`env!`), and frontend (`package.json` import) read from it dynamically
- **Git push/PR: auto-token injection** — GitHub token resolved from MCP configs (encrypted in DB), injected into `gh` and `git push` automatically. SSH URLs rewritten to HTTPS with embedded token — no `gh auth login` or `export GITHUB_TOKEN` needed
- **PR creation: auto-push** — `Create PR` automatically pushes the branch if no upstream exists
- Installation docs simplified: agent install is handled by Kronn's setup wizard, not manual npm commands
- **Workflow runner** — replaced `run.clone()` with lightweight `RunProgressSnapshot`, avoids cloning full run state on every step
- **Error hints** — removed outdated French-only comment (messages were already in English)
- **Multi-arch Docker** — confirmed all Dockerfiles already support amd64 + arm64 natively (base images + arch-aware installs)
- **Zero `as any`** — eliminated all 12 `as any` casts across frontend (workers + tests), replaced with proper types (`VoiceId`, `AutomaticSpeechRecognitionPipeline`, `AgentType`, `AiAuditStatus`, `ToastFn`, `UILocale`)

### Fixed
- **Discussion badge desync** — unseen badge showed false positives when switching away from a discussion with an active agent stream
- **SSH on macOS** — git push now works on macOS Docker Desktop via `/run/host-services/ssh-auth.sock` forwarding
- **`.kronn-tmp/` polluting git status** — added to `.gitignore` + global git excludes in container; retroactive fix on startup for existing projects
- **`.kronn-worktrees/` not gitignored** — same treatment as `.kronn-tmp/`
- **Workflow run progress** — running workflows now show step-by-step progression with current step highlighted, instead of just "Running"
- Test fixtures — replaced project-specific names with generic placeholders
- Tech-debt list cleaned: removed 7 resolved entries

---

## [0.1.0] — 2026-03-24

### Added
- **Multi-agent discussions** — Claude Code, Codex, Vibe, Gemini CLI, Kiro with `@mentions`, debate mode, SSE streaming
- **MCP management** — 3-tier architecture (Server → Config → Project), 48 built-in servers, encrypted secrets (AES-256-GCM), disk sync for all agents
- **Workflow engine** — cron, multi-step multi-agent pipelines, tracker-driven (GitHub), manual triggers, 5-step creation wizard, live SSE progress
- **AI audit pipeline** — 4-state system (NoTemplate → TemplateInstalled → Audited → Validated), 10-step automated analysis, drift detection + partial re-audit
- **Pre-audit briefing** — optional 5-question conversational briefing injected into audit steps
- **Project bootstrap** — create new projects from scratch with AI-guided planning (Architect + Product Owner + Entrepreneur)
- **Tauri desktop app** — native installers for Windows, macOS, Linux (no Docker required)
- **Voice: TTS & STT** — 100% local, Piper WASM (9 voices FR/EN/ES) + Whisper WASM, voice conversation mode
- **5 supported agents** — Claude Code, Codex, Vibe (CLI + direct Mistral API), Gemini CLI, Kiro
- **Agent configuration (3-axis)** — 11 profiles (WHO), 22 skills (WHAT), directives (HOW)
- **ModelTier system** — abstract tier selection (fast/balanced/powerful) resolved per agent
- **Multi-key API management** — multiple named keys per provider with one-click activation
- **Token tracking** — per-message token counting (Claude Code stream-json, Codex stderr)
- **Worktree isolation** — each discussion/workflow in its own git worktree
- **GitHub/GitLab PR management** — create, review, merge from the dashboard
- **Responsive UI** — mobile-friendly layout
- **i18n** — French, English, Spanish (CLI + web)
- **CI pipeline** — GitHub Actions: clippy, cargo test, tsc, vitest, bats, security scan (label-triggered)
- **Security** — Bearer token auth (opt-in), CSP headers, AES-256-GCM for secrets

### Stack
- Backend: Rust (Axum 0.7, tokio, serde, SQLite WAL)
- Frontend: React 18 + TypeScript (Vite 5)
- Type bridge: ts-rs (Rust → TypeScript)
- Container: Docker Compose (backend + frontend + nginx gateway)
