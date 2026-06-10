# Review collaborative par conversation (plan ↔ reviewer)

Statut : **design** (2026-06-13, idée user). Remplacerait la boucle WF `triage → plan_review → Goto(triage)` du feasibility-autopilot par un **débat en discussion partagée** entre le planificateur et le reviewer.

## Le problème actuel

Phase « deux cerveaux » d'aujourd'hui = des étapes WF successives :
```
triage(opus) → plan_lint → plan_review(Codex) → NEEDS_RETRIAGE → triage(opus) → … → gate
```
Coûts mesurés (run 11b, EW-7247) : plan_review **191k puis 135k** = **327k / 752k (43 % du run)**. Pourquoi si cher : à **chaque** round le reviewer relit **tout depuis zéro** (≈20 fichiers + manifeste 30k + lint). Le re-triage est devenu incrémental (`unchanged[]`, ~35k) mais la **relecture reviewer reste from-scratch**.

Conséquences : (1) coût dominé par les relectures répétées ; (2) on a dû capper à 2 rounds → un reviewer adversarial trouve quasi toujours de NOUVEAUX problèmes (run 11b round 2 : 3 findings inédits) → des trous réels passent à la gate humaine non résolus.

## L'idée (user)

> Kronn crée une conversation pour ce WF, le 1ᵉʳ agent (planificateur) fait le plan et s'y connecte, invite le 2ᵉ (reviewer) ; les deux doivent parvenir à un accord. Ça crame infiniment moins de tokens.

## Pourquoi c'est moins cher (analyse honnête)

Dans une discussion orchestrée, le reviewer lit le code+plan **une seule fois** (1er tour ≈130k), puis chaque tour suivant ne lit que **le delta de conversation** (les messages échangés, mis en **cache-résumé** quand le transcript grossit — déjà implémenté dans `orchestrate`). Vs la boucle WF qui re-paie 130-190k de relecture **à chaque** round.

- WF loop, 2 rounds : ~270-380k.
- Discussion, 3 tours : ~130k (1ʳᵉ lecture) + N×(delta 10-30k) ≈ **~200k pour tout le débat**, ET on peut se permettre **plus** de tours.

→ Double gain : **moins cher par tour** + **plus de tours abordables** = meilleure convergence. Ça adresse À LA FOIS la reco coût ET le « cap trop bas » (la review est la partie la plus importante).

⚠️ Ce n'est pas « infini » : la 1ʳᵉ lecture du code reste incompressible, et le transcript grandit (quadratique sans le cache-résumé — qui existe). Gain réaliste : **-30 à -50 % sur la phase plan**, + convergence.

## La fondation existe déjà

`POST /api/discussions/:id/orchestrate` (`src/api/discussions/orchestration.rs`) : débat N agents sur `max_rounds` (cap 3 actuel), chaque agent lit le transcript, **synthèse entre rounds**, message d'accord final, **cache-résumé** des vieux messages quand le budget serre. Primitives MCP : `disc_create_room`, `disc_invite_peer`, `disc_wait_for_peer`, `disc_join`, `disc_append`, `disc_summarize`. Collab multi-agent **déjà prouvée live** (cf. [[project_cross_agent_collab_demo]] — Claude+Codex+Vibe sur une disc partagée).

## Deux options d'intégration au WF

### Option A — nouveau `StepType::CollaborativePlan` (déterministe, robuste)
Le moteur : crée une disc, lance le planificateur (opus) avec la tâche triage, invite le reviewer (Codex), boucle les tours via la logique `orchestrate`, détecte la convergence (protocole `[CONSENSUS: APPROVED]` des deux), extrait le manifeste final que le planificateur écrit dans `.kronn/triage-manifest.md`, puis l'ingère comme aujourd'hui (TypedSchema + dérivation machine-files).
- ✅ Déterministe, garde-fous (timeout, cap tours, anti-deadlock), s'intègre à l'ingestion + gate existantes.
- ❌ Nouveau StepType = travail moteur conséquent.

### Option B — le triage AGENT pilote le débat via MCP (agentique, zéro nouveau StepType)
Le prompt du step triage actuel devient : « crée une room (`disc_create_room`), invite le reviewer (`disc_invite_peer`), débattez jusqu'à accord, puis écris le manifeste final ». L'agent orchestre depuis son propre step.
- ✅ Aucun changement moteur ; colle à la description littérale du user.
- ❌ Moins déterministe (dépend de l'agent qui pilote bien), détection de convergence + garde-fous plus faibles, observabilité moindre.

## Recommandation

**Option A**, mais en réutilisant au maximum la logique `orchestrate` existante (extraire son cœur en fonction appelable par le moteur, pas seulement par l'endpoint SSE). Protocole de convergence explicite + cap tours + fallback gate (si pas d'accord au cap → la gate humaine voit le désaccord, comme aujourd'hui). Cela **remplace** `plan_review` + le `Goto(triage)` + l'incrémental-re-triage par une seule phase « plan débattu ». Le `plan_lint` déterministe (0 token) reste en garde-fou.

## Prochaines étapes
1. Extraire le cœur de débat de `orchestrate` en helper moteur (`run_debate(disc_id, agents, max_rounds, convergence_marker) -> transcript+verdict`).
2. `StepType::CollaborativePlan` (ou réutiliser un step Agent spécial) : crée disc (worktree-bound) → débat planner(opus)+reviewer(Codex) → manifeste final → ingestion.
3. Protocole convergence `[CONSENSUS: APPROVED]` / `[CONSENSUS: BLOCKED <raison>]` + cap (ex. 4 tours) + fallback gate.
4. A/B coût vs run 11b (327k phase plan) sur EW-7247.
