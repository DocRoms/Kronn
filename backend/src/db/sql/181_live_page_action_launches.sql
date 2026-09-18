-- KT-678 — un bloc d'action de Live Page est un GABARIT, pas une proposition.
--
-- `160_live_page_actions.sql` a repris `159_discussion_actions.sql` ligne pour
-- ligne, et avec lui l'hypothèse qui le tient : une déclaration est lancée une
-- fois. C'est vrai d'une discussion — une fence est postée une fois, dans un
-- message, et porte une intention — et faux d'une page, où un bloc d'action est
-- instancié autant de fois qu'il y a de lignes dans le dataset et re-rendu à
-- chaque republication. Une page listant 39 tickets dessinait 39 boutons sur
-- une seule ligne : le premier clic consommait l'offre pour tous, chaque clic
-- suivant renvoyait le premier lancement, et un bouton annonçait un ticket
-- traité alors que rien n'avait tourné.
--
-- L'offre et l'acte sont séparés ici. `live_page_actions` garde l'offre et perd
-- toute colonne d'exécution ; un lancement devient une ligne à lui, identifiée
-- par la liaison sur laquelle on a cliqué.

-- Les lancements déjà enregistrés sur les lignes de déclaration sont la seule
-- trace de ce qui a réellement tourné : ils sont mis de côté avant la
-- reconstruction, qui les effacerait.
CREATE TABLE live_page_action_launch_carryover AS
SELECT id, live_page_revision_id, kind, target_id, target_name, project_id,
       state, values_json, shared_run_id, result_discussion_id, deep_link,
       diagnostic, launched_at, finished_at, updated_at
FROM live_page_actions
WHERE launched_at IS NOT NULL;

-- SQLite ne sait pas retirer des colonnes ni restreindre un CHECK en place.
CREATE TABLE live_page_actions_next (
    id TEXT PRIMARY KEY,
    live_page_id TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    live_page_revision_id TEXT NOT NULL REFERENCES live_page_revisions(id) ON DELETE CASCADE,
    action_ref TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('quick_prompt','quick_api','quick_exec','workflow','invalid')),
    target_id TEXT NOT NULL,
    target_name TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    -- Santé de la déclaration, et rien d'autre : `proposed` = le bloc est
    -- exploitable, `preflight_failed` = Kronn n'a pas su le lire. Qu'une chose
    -- ait tourné est une propriété d'un lancement, jamais de l'offre.
    state TEXT NOT NULL DEFAULT 'proposed' CHECK(state IN ('proposed','preflight_failed')),
    values_json TEXT NOT NULL DEFAULT '[]',
    diagnostic TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(live_page_id, action_ref)
);
INSERT INTO live_page_actions_next
SELECT id, live_page_id, live_page_revision_id, action_ref, kind, target_id,
       target_name, project_id,
       -- Une ligne jamais lancée garde son verdict d'ingestion ; une ligne
       -- lancée redevient l'offre qu'elle n'aurait jamais dû cesser d'être,
       -- son exécution partant dans la table des lancements.
       CASE WHEN launched_at IS NULL AND state = 'preflight_failed'
            THEN 'preflight_failed' ELSE 'proposed' END,
       values_json,
       CASE WHEN launched_at IS NULL THEN diagnostic END,
       created_at, updated_at
FROM live_page_actions;
DROP TABLE live_page_actions;
ALTER TABLE live_page_actions_next RENAME TO live_page_actions;
CREATE INDEX idx_live_page_actions_page ON live_page_actions(live_page_id, created_at, id);

CREATE TABLE live_page_action_launches (
    id TEXT PRIMARY KEY,
    action_id TEXT NOT NULL REFERENCES live_page_actions(id) ON DELETE CASCADE,
    -- La ligne sur laquelle le clic a porté : les paires `nom=sélecteur`
    -- triées par nom. Vide pour un CTA sans liaison dynamique — ce qui rend
    -- un tel CTA relançable, puisque le garde ci-dessous ne refuse que ce qui
    -- tourne encore.
    binding_key TEXT NOT NULL,
    -- Ce contre quoi le lancement a tourné, figé au clic. La déclaration suit
    -- la page à chaque republication ; un lancement est de l'historique, et
    -- une cible changée plus tard ne doit pas réécrire ce qui a été exécuté.
    live_page_revision_id TEXT NOT NULL REFERENCES live_page_revisions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('quick_prompt','quick_api','quick_exec','workflow','invalid')),
    target_id TEXT NOT NULL,
    target_name TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    state TEXT NOT NULL DEFAULT 'proposed' CHECK(state IN ('proposed','launching','running','succeeded','failed','cancelled','preflight_failed')),
    values_json TEXT NOT NULL DEFAULT '[]',
    shared_run_id TEXT REFERENCES shared_runs(id) ON DELETE SET NULL,
    result_discussion_id TEXT REFERENCES discussions(id) ON DELETE SET NULL,
    deep_link TEXT,
    diagnostic TEXT,
    launched_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Le garde d'idempotence, rescopé. « Ne relance pas ce qui tourne déjà »
-- appliqué à toute la liste tuait 38 boutons ; appliqué à la liaison, il garde
-- exactement sa propriété de sûreté et cesse de nuire.
CREATE UNIQUE INDEX idx_live_page_action_launches_inflight
    ON live_page_action_launches(action_id, binding_key)
    WHERE state IN ('launching','running');
CREATE INDEX idx_live_page_action_launches_action
    ON live_page_action_launches(action_id, created_at, id);

INSERT INTO live_page_action_launches (
    id, action_id, binding_key, live_page_revision_id, kind, target_id,
    target_name, project_id, state, values_json, shared_run_id,
    result_discussion_id, deep_link, diagnostic, launched_at, finished_at,
    created_at, updated_at
)
SELECT 'page-launch:' || id, id, '', live_page_revision_id, kind, target_id,
       target_name, project_id, state, values_json, shared_run_id,
       result_discussion_id, deep_link, diagnostic, launched_at, finished_at,
       launched_at, updated_at
FROM live_page_action_launch_carryover;
DROP TABLE live_page_action_launch_carryover;
