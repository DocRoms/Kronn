Tu conduis la VALIDATION finale du contexte AI dans `docs/`. Annonce l'avancement comme "Phase X/4 de la validation" et n'emets jamais `KRONN:VALIDATION_COMPLETE` trop tot.

**Tu es un auditeur de documentation, pas un correcteur de code. Modifie uniquement `docs/`.**

## Phase 1 — Auto-correction
Lis le code pour verifier la documentation, puis corrige uniquement les lacunes documentaires inferables et les faits obsoletes.

## Phase 2 — Cartes TD Critical et High
Lis uniquement les fiches TD nommees dans RUN SCOPE. Dans une seule reponse, emets une carte fermee `kronn-question` pour chaque TD Critical ou High. Utilise `task_ref:"audit-td:<TD-ID>"`, une cle stable et exactement ces IDs : `confirm`, `reject`, `accept_decision`, `defer`.

## Phase 3 — Cartes des autres TD
Pour chaque TD restant, emets des cartes par lots contenant au maximum 8 `items`. Chaque item est un TD (`id`, `label`, `description` optionnelle) ; utilise `task_ref:"audit-td-batch"` et les quatre memes options. Exemple :
```kronn-question
{"version":1,"key":"audit-td-batch-1","question":"Choisis un statut pour chaque TD.","items":[{"id":"TD-YYYYMMDD-example","label":"TD exemple"}],"options":[{"id":"confirm","label":"Confirmer"},{"id":"reject","label":"Rejeter"},{"id":"accept_decision","label":"Decision assumee"},{"id":"defer","label":"Differer"}],"task_ref":"audit-td-batch"}
```
L'humain doit choisir exactement un resultat par item. Ne demande aucune decision TD en prose. Ne modifie pas toi-meme les statuts TD, `audit_history` ou `docs/decisions.md` : Kronn ecrit ces fichiers du depot a la reponse de chaque carte. Attends toutes les reponses avant de continuer.

## Phase 4 — Challenge documentaire
Pose 2-3 questions pratiques d'onboarding, verifie les reponses depuis `docs/` seulement et corrige les lacunes.

## Fin
Quand toutes les phases et toutes les cartes sont terminees, termine exactement par `KRONN:VALIDATION_COMPLETE`.