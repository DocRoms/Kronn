# Registre d'onboarding

> Catalogue des parcours **Onboarding** du Mode Mentor (posture explicative, façon cours).
> Chaque sujet devient un cours à chapitres généré depuis le vrai code du projet.
>
> Ce fichier est un artefact de doc-IA, au même titre que `inconsistencies-tech-debt.md` :
> il est **curé à la main** ET **enrichi par l'agent d'audit d'onboarding** (qui repère les
> sous-systèmes complexes, les zones à fort churn et le code non documenté).
>
> **Format** — une section `##` par sujet, avec ces puces (labels FR ou EN, tolérants) :
> `- **Type** :` (tronc \| branche \| capstone \| culture) · `- **Niveau** :` · `- **Périmètre** :`
> · `- **Prérequis** :` · `- **Références** :` (chemins de fichiers/docs séparés par des virgules).
>
> Quand la posture onboarding **génère** le cours d'un sujet, ses chapitres sont persistés dans
> `docs/onboarding/NN-<slug>.md` et une puce `- **Cours** : docs/onboarding/NN-<slug>.md` est ajoutée
> automatiquement à la section correspondante. Ce dossier + ce lien sont gérés par le générateur —
> ne les édite pas à la main (comme `docs/tech-debt/TD-*.md` pour la dette technique).
> Le `Type` structure le catalogue en curriculum : **tronc** (à voir en premier) → **branches**
> (spécialisations) → **capstone** (projet de synthèse) → **culture** (normes d'équipe).
> Tout texte libre sous les puces sert de description. Voir `docs/design/mentor-mode.md`.

## Prise en main & architecture
- **Cours** : docs/onboarding/01-prise-en-main-architecture.md
- **Type** : tronc
- **Niveau** : débutant
- **Périmètre** : lancer Kronn en local, comprendre les 3 services (backend/frontend/gateway) et le trajet d'une requête API du routeur axum jusqu'au handler.
- **Prérequis** : aucun.
- **Références** : `docs/AGENTS.md`, `docs/architecture/overview.md`, `backend/src/lib.rs`, `Makefile`, `kronn`

Le premier jour : `./kronn start` / `make`, la stratégie de chargement de contexte par tiers de `docs/AGENTS.md`, et comment `build_router()` (`backend/src/lib.rs`) branche les routes sur les handlers d'`api/`. C'est la carte mentale qui rend tous les autres sujets lisibles.

## Le moteur de workflow
- **Type** : branche
- **Niveau** : intermédiaire
- **Périmètre** : comprendre comment un workflow s'exécute — steps, contrat inter-step (envelope `---STEP_OUTPUT---`), guards et boucles.
- **Prérequis** : « Prise en main & architecture », bases de Rust.
- **Références** : `backend/src/workflows/runner.rs`, `backend/src/workflows/steps.rs`, `backend/src/workflows/step_output_format.rs`, `docs/architecture/overview.md`

Le cœur de Kronn : un pipeline déterministe qui orchestre des agents et des appels directs (ApiCall, Exec, JsonData). Bon sujet pour saisir comment les données circulent d'un step au suivant.

## Le Mode Mentor (deux postures)
- **Type** : branche
- **Niveau** : intermédiaire
- **Périmètre** : la feature pédagogique elle-même — posture socratique (mentor→censeur, strict absolu) vs posture onboarding (cours explicatif à chapitres).
- **Prérequis** : « Prise en main & architecture ».
- **Références** : `docs/design/mentor-mode.md`, `backend/src/models/mentor.rs`, `backend/src/api/mentor.rs`, `frontend/src/pages/MentorPage.tsx`

Comment un `MentorState` porte le parcours sur une discussion, comment le gating et les checkpoints sont appliqués, et comment le front rend les deux postures.

## Les plugins MCP & API
- **Type** : branche
- **Niveau** : débutant
- **Périmètre** : comment Kronn expose des capacités aux agents — serveurs MCP synchronisés sur disque vs plugins API (injection de prompt + curl).
- **Prérequis** : « Prise en main & architecture ».
- **Références** : `backend/src/core/mcp_scanner.rs`, `docs/AGENTS.md`

La distinction MCP / API / hybride et la façon dont un agent découvre les outils disponibles.

## Ajouter un endpoint API de bout en bout
- **Type** : capstone
- **Niveau** : intermédiaire
- **Périmètre** : première vraie tâche guidée — traverser modèle Rust → handler `api/` → route dans `build_router()` → `make typegen` → consommation côté front.
- **Prérequis** : tout le tronc.
- **Références** : `docs/repo-map.md`, `backend/src/models/mod.rs`, `backend/src/api/workflows.rs`, `backend/src/lib.rs`, `frontend/src/lib/api.ts`

Synthèse : on définit un modèle (source de vérité Rust), on écrit le handler, on enregistre la route, on régénère les types TS, on branche le front. Reprend un vrai exemple d'`api/` comme patron.

## Qualité & conventions (comment ne pas casser la CI)
- **Type** : culture
- **Niveau** : débutant
- **Périmètre** : les règles non négociables — `cargo clippy -- -D warnings`, tests obligatoires pour tout changement, `make typegen` après un modèle, ne jamais éditer `generated.ts` à la main.
- **Prérequis** : aucun.
- **Références** : `docs/AGENTS.md`, `docs/testing-quality.md`, `docs/coding-rules.md`

À voir tôt : la barre qualité de l'équipe (clippy strict, tests systématiques, types générés depuis Rust) et les pièges qui cassent la CI. Marqueur culturel fort du projet.

<!-- proposé par l'audit onboarding (2026-07-28) -->
## La couche d'exécution des agents CLI (agent runner)
- **Type** : branche
- **Niveau** : avancé
- **Périmètre** : comment Kronn détecte, résout et spawn les binaires d'agents (Claude/Codex/Gemini/Vibe/Kiro/Copilot) puis streame leur stdout en SSE avec suivi de tokens.
- **Prérequis** : « Prise en main & architecture », bases de Rust.
- **Références** : `backend/src/agents/mod.rs`, `backend/src/agents/runner.rs`, `docs/operations/host-mcp-runtime.md`, `docs/AGENTS.md`

`runner.rs` (le plus gros fichier du repo) et `agents/mod.rs` sont le cœur d'exécution que le sujet « moteur de workflow » ne touche pas : résolution host-aware (Docker/WSL/macOS), prompt via stdin pour contourner `ARG_MAX`, modes Text vs StreamJson, et le parsing du flux pour le comptage de tokens.

<!-- proposé par l'audit onboarding (2026-07-28) -->
## Désagentification : le moteur d'appels API directs et sa sécurité
- **Type** : branche
- **Niveau** : avancé
- **Périmètre** : le step `ApiCall`/Quick API qui appelle une API depuis le moteur Rust (0 token), extrait du JSON via JSONPath, et se protège contre SSRF/DNS-rebind + fuite de secrets.
- **Prérequis** : « Le moteur de workflow », « Les plugins MCP & API ».
- **Références** : `backend/src/workflows/api_call_executor.rs`, `backend/src/workflows/api_call_security.rs`, `backend/src/api/quick_apis.rs`, `docs/operations/deagent-apicall.md`

Le moteur zéro-token, distinct de la découverte d'outils : trois gardes de sécurité (allowlist d'hôtes, blocage DNS-rebind, redaction des secrets) + refresh OAuth2 transparent. Fort enjeu sécurité, entièrement doc-couvert.

<!-- proposé par l'audit onboarding (2026-07-28) -->
## Anti-hallucination et provenance vérifiée
- **Type** : culture
- **Niveau** : intermédiaire
- **Périmètre** : la signature technique du projet — chaque assertion porte une citation `[src: file:...]` vérifiée mécaniquement, avec un checker de faithfulness informatif.
- **Prérequis** : « Prise en main & architecture ».
- **Références** : `backend/src/core/anti_halluc.rs`, `backend/src/core/faithfulness.rs`, `docs/conventions/agents-md-format-v1.md`, `docs/AGENTS.md`

Le §0 de `docs/AGENTS.md` (protocole anti-hallucination + grammaire de citations) est le marqueur culturel le plus fort du projet. `anti_halluc.rs` blinde les prompts, `faithfulness.rs` vérifie claim ⊨ evidence (verdict informatif, jamais auto-bloquant).

<!-- proposé par l'audit onboarding (2026-07-28) -->
## Collaboration multi-utilisateur et fédération P2P
- **Type** : branche
- **Niveau** : avancé
- **Périmètre** : comment des instances Kronn se découvrent (invite codes, Tailscale/LAN), partagent des discussions et fédèrent les messages en temps réel via WebSocket.
- **Prérequis** : « Prise en main & architecture ».
- **Références** : `backend/src/api/ws.rs`, `backend/src/core/ws_client.rs`, `backend/src/api/federation.rs`, `backend/src/core/tailscale.rs`

Sous-système entier peu couvert par `repo-map.md` : présence peer-to-peer et broadcasts (`ws.rs`), connexions sortantes avec backoff (`ws_client.rs`), diffusion d'un message aux instances pairs (`federation.rs`), détection réseau (`tailscale.rs`).

<!-- proposé par l'audit onboarding (2026-07-28) -->
## Le stack d'injection de contexte : skills, profiles, directives
- **Type** : branche
- **Niveau** : intermédiaire
- **Périmètre** : les trois couches parallèles (builtin embarqué + custom disque) qui composent le prompt système d'un agent, et leur synchronisation en fichiers natifs pour la découverte progressive.
- **Prérequis** : « Prise en main & architecture », « La couche d'exécution des agents CLI ».
- **Références** : `backend/src/core/skills.rs`, `backend/src/core/profiles.rs`, `backend/src/core/directives.rs`, `backend/src/core/native_files.rs`

`skills.rs` expose `BUILTIN_SKILLS` (embarqués au compile-time) et `build_skills_prompt`, mêmes patterns pour profiles/directives. `native_files.rs` écrit les `SKILL.md` dans `.claude/skills/`, `.gemini/skills/`, etc. Le levier distinctif par lequel Kronn oriente le comportement des agents.
