# Aider un modèle local sur une tâche longue — ce qui a été essayé, et mesuré

2026-09-21. Six modèles Ollama locaux, plus OpenRouter et NVIDIA. Le scénario de
référence se rejoue avec les bancs cités en fin de page. Les quatre variantes,
elles, **ne sont pas dans l'arbre** : rejetées, leurs correctifs n'ont pas été
commités. Leurs chiffres se lisent, ils ne se rejouent pas sans réécrire la
variante.

**Lire ceci avant de proposer un mécanisme d'aide aux agents.** Quatre idées
plausibles, conçues indépendamment par des experts, ont été construites et
mesurées : deux ont **dégradé** le résultat, deux n'ont **jamais vu leur
déclencheur** se produire et ne sont donc pas jugées. Elles sont décrites ici
avec ce que chacune établit, et ce qu'elle n'établit pas.

Portée : **une** forme de tâche (un inventaire décomposable en unités
listables), **un** dépôt, une fenêtre qui déborde. Rien ici ne montre qu'une
information de progression est nuisible en général.

## La règle qui en découle

> **Toute aide comportementale ou modification de réglage doit démontrer un
> bénéfice sur plusieurs modèles et tâches représentatives avant activation.
> Les informations nécessaires à l'exactitude du protocole restent
> obligatoires, et leur présentation se mesure séparément.**

La distinction compte : une consigne, un bilan d'avancement ou un réglage
d'échantillonnage sont des aides comportementales et doivent faire leurs
preuves. Un outil qui a échoué, un résultat raccourci, une permission refusée
sont des faits de protocole — les taire ferait mentir le run, donc ils partent
quoi qu'il en coûte. Un mécanisme comportemental sans effet prouvé n'est pas
neutre : il déplace le comportement du modèle, et on impute ensuite la
dégradation au modèle.

Corollaire vérifié ce jour-là : un mécanisme qui « ne peut pas nuire » a fait
passer un modèle de 9 unités couvertes avec lecture observée et aucune
affirmation sans lecture, à 2 couvertes et 7 affirmations sans lecture observée.

## Le scénario

Inventorier les 20 fichiers PHP de `src/` d'un vrai dépôt Symfony, une ligne par
fichier, fenêtre 24 576 tokens. Le contexte déborde, donc tenir tout en tête ne
suffit pas. Banc : `bench_inventory_of_a_repository`, dont le contrôle court
est `bench_four_facts_from_a_repository`.

**Trois mesures distinctes, à ne pas confondre** :

1. **couverture avec lecture observée** — l'unité est nommée dans le livrable
   ET son fichier a été ouvert pendant le run ;
2. **affirmation sans lecture observée** — l'unité est nommée sans qu'aucune
   lecture de sa source n'ait été observée. Ce n'est pas « fabrication
   détectée » : une preuve peut venir d'un contexte initial, d'un diff, d'un
   tour précédent ou d'une lecture partielle ;
3. **exactitude** — la description est-elle juste. **Non mesurée ici.** Le banc
   ne vérifie à aucun moment le contenu d'une ligne, seulement qu'elle porte
   sur un fichier ouvert.

Compter la seule présence du nom récompense l'affirmation sans lecture : une
variante affichait 15/20 en ayant ouvert trois fichiers.

**L'instrument a été corrigé après ces mesures**, ce qui interdit de comparer
un chiffre d'avant à un chiffre d'après. Quatre défauts, tous relevés en
relecture :

* la lecture du banc coupait à 8 000 caractères, ignorait `offset`/`limit` et ne
  signalait pas la coupe, alors que le vrai outil rend `truncated`,
  `total_lines` et `next_offset` — un modèle qui respectait le contrat était
  pénalisé. Le banc appelle maintenant `read_file_payload`, le vrai code ;
* le score agrégeait la réponse ET tous les corps écrits. Il porte maintenant
  sur **une** source : la dernière version du livrable. Quand le livrable n'a
  jamais été écrit, le repli est **tout le texte émis pendant le run** — pas la
  réponse finale, que ce flux ne délimite pas. La colonne l'écrit ainsi, plutôt
  que de laisser croire à une réponse isolée ;
* l'unité était le radical du nom de fichier, donc `src/Admin/User.php` et
  `src/Public/User.php` n'en faisaient qu'une. C'est le chemin relatif complet,
  et la consigne demande désormais ce chemin en tête de ligne. La
  normalisation, elle, rendait deux clefs pour un même fichier selon qu'il
  était nommé en relatif ou en absolu : un chemin absolu était amputé de son
  `/` puis recollé à la racine ;
* **ouvrir un fichier n'est pas recevoir une valeur.** Le banc retenait le
  chemin dès que la lecture aboutissait, tranche vide comprise. Pour le
  scénario court, une preuve ne compte que si le texte réellement rendu
  contient la valeur ; pour l'inventaire, où le livrable nomme un chemin que
  le fichier ne contient jamais, l'ouverture reste la preuve ;
* le scénario court cherchait `8.2` et `7.3` dans un **chemin** de fichier,
  ce qu'aucun chemin ne contient. Les fichiers de preuve sont maintenant
  trouvés en cherchant la valeur dans le **contenu** du dépôt, et le banc
  annonce au démarrage combien de fichiers portent chaque fait.

Les deux scénarios sont aussi deux tests séparés : ils ne mesurent pas la même
chose et n'ont pas à partager un drapeau.

## Ce qui a été essayé, et ce que ça établit

### 1. Dire à l'agent comment tenir une tâche longue (3 variantes) — DÉGRADE

Consigne dans le prompt système : ouvrir une tâche, poser une définition de
fini, cocher au fur et à mesure, se relire en cas de doute. Variante 2 imposait
en plus l'ordre (« avant ton premier appel »).

| | couverture avec lecture | affirmation sans lecture |
|---|---|---|
| sans consigne | 13 | 0 |
| variante 1 | 10 | 21 |
| variante 2 | 9 | 14 |

**Rejeté.** La consigne déplace le modèle de « faire le travail » vers
« produire l'artefact du travail » : un plan, une DoD, un inventaire plausible.

Fait notable : **aucun** des six modèles locaux ni le modèle payant à
raisonnement (z-ai/glm-5.3) ne crée spontanément de tâche. Le payant réussit par
force brute — 250 000 tokens pour 4 faits — et sommé de tenir une DoD, il la
crée au 19ᵉ appel, après avoir tout fait, pour +68 % de tokens et 7,6× le temps.

### 2. Rendre à l'agent l'état que Kronn observe — DÉGRADE

Kronn exécute les appels, donc il sait quels fichiers ont été lus. Une ligne de
faits — jamais une instruction — collée à chaque résultat d'outil.

| | couverture avec lecture | affirmation sans lecture | fichiers lus |
|---|---|---|---|
| éteint | 10 et 0 | 0 et 0 | 9 et 6 |
| allumé | 2 et 1 | 7 et 7 | 2 et 1 |

**Rejeté, et c'est le résultat le plus contre-intuitif.** « Tu as lu X » est lu
comme un accomplissement, pas comme un état : le modèle conclut qu'il a fini et
se met à rédiger.

### 3. Pousser l'agent quand Kronn constate qu'il ne fait rien — NON ÉVALUÉ

Après trois tours sans un seul appel d'outil réussi, une ligne d'observation.

**Non évalué, et non rejeté.** Chiffres identiques au token près sur quatre
modèles : le seuil ne s'est jamais déclenché, parce que le mode d'échec visé —
l'abandon sans appel réussi — avait disparu avec les correctifs structurels
livrés le même jour. Le mécanisme n'a donc pas été jugé, seulement privé de sa
cible. Il faudrait un modèle ou un scénario qui cale encore pour en dire quoi
que ce soit.

### 4. Signaler un livrable qui décrit des fichiers jamais ouverts — NON ÉVALUÉ

**Hors cible, donc non jugé.** Le mécanisme surveille `write_file`, or ces
modèles fabriquent dans leur **réponse**, pas dans un fichier écrit. Il ne
s'est jamais déclenché, ce qui ne dit rien de sa valeur une fois braqué sur la
bonne surface.

Et son nom était faux : « fabrication détectée » affirme plus que ce que Kronn
observe. Le fait constatable est « aucune lecture de cette source observée dans
ce run » — une preuve peut venir d'un contexte initial, d'un diff, d'un tour
précédent ou d'une lecture partielle.

### 5. `repeat_penalty` et `num_keep` — DÉGRADE (3 modèles sur 4)

Jugé sur sa vraie cible, les tours perdus en appels identiques refusés :

| | tours perdus | couverture avec lecture |
|---|---|---|
| qwen3.5:2b | 24/36 → 3/8 | 1 → 0 |
| qwen3.5:4b | 8/29 → 5/27 | 0 → 8 |
| gemma4:e2b | 1/3 → 3/7 | 0 → 0 |
| gemma4:e4b | 8/27 → 13/32 | 10 → 9 |

**Rejeté** : échoue sur trois modèles fiables sur quatre. Le cas `qwen3.5:2b`
est un piège de lecture — les tours perdus s'effondrent parce qu'il abandonne
plus tôt, pas parce qu'il travaille mieux.

`num_keep` est **inerte** : la garde anti-troncature rend impossible le cas où
il servirait.

## Ce qui a marché, et le point commun

Aucun de ces correctifs ne dit quoi que ce soit au modèle. Ils retirent des
obstacles.

* l'élagueur raccourcit un résultat au lieu de le détruire, et cesse de dire
  « redemande » — ce qui provoquait la relecture qu'on lui reprochait ;
* la garde anti-répétition ne confisque plus `task_get` / `plan_get` après deux
  appels, puisque leur réponse change quand la tâche change ;
* la fenêtre est dimensionnée sur mesure et compte les outils déclarés ;
* `disc_read` sans identifiant relit la discussion courante — l'agent n'a
  jamais connu son propre id.

Effet cumulé : `qwen3.5:4b` passe de « abandonne en 8 tours sans ouvrir un
fichier » à « 29 tours, 6 fichiers lus ».

## Ce qui pourrait expliquer les affirmations sans lecture

**Hypothèse, pas résultat.** Aucune mesure ici ne fait varier la forme de la
tâche toutes choses égales par ailleurs, et aucune comparaison n'écarte la
consigne. Ce qui suit est la lecture la plus simple des traces, à vérifier avant
d'en tirer une décision.

« Une ligne par fichier » est satisfaisable à partir d'une liste de noms, sans
rien ouvrir. Trace d'un run : huit `list_files` avant dix `read_file`, puis cinq
lignes décrivant des fichiers **réels** jamais ouverts. Le cas extrême est
`gemma4:12b-mlx` : treize lignes pour zéro fichier lu.

Observé en revanche, sans hypothèse : le préambule anti-hallucination
(`core/anti_halluc.rs`, actif par défaut en `Warn`) dit « ne déclare aucun fait
non vérifié », il part à chaque run, et les affirmations sans lecture se
produisent quand même. Et les deux variantes en prose mesurées ici ont dégradé
la couverture au lieu de la relever — **sur cette tâche et ces modèles**.

## Pièges de méthode, payés comptant

* **Un run par cellule suffit** : le décodage glouton avec `seed` est
  reproductible au token près. Mais **aucune comparaison n'est valable entre
  deux builds** — changer une description d'outil déplace toutes les mesures.
  Toujours mesurer drapeau éteint puis allumé dans le même build.
* **Les modèles MLX ne sont pas reproductibles** entre deux runs : le slot est
  figé au chargement, donc le résultat dépend de ce qui était résident avant.
  Les quatre GGUF, si.
* **Compter la présence d'un nom récompense le mensonge.** Vérifier que le
  fichier a été lu.
* **Une promesse du prompt non tenue par le code casse les modèles.** Une
  description annonçait `tools_load` multi-familles avant que l'exécuteur ne le
  gère : deux modèles sont passés de `ok` à `MISSED`.

## Annexe — le texte exact qui a été injecté

Les correctifs des variantes n'ont pas été commités. Ce qu'elles envoyaient au
modèle, si : sans ça, « la consigne dégrade » n'est pas vérifiable.

**Variante 1 — la consigne de méthode** (`KRONN_AGENT_JOB_METHOD=1`), ajoutée
au prompt système :

```
=== A JOB THAT TAKES SEVERAL STEPS ===

Open a task with `task_create`, listing the steps in its `definition_of_done`.
Tick each one with `task_update_dod` the moment you finish it, and read the task
back with `task_get` whenever you are unsure what is left. Older tool results are
trimmed as the conversation grows, so progress left only in the conversation can
disappear; progress recorded on the task cannot. Do not re-read a file you have
already read: put what you found on the task instead.
```

La variante 2 ajoutait l'ordre d'exécution (« avant ton premier appel »).

**Variante « état observé »** — un champ `kronn_progress` inséré dans chaque
résultat d'outil, plafonné à 300 octets, alimenté par les seuls `read_file`
aboutis :

```
[Kronn] read so far: src/Kernel.php, src/Controller/HomeController.php (2 files)
[Kronn] read so far: 40 files, most recent: …           (au-delà du plafond)
[Kronn] read so far: 1 file                              (dernier recours)
```

Détail qui a failli fausser la mesure : la première version collait ce texte
**après** le JSON du résultat, ce qui le rendait non analysable et faisait
retomber l'élagueur sur une coupe aveugle — le défaut même qu'un autre
correctif de la journée venait de réparer. D'où le champ plutôt que le suffixe.

## Reproduire

```
KRONN_OLLAMA_BENCH=1 KRONN_OLLAMA_NUM_CTX_CAP=24576 \
KRONN_BENCH_PROJECT=<un dépôt> \
KRONN_BENCH_MODELS="qwen3.5:2b,qwen3.5:4b,gemma4:e2b,gemma4:e4b" \
cargo test --lib bench_inventory_of_a_repository -- --ignored --nocapture
```

Le contrôle court est `bench_four_facts_from_a_repository`, mêmes variables,
plus `KRONN_BENCH_FACTS="libellé=valeur,..."` pour l'adapter à un autre dépôt.

**Correction (2026-09-22):** the catalogue benchmarks below use `BenchTools`
or `WorkTools`, which implement family loading themselves. They did not test
the production dispatcher, where `tools_load` was unreachable. Their earlier
6/6 and 25% saving claims cannot establish the shipped mechanism's benefit.
`bench_quick_prompt_native_catalogue` now delegates to `KronnToolExecutor`
against an ephemeral database; its report distinguishes actual queued work
from completed child inference. [src: file: backend/src/api/agent_quick_prompt_bench.rs:1]

Les autres bancs : `bench_tiered_catalogue_across_models` (catalogue par tiers),
`bench_project_doc_inline_against_pointer` (doc projet),
`bench_two_families_in_one_run` (tâche composée). Tous `--ignored`, tous pilotés
par `KRONN_BENCH_MODELS`, tous capables de viser un fournisseur payant via
`KRONN_BENCH_HTTP_ENDPOINT` / `KRONN_BENCH_HTTP_KEY`.
