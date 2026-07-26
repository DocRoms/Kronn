# Mode Mentor — parcours d'apprentissage socratique pour débutants

Statut : **design** (2026-07-22, idée user). Une surface d'apprentissage dédiée où un débutant (junior / apprenti) traite un sujet ou un ticket **avec** l'IA, mais en se posant les questions : le mentor guide, pointe des ressources, challenge le plan puis le code — **sans jamais donner la solution**. But : casser l'usage passif de l'IA (« donne-moi la réponse »).

> **Mise à jour 2026-07-25 — retrait du rôle tuteur.** Le rôle « tuteur » (peer invité, gate `brouillon → validation tuteur`) a été **abandonné** : il n'avait jamais été câblé (pas d'invitation `peer` dans `api/mentor.rs`) et le gate de validation était déjà neutralisé (tout parcours s'ouvre directement à l'apprenti sur `status = open`). Concrètement : endpoint `POST …/validate` + `validate_by_tutor()` supprimés ; le bouton d'override « Forcer (tuteur) » devient **« Passer outre »**, un déblocage **self-serve** de l'apprenti (garde le flag `forced` + la note « débloqué manuellement »). Ce déblocage sert aussi d'issue de secours quand le mentor n'approuve pas et que les indices sont épuisés (sinon cul-de-sac → abandon). Les passages ci-dessous mentionnant « tuteur » / « validation tuteur » / `draft` sont conservés comme trace de conception mais **ne reflètent plus l'implémentation**.

> **Mise à jour 2026-07-27 — tour mentor rapatrié côté serveur (review PR #135).** Le tour mentor→censeur→evaluateur était auparavant **orchestré côté front** (`triggerStream` streamait la sortie BRUTE du mentor au navigateur, le front décidait `reply = leak === false ? … : null` puis POSTait `record_turn` / `block-approval`). C'était contournable : un apprenti pouvait lire la solution non filtrée dans le flux réseau, ou forger un `reply`/une approbation. **Désormais le tour tourne entièrement côté serveur** (nouvel endpoint `POST /api/mentor/parcours/{id}/turn` → `run_turn`, sur le pattern du hint : tâche de fond + poll `last_turn.status`). Le serveur exécute le workflow, parse le verdict censeur (`parse_leak`, **fail-closed** — révèle seulement si `leak == false`) et le verdict evaluateur (`parse_ready`), fait UNE reformulation auto sur fuite, puis n'écrit/renvoie que le résultat vetté dans `block.turns` — **la sortie brute du mentor ne quitte jamais le serveur**. Endpoints `record_turn` + `block-approval` **supprimés** (plus rien de forgeable). Modèle : nouveau `TurnState` (`last_turn`) + `begin_turn`/`finish_turn`. Autres fixes de la même review : `advance` refuse toute phase ≠ phase courante (même avec `force`) — plus de saut de blocs ; `apply()` fait load→mutate→save dans un **seul** `with_conn` (plus de lost-update entre run de fond et action apprenti) ; Ressources/Target ne comptent « fait » qu'une fois **dépassés** (front + backend `progress()`) ; le refund de cran d'indice ne s'applique plus au plafond `HINT_MAX` ; `complete_chapter` hors borne renvoie une erreur (plus de faux `success`). Les passages « le front orchestre le tour » ci-dessous **ne reflètent plus l'implémentation**.

## Le problème

Un débutant + IA sans garde-fou → il obtient la bonne réponse **sans construire le modèle mental**. Il devient dépendant et ne progresse pas. On ne veut pas empêcher l'usage de l'IA, on veut **imposer la réflexion** avant qu'elle n'assiste.

## Le principe

- **Strict absolu** : l'IA ne produit jamais de code ni la solution. Uniquement questions, ressources, indices conceptuels gradués. Dernier cran de l'échelle = escalade à un tuteur humain, jamais un fragment de solution.
- **Surface à blocs gatés** (pas un chat qui défile) : ①Compréhension → ②Ressources → ③Cible visée → ④Plan → ⑤Code → ⑥Bilan. ⑤ verrouillé tant que ④ pas validé, etc. Le gating **force l'effort dans le bon ordre**.
- **Mode universel** (pas de niveaux paramétrés pour la v1).

Maquette de forme validée : voir l'artifact `Mode Mentor · Kronn` (jetable).

## Décisions verrouillées

| Sujet | Choix |
|---|---|
| Posture IA | Strict absolu — jamais de solution |
| Surface | Page-parcours à blocs gatés, reprenable |
| Niveaux | Universel unique |
| Entrée | Hybride : ticket Jira **ou** sujet libre → brouillon IA → ~~validation tuteur~~ **ouvert directement à l'apprenti** (rôle tuteur retiré, cf. note du 2026-07-25) |
| Bloc ③ Cible | Schéma d'archi (*où on va*) + tests d'acceptation (*comment savoir que c'est bon*) |
| Parcours parallèles | Oui — 1 disc par parcours, isolation gratuite |
| Garde-fou | 2ᵉ modèle « censeur » qui vérifie chaque sortie du mentor |
| Gating | **Piloté par l'état appli** (`mentor_state` + `/api/mentor`), pas par des Gate steps WF |

## L'archi — Kronn a déjà presque toutes les briques

### Le parcours = une Discussion

La `Discussion` (`backend/src/models/discussions.rs:14`) est l'entité pivot et convient tel quel. **1 parcours = 1 disc** → parallélisme et isolation gratuits.

- **`pin_first_message`** (`discussions.rs:63`) épingle le 1er message et ne le résume jamais → on y fige le **protocole socratique** (règles strict-absolu + sujet + critères). C'est l'ancre du garde-fou.
- **`profile_ids` / `directive_ids`** portent les personas Mentor + Censeur.
- **`discussion_sessions`** (`backend/src/db/sql/060_discussion_sessions.sql`) rôles `owner`/`peer` : apprenti = `owner`, tuteur = `peer` invité via `disc_invite_peer` (`backend/src/api/disc_invite.rs:697`). → **la disc EST la surface partagée** (répond au besoin « quelque chose de partagé quelque part »).
- **`workflow_run_id`** (`discussions.rs:90`) rattache la disc au run de setup.

### Nouveau : colonne `mentor_state` (JSON)

Pas de champ metadata libre aujourd'hui ; on ajoute une colonne JSON sur `discussions` — migration **`backend/src/db/sql/074_mentor_state.sql`** — sérialisée comme l'existant `participants_json`. Contenu :

```jsonc
{
  "status": "open | done",  // (+ `generating`; `draft`/`validated` gardés en legacy — plus produits depuis 2026-07-25)
  "source": { "type": "jira|free", "ticket_key": "EW-1234" | null },
  "phase": "comprehension|resources|target|plan|code|bilan",
  "blocks": {
    "comprehension": { "unlocked": true, "validated": true, "learner": "…" },
    "resources":     { "items": [ { "title","url","kind","read": false } ] },
    "target":        { "archi": "…", "acceptance_tests": "…" },
    "plan":          { "unlocked": true, "validated": false, "revisions": 2 },
    "code":          { "unlocked": false },
    "bilan":         { "unlocked": false }
  },
  "hint_level": 1,
  "hints_spent": [ { "block": "plan", "level": 1 } ]
}
```

Le gating vit ici et est **appliqué côté backend** (`/api/mentor`) + rendu par le front. Les workflows ne servent qu'au **setup** et au **tour mentor→censeur**.

### Le tour de mentor : réutiliser le pattern produce→verify

Kronn a déjà **`MultiAgentReviewConfig`** (`backend/src/models/workflows.rs:646`) : un agent produit, invite un `reviewer_agent` (modèle différent recommandé), débat jusqu'à `max_rounds`. C'est exactement le duo **mentor → censeur**.

```
soumission apprenti → mentor (produit) → censeur (vérifie fuite) → affiché
```

- **Mentor** = `AgentProfile.persona_prompt` (le QUI, `backend/src/models/agents.rs:113`) + `Directive` strict-absolu (le COMMENT, `:156`).
- **Censeur** = 2ᵉ profile/directive, mandat unique : *« cette sortie révèle-t-elle tout ou partie de la solution ? »* → verdict structuré `{ leak: bool, severity, spans }`. Si fuite → régénération avec rappel strict, plafonnée à N essais, puis repli sur relance neutre + escalade tuteur. Chaque fuite est **loguée** (tuning de la directive dans le temps).
- Assemblage du prompt : `build_agent_prompt` (`backend/src/api/disc_prompts.rs:295`) ; runner : `AgentStartConfig` (`backend/src/agents/runner.rs:369`).
- Réutiliser le cœur de `POST /api/discussions/:id/orchestrate` pour le débat (cf. [[collaborative-plan-review]] qui recommande d'extraire ce cœur en helper moteur).

### Setup hybride (entrée ticket **ou** sujet libre)

Workflow de création (trigger manuel / API) :
1. **Ticket** : step `fetch_issue` sur le pattern `JsonData → ApiCall` Jira `/rest/api/3/issue/{key}` (`backend/src/workflows/big_ticket_template.rs:338`), plugin `jira` du registre (`backend/src/core/registry.rs:440`), exécuté par `api_call_executor.rs` (credentials injectées server-side — rien à écrire). **Sujet libre** : pas de fetch, on part du texte.
2. Agent « génère le brouillon » : objectif, critères, ressources[], cible (archi + tests) → écrit dans `mentor_state` + 1er message épinglé. Le parcours **s'ouvre directement** à l'apprenti (`status = open`) — pas de gate de validation tuteur.
3. Poste un **commentaire backlink** sur le ticket Jira (« Parcours mentor → <lien Kronn> »). *(L'invitation d'un tuteur `peer` prévue à l'origine a été abandonnée — cf. note du 2026-07-25.)*

### Jira = backlink only

Source de vérité = la disc. Jira reçoit seulement un commentaire lien (+ éventuel label/statut à la complétion). Justification : un commentaire est une mauvaise surface d'édition, et **les sujets libres n'ont pas de ticket**.

### Frontend

- Nouvelle valeur `'mentor'` dans `type Page` de `frontend/src/pages/Dashboard.tsx` + `pages/MentorPage.tsx` lazy-loadée.
- Namespace `mentor` dans `frontend/src/lib/api.ts` (helper REST enveloppé `api<T>()`), live via `hooks/useWebSocket.ts`.
- Réutiliser `components/{MessageBubble,ChatInput,AgentQuestionForm}.tsx` pour la surface socratique ; les blocs de parcours se composent comme les groupes de messages existants.
- Types front auto-générés depuis les structs Rust `#[ts(export)]` → **définir les types côté Rust d'abord**.

## Contrats API (`/api/mentor`) — implémenté (à jour 2026-07-27)

- `GET  /api/mentor/parcours` → liste des parcours (`ParcoursSummary[]`) pour la page d'accueil.
- `GET  /api/mentor/parcours/{id}` → `MentorState` typé (le front poll ici pendant qu'un hint/tour/bilan est `pending`).
- `POST /api/mentor/parcours/generate` → crée un parcours vide (`status: generating`) + lance le workflow générateur en fond (mentor **ou** cours onboarding selon `mode`). `project_id` blanc normalisé → `None`.
- `DELETE /api/mentor/parcours/{id}` → supprime le parcours (disc + state) — ex. nettoyer une génération en échec.
- `POST /api/mentor/parcours/{id}/submit` `{ block, content }` → stocke la soumission de l'apprenti.
- `POST /api/mentor/parcours/{id}/turn` `{ block, submission }` → **(serveur, 2026-07-27)** lance le tour mentor→censeur→evaluateur en fond, fail-closed ; renvoie `last_turn: pending` (poll `GET …/{id}`). La réponse vettée atterrit dans `block.turns` ; la sortie brute du mentor ne quitte jamais le serveur.
- `POST /api/mentor/parcours/{id}/hint` `{ block, submission }` → « Coup de pouce » gradué, généré + vetté en fond (`last_hint`), fail-closed.
- `POST /api/mentor/parcours/{id}/advance` `{ block, force? }` → valide **le bloc courant** (refusé si `block ≠ phase`) et déverrouille le suivant. `force` = « Passer outre » (bypass read/approval, jamais l'ordre des blocs).
- `POST /api/mentor/parcours/{id}/resource-read` `{ index, read }` → coche une ressource (bloc ② Ressources).
- `POST /api/mentor/parcours/{id}/chapter` `{ index, answer? }` → **(onboarding)** marque un chapitre terminé ; index hors borne → erreur.
- `POST /api/mentor/parcours/{id}/bilan` → (re)génère la synthèse de clôture en fond (`bilan_synthesis`).
- `GET  /api/mentor/onboarding-catalog/{project_id}` → sujets curés parsés depuis `docs/onboarding.md`.

**Retirés :** ~~`/validate`~~ (2026-07-25, rôle tuteur) · ~~`/turn` version front + `record_turn`~~ et ~~`/block-approval`~~ (2026-07-27 — l'approbation est désormais posée server-side par le tour, plus rien de forgeable).

## Le point dur : faire tenir le strict absolu

C'est 80 % de la réussite. Trois leviers :
1. **Découper, pas résoudre** : face au blocage, le mentor fragmente en sous-questions jusqu'à la trivialité.
2. **Exemple analogue, jamais le cas réel** : indice ultime = un exemple sur un *autre* problème structurellement proche.
3. **Censeur non négociable** : le producteur est biaisé vers aider (donc vers lâcher la réponse) ; un sceptique au mandat étroit et modèle séparé rattrape ce que la directive laisse passer.

## Extension — deux postures : Mentor & Onboarding (2026-07-22, idée user)

Le Mode Mentor et un mode **Onboarding / cours** (façon OpenClassrooms) sont **une seule feature à deux postures**, pas deux modules. Même substrat (disc + `mentor_state` + lien projet + ancrage doc IA + `MentorPage`), posture pédagogique inversée :

| | **Mentor** | **Onboarding / Cours** |
|---|---|---|
| Orienté | une tâche (ticket) à résoudre | une connaissance à acquérir |
| Posture | socratique — ne donne jamais la solution | explicatif — montre et explique le vrai code |
| Censeur | oui | **non** (un cours qui refuse d'expliquer est inutile) |
| Rythme | gates durs (blocs verrouillés) | checkpoints souples (quiz / « essaie toi-même ») |
| Blocs | comprehension→resources→target→plan→code→bilan | chapitres (explication + checkpoint) |

**Impact technique (à implémenter, non fait) :**
- `MentorState` gagne un discriminant `mode: "mentor" | "onboarding"` + un contenu variant (enum serde taggé → union discriminée TS via ts-rs). Le shape 2a actuel = le contenu de la posture `mentor`. Le contenu `onboarding` = `Vec<Chapter>` (`{ title, explanation, checkpoint, done }`).
- Nouveau persona **« Prof / formateur »** (explicatif) + directive « explique clairement, montre le vrai code, ponctue de checkpoints ». Le Censeur ne s'active qu'en mode `mentor`.
- `MentorPage` rend les deux (mode-aware) : mentor = 6 blocs ; onboarding = liste de chapitres dépliables + checkpoint.

**Ancrage projet (les deux postures) :** l'agent tourne dans le worktree du projet → lit la doc IA (`AGENTS.md`, `repo-map.md`, `glossary.md`, `decisions.md`, `.planning`) et les vrais fichiers pour cadrer sujet, ressources et cible. Le mentor reste strict-absolu même si la solution est dans le repo.

**Registre des sujets d'onboarding (doc IA, comme la dette technique) :** un `docs/onboarding.md` — chaque sujet `{ titre, périmètre, prérequis, fichiers/docs de référence, niveau }` — alimenté à la fois **curé par un humain** ET **suggéré par un audit agent** (scanne code + doc IA → propose modules complexes, zones à fort churn, sous-systèmes non documentés ; mirroir des `audit_*` / intel-updater existants). Le mode onboarding lit ce registre → catalogue de cours → génération d'un parcours-chapitres depuis les vrais fichiers.

## Prochaines étapes

1. ✅ **Fait (2026-07-22)** — Personas Kronn créés : profil `custom-mentor-socratique--mode-mentor` (Noé), profil `custom-censeur-anti-solution--mode-mentor` (Garde), directive `custom-mentor---jamais-de-solution--strict`. Stress-test des prompts : la directive seule laisse passer la « solution en prose » ; c'est le Censeur qui ferme ce trou → valide l'archi à deux couches.
   **Test runtime réel (workflow 3 steps, run `dafdefb3`)** — validé end-to-end : (a) le mentor a tenu sous pression (refus du code ET du pseudo-code, résistance au « juste cette fois » / « le mentor d'hier », respect du gating ④ Plan, n'a même pas lâché le nom `isLoading`) ; (b) le censeur n'a PAS crié au loup sur la réponse propre (`leak:false`) ; (c) le censeur a attrapé une fuite prose+code volontaire (`leak:true, severity:high`, spans précis). L'archi mentor→censeur tient.
2. ✅ **Fait (2026-07-22, branche `feat/mentor-mode-backend`)** — Fondation données : migration `074_mentor_state.sql` (colonne `mentor_state TEXT` nullable sur `discussions`), enregistrée dans `db/migrations.rs`, + accesseurs dédiés `get_mentor_state` / `set_mentor_state` dans `db/discussions.rs` (pattern `no_agent` : hors struct `Discussion`, lu/écrit directement sur la colonne). `cargo check` OK. Reste : brancher la lecture/écriture depuis `api/mentor.rs`.
3. Module `backend/src/api/mentor.rs` : endpoints ci-dessus, orchestration mentor→censeur via le helper de débat extrait d'`orchestrate`.
   - ✅ **2a fait (2026-07-22)** — Types `MentorState` (+ `MentorStatus`/`MentorPhase`/`MentorSource`/`MentorResource`/`MentorBlock`, ts-rs export) dans `models/mentor.rs` ; handlers `GET`/`PUT /api/mentor/parcours/{disc_id}` (round-trip typé sur la colonne `mentor_state`) ; routes montées dans `lib.rs`. Tests serde (round-trip + forme JSON snake_case + JSON minimal) verts.
   - ✅ **2b-i fait (2026-07-22)** — Machine à états déterministe : logique en méthodes sur `MentorState` (`validate_by_tutor`, `submit`, `advance` = valide un bloc + déverrouille le suivant, `hint` capé à `HINT_MAX`) + endpoints `POST /api/mentor/parcours/{id}/{validate,submit,advance,hint}` (helper `apply` load→mutate→save). 5 tests de transition verts. Types `SubmitBlockRequest`/`AdvanceBlockRequest` propagés dans generated.ts.
   - 🔄 **2b-ii en cours (2026-07-22)** — Approche retenue : **réutiliser le moteur de workflow** (choix user), pas de plomberie agent hand-rolled. Le **moteur du tour mentor→censeur est fait & vérifié** : workflow paramétré `Mode Mentor — tour mentor→censeur` (id `c331d313-e5e5-4645-994b-65c83be32ca6`, Manual, variables `subject`/`block`/`submission`). Run de validation `416697fb` avec un vrai plan d'apprenti → mentor a pointé 2 angles morts (CLS, nombre de placeholders) en questions sans solution ; censeur `leak:false`. **Glue — config faite** : réglage `ServerConfig.mentor_turn_workflow_id` (`Option<String>`, exposé via `ServerConfigPublic`, réglable via `UpdateServerConfigRequest` + POST `/api/config/server`) → le front connaît l'id du workflow sans round-trip. `None` = tour live non câblé (le parcours reste viewer + machine à états). Pas d'endpoint serveur `/turn` : le handler `POST /api/workflows/{id}/trigger` étant un gros endpoint SSE, **le front orchestre le tour** via l'API workflows existante (trigger + poll), puis écrit la réponse vettée via les endpoints mentor (2a/2b-i). **⚠ Périmé le 2026-07-27** — ce choix front-orchestré exposait la sortie brute du mentor au navigateur et laissait la vérification censeur/approbation côté client (forgeable). Rapatrié côté serveur (`POST …/turn` → `run_turn`, fond + poll, fail-closed) ; voir la note de MàJ 2026-07-27 en tête. ✅ **4b fait (2026-07-23)** — Front : la `MentorPage` orchestre le tour live. Sur un bloc learner déverrouillé (parcours réel chargé par disc_id), une zone de saisie → `api.mentor.submit` puis `api.workflows.triggerStream(mentor_turn_workflow_id, …, {subject, block, submission})` ; `onStepDone` capte la sortie `mentor` et parse le verdict `censeur` (bloc `---STEP_OUTPUT---…---`). **Fail-closed** : la réponse n'est révélée que si `leak === false`, sinon avis « filtré par le garde-fou ». Bouton coup de pouce (`api.mentor.hint`). Méthodes `submit`/`hint`/`advance` ajoutées à `api.mentor`. Garde-fou : sur l'aperçu client (sans disc) l'envoi est désactivé. La réponse mentor vit en state composant (pas de champ persistant dans `MentorState`). +10 clés i18n fr/en/es. tsc + lint:i18n + vitest i18n verts.
   - ✅ **Création de parcours faite (2026-07-23)** — `POST /api/mentor/parcours` (`create_parcours`) : crée la disc (profil Mentor + directive stricte, 1er message System épinglé = protocole) + init `MentorState::new_draft` (statut `draft`), insert atomique (disc + message + state). Types `CreateParcoursRequest`/`CreateParcoursResponse`. Front : `api.mentor.createParcours` + formulaire sur l'écran d'accueil de `MentorPage` (titre, objectif, source libre/Jira + clé ticket), +8 clés i18n fr/en/es. Tests + tsc + lint:i18n + vitest verts.

**→ Posture Mentor complète end-to-end : créer (UI) → soumettre → tour mentor→censeur→evaluateur (serveur, fail-closed) → coup de pouce → avancer.** *(« valider (tuteur) » retiré le 2026-07-25 ; tour rapatrié serveur le 2026-07-27 — voir les notes de MàJ en tête.)*
4. ✅ **Increment 3 fait (2026-07-23)** — Setup intelligent (hybride IA→tuteur). **3a** : workflow générateur `Mode Mentor — génération de parcours` (id `30d3d766-5fbf-4c83-8016-de7a6d5c6e06`, variables `subject`/`ticket_key`, sortie TypedSchema `{objective, criteria[], resources[], target_archi, target_tests}`) ; l'agent fetch le ticket Jira via MCP si `ticket_key`, sinon part du sujet libre, et ancre sur la doc IA + le vrai code. Vérifié runtime (run `80ad7764`, sujet Web Components → brouillon de qualité, projet-ancré). **3b** : config `mentor_generator_workflow_id` (miroir de `mentor_turn_workflow_id`) + `CreateParcoursRequest` étendu (`resources`/`target_archi`/`target_tests` optionnels) → `create_parcours` persiste le contenu généré. **3c** : bouton « Générer avec l'IA » dans le formulaire de `MentorPage` (déclenche le générateur via `triggerStream`, parse le `STEP_OUTPUT`, pré-remplit objectif + garde critères/ressources/cible pour le create). +4 clés i18n. tsc + lint:i18n + vitest verts.
   ⚙️ **Activation runtime** : renseigner les 2 ids de workflow dans les settings Kronn — `mentor_turn_workflow_id` = `c331d313-…`, `mentor_generator_workflow_id` = `30d3d766-…` (via POST `/api/config/server` ou l'UI settings).
5. ✅ **4a fait (2026-07-22)** — Front : namespace `mentor` dans `lib/api.ts` (`getParcours`/`putParcours`), `pages/MentorPage.tsx` (+ `.css`) qui rend les 6 blocs gatés depuis un `MentorState` (chargé par disc_id via l'API, ou aperçu client mocké EW-2481), câblage `Dashboard.tsx` (onglet `mentor`, `type Page`, nav, render), 36 clés i18n × fr/en/es. Accent via token `--kr-accent` (→ `#0172f0` sous le thème Euronews). Checks verts : `tsc -b`, `lint:i18n` (parité), vitest i18n (27). Tour live mentor→censeur = 2b (zones apprenti en lecture pour l'instant).
6. Boucle de tuning : logs de fuites du censeur → affinage de la directive Mentor.

## Posture Onboarding (cours) — implémentation

- ✅ **O1/O2/O2c (backend) faits (2026-07-23)** — `MentorState.mode` (`Mentor`|`Onboarding`, défaut `Mentor`, back-compat) + `chapters: Vec<Chapter>` (`Chapter { title, explanation, checkpoint?, done }`, `Checkpoint { question, options[], answer?, reveal? }`) ; persona builtin `mentor-prof` (Théo, explicatif, sans censeur) ; `create_parcours` branché sur le mode ; workflow générateur de cours seedé `mentor-course` (`backend/src/workflows/seeds/mentor-course.json`, TypedSchema `{objective, chapters[]}`) + config `mentor_course_workflow_id` auto-câblée au boot.
- ✅ **O3 (front) fait (2026-07-23)** — Endpoint `POST /api/mentor/parcours/{id}/chapter` (`complete_chapter`). `MentorPage` : sélecteur de posture Mentor/Onboarding + bouton « Générer le cours » (déclenche `mentor-course`, parse `{objective, chapters}`) dans le formulaire ; `ParcoursView` rend une **vue chapitres** en mode onboarding (cartes déverrouillées séquentiellement, checkpoint quiz/exercice, « marquer terminé », progression, badge « Cours », sans garde-fou). i18n fr/en/es. Verts.
- ✅ **O4a — Registre + catalogue (fait, 2026-07-23)** — `docs/onboarding.md` = registre curé (une section `##` par sujet : `Niveau`/`Périmètre`/`Prérequis`/`Références`), artefact doc-IA au même titre que la dette technique. Parser tolérant `core::onboarding_registry::parse_registry` → `Vec<OnboardingTopic>` (tests unitaires). Endpoint `GET /api/mentor/onboarding-catalog/{project_id}` (lit `docs/onboarding.md` du checkout via `scanner::detect_docs_dir`, renvoie `[]` si absent). Front : en mode onboarding le formulaire propose un sélecteur de projet + le **catalogue** des sujets ; cliquer un sujet pré-remplit titre/objectif et injecte ses fichiers de référence dans le sujet de génération. Kronn dogfoode son propre `docs/onboarding.md` (3 sujets).
- ✅ **Ancrage projet de la génération (fait, 2026-07-23)** — les workflows générateurs sont seedés avec `project_id: null` (agnostiques). `TriggerWorkflowRequest` gagne un `project_id` optionnel : le handler `trigger` valide l'id puis **override en mémoire** `wf.project_id` avant `execute_run` (la ligne DB n'est pas touchée — anchor one-shot), donc le runner fournit le checkout du projet choisi (cwd + contexte MCP). `api.triggerStream` accepte un `projectId` ; `MentorPage.generateCourse` passe le projet du catalogue → le cours est généré depuis les VRAIS fichiers du projet. (Le générateur de parcours mentor pourra passer un projet de la même façon quand la posture mentor exposera un sélecteur de projet.)
- ✅ **O4b — Agent d'audit d'onboarding (fait, 2026-07-23)** — branché sur le **pipeline AI Audit** (choix user : « la partie IA Audit »). Nouveau `AuditKind::Onboarding` (models/projects.rs) + `ONBOARDING_STEPS` (1 step, `target_file: docs/onboarding.md`) + arm `kind_to_steps` (api/audit/mod.rs). Le step scanne le projet (doc IA `AGENTS.md`/`repo-map.md`, modules complexes, zones à fort churn via `git log --name-only`, code non documenté), ancre sur de vrais fichiers, et **append** au registre en respectant les priors (ne duplique pas, ne réécrit pas les entrées curées) avec un marqueur `<!-- proposé par l'audit onboarding (date) -->`. Non-validatable (pas de disc de validation : le registre est curé à la main). Parser étendu pour ignorer les commentaires HTML. Front : option « Onboarding » dans `SubAuditModal` (déclenche l'audit via le flux `fullAuditStream` existant avec `kind: 'Onboarding'`). i18n fr/en/es. Le catalogue O4a lit le résultat. → **Boucle complète : audit propose → humain cure `docs/onboarding.md` → catalogue → génération du cours.**

## Distribution (seed builtin) — pour que TOUTE installation ait le Mode Mentor

Choix user (2026-07-23) : au lieu d'objets créés à la main par instance, tout est **seedé/builtin** pour marcher out-of-the-box.
- ✅ **Personas en builtin (fait)** — profils `mentor-socratique` (Noé) + `censeur-mentor` (Garde) dans `backend/src/profiles/*.md`, directive `mentor-no-solution` dans `backend/src/directives/*.md`, tous enregistrés dans `BUILTIN_PROFILES` / `BUILTIN_DIRECTIVES` (ids stables, présents sur toute install). Les objets custom créés en proto via MCP ont été **supprimés** et le workflow-tour repointé sur les ids builtin. Tests builtin verts (24).
- ✅ **Seed des workflows (fait, Part 2 — 2026-07-23)** — nouveau mécanisme de seed idempotent. Les 2 workflows sont embarqués en JSON (`backend/src/workflows/seeds/{mentor-turn,mentor-generator}.json`, référençant les personas builtin), ids stables `mentor-turn` / `mentor-generator`. `db::workflows::ensure_mentor_workflows(conn)` désérialise + insère si absent (respecte les éditions opérateur). Appelé au **boot** (`main.rs`, après ouverture DB) qui **auto-câble** `mentor_turn_workflow_id` / `mentor_generator_workflow_id` s'ils sont vides. Tests de désérialisation verts. → **Toute installation a désormais personas + workflows + câblage, zéro manip.** Le menu déroulant Réglages devient un simple override optionnel (non implémenté ; plus nécessaire pour le fonctionnement de base). Les workflows créés à la main en proto (`c331d313`, `30d3d766`) sont désormais redondants sur cette instance.
