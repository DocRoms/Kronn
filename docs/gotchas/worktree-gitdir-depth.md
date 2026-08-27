# Worktree gitdir : profondeur relative et back-reference (KT-331)

`fix_worktree_paths` (`backend/src/core/worktree.rs`) réécrit les deux références
croisées d'un worktree git en **relatif**, seule forme portable entre la vue
conteneur (`/host-home/...`) et la vue hôte. Un worktree Kronn vit à
`<repo>/.kronn/worktrees/<name>` (3 niveaux sous le dépôt).

## Les deux défauts corrigés

1. **Forward `<worktree>/.git`** — écrivait `gitdir: ../../.git/worktrees/<name>`
   (2 niveaux). `../../` remonte à `<repo>/.kronn`, pas `<repo>` → **toute** commande
   git lancée *à l'intérieur* du worktree échoue (`not a git repository`). Correct =
   `../../../.git/worktrees/<name>` (3 niveaux, calculé depuis le chemin réel).

2. **Back-reference `<repo>/.git/worktrees/<name>/gitdir`** — écrivait
   `.kronn/worktrees/<name>/.git`. Git résout ce fichier **relativement à son propre
   dossier** (`<repo>/.git/worktrees/<name>/`), donc il pointait vers
   `<repo>/.git/worktrees/<name>/.kronn/worktrees/<name>` (inexistant) → `git worktree
   list` marque la worktree **`prunable`**, et `git worktree prune` peut supprimer
   l'entrée admin. Correct = `../../../.kronn/worktrees/<name>/.git` (remonte 3 niveaux
   jusqu'à la racine, puis redescend).

## Pourquoi c'était latent

Aucun test n'exécutait de vraie commande git *dans* le worktree : ils vérifiaient
l'existence du fichier `.git` ou une **sous-chaîne** de son contenu — et
`../../../.git/worktrees/` contient `../../.git/worktrees/` comme sous-chaîne, donc
l'assertion passait pour les deux formes. Côté runtime, le backend lance git avec
`current_dir(repo_path)` (le dépôt principal), jamais depuis l'intérieur du worktree.
Le bug ne mordait donc que quand un worker exécute git dans le checkout — exactement
le flux worker CLI de KT-328.

## Impact sur les worktrees de discussion existants

`create_discussion_worktree` appelle `fix_worktree_paths` sur le chemin de prod
(`backend/src/api/discussions/crud.rs`) : les worktrees de discussion créés **avant**
ce correctif portent l'ancien gitdir (2 niveaux + back-ref prunable). Le correctif ne
répare que les **nouveaux** worktrees ; le chemin de réutilisation
(`create_discussion_worktree`, branche déjà checkout) retourne tôt sans re-réécrire.
La réparation des worktrees existants est **hors périmètre de KT-331** (arbitrage
Romu) : elle relève d'un balayage au boot (territoire KT-322) ou d'une re-création.

## Adoption par les worktrees de tâche (KT-318)

`create_task_worktree` s'appuyait sur le gitdir **absolu natif** (contournement tant
que `fix_worktree_paths` était buggé) : correct pour un worker qui partage la vue FS
du créateur, mais inutilisable par un worker CLI hôte quand le backend tourne en
Docker (chemin conteneur). Depuis KT-331 il appelle `fix_worktree_paths` corrigé
(forme relative portable) — le prérequis Git du handshake CLI (KT-328).
