# UX Review — Refonte Workflow Engine (0.7.0)

> **Audience** : designers UX + utilisateurs Kronn invités à la session de review.
> **Objectif** : collecter du feedback structuré sur 7 nouvelles features pour itérer l'UI **sans toucher au moteur d'exécution** (les invariants backend sont fixés). Les corrections proposées concerneront le wizard, le RunDetail, et la documentation in-app.
> **Durée recommandée** : 2h en groupe, ou 30 min en interview individuelle par feature prioritaire.

---

## Contexte rapide

Kronn est passé d'un moteur de workflow simple (steps Agent en série) à un moteur expressif inspiré de l'Auto-Dev YAML : 7 nouvelles primitives pour modéliser des cas réels (limites de coût, validation humaine, fan-out de tâches, exécution shell, boucles d'auto-correction, rollback sur échec, etc.).

Le **mécanisme** est solide (1481 tests backend verts, sécu validée). L'**UI**, elle, expose juste de quoi utiliser les features sans patterns guidés. Les designers vont nous aider à transformer ces primitives en flows que l'utilisateur lambda comprend en 2 minutes.

**Important** : on ne refait PAS le wizard de zéro. Toute proposition doit s'intégrer dans le wizard existant (création + édition de workflow) et le RunDetail (visualisation d'un run).

---

## Tableau récap des 7 features

| Feature | Priorité UX | État actuel | Problème principal |
|---|---|---|---|
| **Loops + state** | 🔴 HAUTE | Goto dans une `condition` + champs `iter` / `state` en text | Le pattern « boucle » n'est pas un objet de 1ère classe — l'user doit composer 4 primitives à la main |
| **Rollback** | 🔴 HAUTE | Section dédiée dans wizard, Notify uniquement | L'utilisateur ne comprend pas QUAND ça se déclenche ni pourquoi avoir plusieurs étapes |
| **Gate** | 🟠 MOYENNE | Bouton "Gate" + panel décision dans RunDetail | OK fonctionnel, mais le mental-model "pause→reprise" est bare-bones |
| **Exec** | 🟠 MOYENNE | Bouton + allowlist + dropdown binaire | Discovery : l'allowlist est dans Config, le step est dans Steps — l'user se cogne au "désactivé" sans savoir où aller |
| **Artifacts** | 🟡 BASSE | Déclaration champ par champ dans Config avancée | Pas de preview des artifacts produits dans RunDetail |
| **Guards** | 🟡 BASSE | Card visible dans Config | Déjà bien — l'UX a été reviewée par Antoine |
| **TypedSchema** | ⚪ AUCUNE | Invisible (utilisé en interne par output_format) | Pas d'enjeu user-facing |

---

## Détail par feature

Pour chaque feature : **quoi** (1 phrase), **où dans l'UI** (chemin), **ce qui marche** (UX wins déjà en place), **ce qui bloque** (problème observé), **ce qu'on ne peut PAS toucher** (invariants backend — voir aussi la section dédiée en fin de doc).

### 1. Loops + state vars 🔴

**Quoi** : permet à un step de boucler en arrière (Goto), avec un compteur d'itération et une mémoire d'état durable entre passages.

**Où** :
- Wizard → step → bouton "+ Condition custom" → action **Goto** + champ numérique `× N`
- Variables `{{iter.<step>}}` et `{{state.<key>}}` listées dans le panneau "Chaînage entre steps" (texte uniquement, non-cliquable)
- Côté agent : l'agent écrit `---STATE:clé=valeur---` dans son output (texte libre, aucune aide UI)

**Ce qui marche** :
- Le mécanisme est complet : compteurs auto, max_iterations par-edge, state persisté en DB
- Back-compat avec les workflows existants (Goto sans cap fonctionne toujours)

**Ce qui bloque** :
- L'utilisateur doit savoir 4 conventions distinctes (Goto, max_iterations, `{{iter}}`, `---STATE:---`) pour assembler ce que l'utilisateur appelle "une boucle"
- Pas de visualisation du retour-en-arrière dans la liste des steps (juste une chaîne linéaire)
- Le step cible Goto est un input texte libre — typo silencieuse possible

**Pistes UX déjà identifiées** (à valider/écarter en session) :
- Un widget "Loop block" qui regroupe visuellement 2+ steps + leur condition de retour
- Génération auto des instructions agent (`"écris ---STATE:..."` en fin de prompt)
- Dropdown des steps existants au lieu d'un free-text

### 2. Rollback / compensation 🔴

**Quoi** : étapes secondaires qui s'exécutent UNIQUEMENT si le pipeline principal termine en `Failed`. Pour notifier ops, revert un déploiement, etc.

**Où** :
- Wizard → onglet Steps → en bas, après les steps principaux : section **"Rollback / compensation"** (cadre dashed ambre)
- Wizard expose Notify uniquement ; Agent / ApiCall en rollback restent dispos via API

**Ce qui marche** :
- Section visuellement distincte des steps principaux
- Variables spéciales `{{failed_step.name}}` et `{{failed_step.output}}` injectées
- Refus de Gate en rollback (validé save-time avec message d'erreur)

**Ce qui bloque** :
- L'utilisateur ne comprend pas que `Failed` ≠ `Cancelled` ≠ `StoppedByGuard` ≠ `Reject` (les 4 états où le rollback peut/ne peut pas tirer)
- Plusieurs étapes = ordre séquentiel + stop sur premier échec → règle non-évidente
- Pas de bouton "tester ce rollback en dry-run"

**Pistes UX déjà identifiées** :
- Une diagramme du flow "Pipeline OK → fin / Pipeline KO → rollback runs"
- Un toggle "fail-only" vs "fail-or-cancel" (extension future si demande)

### 3. Gate (validation humaine) 🟠

**Quoi** : un step qui met le run en pause, attend une décision humaine via 3 boutons (Approuver / Demander des changements / Rejeter), puis continue ou jump.

**Où** :
- Wizard : bouton "Gate" dans le sélecteur de step type → form avec textarea message + dropdown cible "request changes"
- RunDetail : panel jaune avec les 3 boutons quand `run.status === 'WaitingApproval'`

**Ce qui marche** :
- Le panel se distingue clairement du reste du RunDetail (couleur ambre, icône main)
- Les 3 boutons ont des couleurs sémantiques (vert / ambre / rouge)
- Le commentaire est requis pour "request changes" avec validation inline

**Ce qui bloque** :
- L'utilisateur ne réalise pas qu'un Gate **bloque la consommation de tokens** — c'est un avantage clé non-mis-en-avant
- Pas d'indication du **temps écoulé en pause** (un run en `WaitingApproval` depuis 3h passe inaperçu)
- Pas de notification quand un Gate fire (Slack, email, push, …)

**Pistes UX déjà identifiées** :
- Compteur "en attente depuis Xh" sur la Run card
- Auto-rappel email/Slack après N heures (extension future)

### 4. Exec (shell direct) 🟠

**Quoi** : un step qui invoque un binaire allowlisté (`cargo test`, `npm build`, `make deploy`) directement, sans agent ni shell.

**Où** :
- Config → onglet **"Allowlist Exec"** : input séparé par virgules pour autoriser des binaires
- Wizard → step → bouton "Exec" → form avec dropdown du binaire + textarea args + timeout
- RunDetail : badge `EXEC` (rouge) sur les step results

**Ce qui marche** :
- Couleur rouge cohérente partout (signal "feature sensible")
- Dropdown des binaires (vs free-text → impossible de typer un binaire non-autorisé)
- Warning rouge "Exec désactivé, va dans Config" quand l'allowlist est vide

**Ce qui bloque** :
- L'allowlist est dans Config (onglet 3) mais le bouton Exec est dans Steps (onglet 2) — désorientation
- L'utilisateur ne comprend pas pourquoi l'allowlist existe (la sécu n'est pas expliquée)
- Args = textarea ligne-par-ligne — pas de feedback "votre arg #2 contient `{{steps.X}}`, voici ce qu'il rendra"
- Aucun lien vers les patterns courants (`cargo test`, `npm run build`)

**Pistes UX déjà identifiées** :
- Bouton "Configurer l'allowlist →" dans le warning (lien direct vers Config)
- Templates pré-remplis ("Tester avec cargo / npm / pytest")
- Preview live du rendu des args templatés

### 5. Artifacts 🟡

**Quoi** : des outputs de step persistés en fichier dans le workspace, référençables via `{{artifacts.<nom>}}`.

**Où** :
- Wizard → onglet Config → section "Artifacts" (avancée)
- Côté agent : l'agent écrit `---ARTIFACT:nom---\ncontenu\n---END_ARTIFACT---`

**Ce qui marche** :
- Pre-seeding à `""` permet de référencer un artifact avant qu'il soit produit (round 1 d'une boucle)
- Validation save-time des paths (pas de `..`, pas d'absolu)

**Ce qui bloque** :
- Pas de preview des artifacts produits dans RunDetail (l'user voit le step output mais doit aller dans le worktree pour voir le fichier)
- Pas d'indication "round 1 a écrit l'artifact `plan.md`, round 2 va le lire"

### 6. Guards (limites d'exécution) 🟡

**Quoi** : timeout wall-clock + max appels LLM + détection de boucle. Filets de sécurité contre les runs runaway.

**Où** : Wizard → onglet Config → card **"Limites d'exécution"** (visible par défaut, pas en avancé)

**Ce qui marche** : déjà reviewé par Antoine UX, le naming "Limites d'exécution" remplace "Guards", placement first-class. RunStatus `StoppedByGuard` distinct (orange shield, pas rouge erreur). Bon point de référence pour ce que les autres features pourraient devenir.

**Ce qui bloque** : pas grand-chose. Éventuel : le concept "max revisits per step" est obscur pour qui ne fait pas de loops.

### 7. TypedSchema ⚪

Invisible côté user. Utilisé en interne via `output_format` dans le wizard expert. Pas d'enjeu UX.

---

## Scénarios à faire tester

Donner ces tâches aux utilisateurs **sans guidage**, observer là où ils bloquent. Chaque scénario teste 1-2 features.

### Scénario A — Auto-Dev loop (loops + state)
> "Tu veux qu'un agent implémente une feature, puis qu'un autre la review. Si la review n'est pas concluante, on retourne implementer en tenant compte du feedback. Maximum 5 tentatives. Crée ce workflow dans Kronn."

**Ce qu'on observe** :
- L'utilisateur trouve-t-il `Goto` ? (probablement non — c'est dans une "condition")
- Comprend-il le `× 5` ?
- Sait-il comment passer le feedback de la review à l'implement suivant ? (`{{state.last_review}}`)

### Scénario B — Pipeline avec validation humaine (gate)
> "Avant de merger un PR, tu veux qu'un humain valide. Si la review humaine demande des changements, on retourne au step `implement`. Si elle approuve, le workflow continue. Si elle rejette, le run échoue."

**Ce qu'on observe** :
- L'utilisateur trouve-t-il le bouton "Gate" ?
- Comprend-il la différence entre "Demander des changements" et "Rejeter" ?
- Sait-il comment configurer la cible "Demander des changements" → `implement` ?

### Scénario C — Tests + rollback (exec + rollback)
> "Tu veux lancer `cargo test` après chaque déploiement. Si les tests échouent, tu veux notifier l'équipe ops sur Slack avec le message d'erreur."

**Ce qu'on observe** :
- L'utilisateur sait-il qu'il faut configurer l'allowlist Exec d'abord ?
- Comment fait-il le pont entre "exec a échoué" et "déclenche une notification" ?
- Comprend-il que la notification va dans la section Rollback (pas dans les steps principaux) ?

### Scénario D — Audit avec timeout (guards)
> "Tu veux qu'un agent fasse un audit du repo. Le run ne doit pas dépasser 30 minutes ni 50 appels d'IA, sinon il s'arrête de lui-même."

**Ce qu'on observe** :
- L'utilisateur trouve-t-il "Limites d'exécution" sans aide ?
- Comprend-il les 3 limites distinctes ?

### Scénario E — Reprise après pause (gate resume)
> "Un run est en pause depuis 5 minutes (status WaitingApproval). Trouve-le et approuve-le."

**Ce qu'on observe** :
- L'utilisateur identifie-t-il rapidement le run en pause vs un run terminé ?
- Comprend-il qu'il consomme zéro tokens pendant la pause ?

---

## Questions ciblées par feature

À utiliser après que l'utilisateur a réalisé les scénarios. **Pas de question ouverte du type "qu'en penses-tu ?"** — questions précises qui révèlent le mental model.

### Loops
1. Comment dirais-tu, en une phrase, ce que fait `× 5` à côté du Goto ?
2. Sans regarder l'aide, comment ferais-tu pour passer le feedback d'une review à l'itération suivante de l'implement ?
3. Si le `× 5` est atteint et que la condition Goto est toujours vraie, qu'est-ce qui se passe ? Tu t'attendrais à quoi ?

### Rollback
1. Le rollback se déclenche dans quels cas ? Coche : Failed / Cancelled / StoppedByGuard / Reject d'un Gate.
2. Si tu mets 3 étapes de rollback et que la 1ère échoue, qu'est-ce qui se passe pour les 2 autres ?
3. Pourquoi est-ce qu'on ne peut pas mettre un Gate dans le rollback ?

### Gate
1. Que coûte une pause de 24h sur un run Gate, en tokens ? (réponse attendue : 0)
2. La différence entre "Demander des changements" et "Rejeter" — c'est quoi en pratique ?
3. Si tu approuves un Gate, le run reprend depuis quel step ?

### Exec
1. Pourquoi tu dois définir une allowlist avant d'utiliser un step Exec ?
2. Si un step précédent retourne `; rm -rf /` dans son summary et que tu l'utilises dans un arg de `npm test --grep "{{steps.X.summary}}"`, qu'est-ce qui s'exécute ?
3. Tu veux mettre `bash -c "echo hi"` dans l'allowlist. Pourquoi est-ce refusé ?

### Guards
1. La différence entre "max appels IA" et "détection de boucle" ?
2. Si le run s'arrête sur un guard, c'est un échec ?

### Artifacts
1. Tu écris `{{artifacts.plan}}` dans le step 1 mais aucun step n'a encore produit cet artifact. Qu'est-ce que tu vois dans le prompt rendu ?

---

## Non-négociables backend

Le designer / le testeur peuvent proposer ce qu'ils veulent côté UI, mais **les invariants suivants ne bougent pas** sans une discussion d'archi séparée. Ils sont là pour la sécurité, la rétro-compatibilité, ou les contraintes du moteur.

### Gate
- ❌ Un Gate ne peut PAS être dans `on_failure` (rollback). Le run est déjà `Failed`, donc la reprise via `decide` est conceptuellement bloquée. Erreur save-time, pas négociable.
- ❌ Le RunStatus `WaitingApproval` est terminal pour le runner — il ne tourne pas en arrière-plan en attendant. Le worktree est préservé, mais aucun process ne consomme de ressources. **Les designers ne peuvent pas demander un "auto-resume après timeout"** sans qu'on retravaille le moteur.
- ✅ La cible "Demander des changements" peut être n'importe quel step antérieur (par défaut : le précédent).

### Exec
- ❌ Pas de `sh -c`, jamais. Si un user demande des pipes (`cmd | tee`), il fait 2 steps successifs.
- ❌ L'allowlist est **par-workflow**, pas globale. Décision : éviter qu'un workflow malveillant (importé, partagé) bypass un user en réutilisant l'allowlist d'un autre.
- ❌ Args = argv séparés. Pas de `command: "rm -rf {{var}}"` qui se split. Le séparateur est forcé : `command: "rm"`, `args: ["-rf", "{{var}}"]`.
- ❌ Pas de regex dans l'allowlist. Match exact sur le nom du binaire. Décision : la regex = trou de sécu typique (`*` qui matche `bash` parce que mal écrite).
- ✅ Le binaire est résolu par le `PATH` du process Kronn — pas chrooté. Si l'admin a installé `cargo` globalement, l'allowlist `["cargo"]` le trouve.

### Rollback
- ❌ Fire UNIQUEMENT sur `Failed`. Pas sur `Cancelled` (l'user a explicitement stoppé), pas sur `StoppedByGuard` (le système s'est protégé), pas sur `Reject` d'un Gate (l'user a explicitement rejeté). Décision : "rollback = filet automatique pour pannes", pas "rollback = exécute toujours en fin".
- ❌ Pas de récursion : si une étape de rollback échoue, les suivantes sont skippées et le run reste `Failed`. Pas de "rollback du rollback".

### Loops
- ❌ Pas de "while true" sans cap. Soit `max_iterations` par-edge (Phase 6), soit le guard global `loop_detection_max_revisits`. Au moins un des deux doit fire à un moment.
- ❌ Le state map persiste **entre runs** (sur la run row), mais **pas entre workflows**. Si tu veux du state cross-workflow, c'est un autre concept (cache global) qui n'existe pas encore.

### Guards
- ❌ Le timeout est wall-clock, pas "active time". Si un Gate met le run en pause 6h et que `timeout_seconds=3600`, le run est `StoppedByGuard` au resume. Décision Senior Dev : "active-time" leak du state complexe entre reboots.

### Artifacts
- ❌ Path workspace-relative. Pas d'absolu, pas de `..`. Validé save-time.
- ❌ Pas de versioning automatique. Si 2 steps produisent le même artifact, le 2nd écrase le 1er.

### Général
- ❌ Le runner exécute **un step à la fois**. Pas de parallélisme intra-run (BatchQuickPrompt fait du fan-out de discussions, ce qui est différent — c'est plusieurs runs filles).
- ❌ Pas de modification du workflow pendant un run en cours. L'utilisateur peut éditer la définition, mais le run en cours utilise la version snapshotée à son démarrage.

---

## Format de feedback à viser

À la fin de la session, on veut produire pour chaque feature :
- 1-3 pistes UX concrètes (mockup à faire) avec priorité estimée
- Une décision claire : "on garde l'UI actuelle / on itère / on retravaille"
- Pour les pistes "on retravaille" : un ticket avec contexte + invariants à respecter

Ne pas viser un consensus à tout prix. Si 2 designers proposent 2 directions opposées, on note les 2 et on tranche post-session.

---

## Annexes utiles pendant la session

- Captures d'écran des 7 features dans leur état actuel : à produire avant la session (en TODO)
- Les 4 mémos UX déjà flaggés : `feedback_gate_ux_pending`, `feedback_rollback_ux_pending`, `feedback_loop_ux_pending`, `feedback_exec_ux_pending` — résumés courts du problème vu par l'auteur du code
- Les tests backend correspondants pour comprendre ce qu'on ne peut pas casser : `backend/src/workflows/{exec_step,gate_step,runner}.rs`
