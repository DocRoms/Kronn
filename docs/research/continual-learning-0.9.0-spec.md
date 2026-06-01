# Kronn 0.9.0 — Continual Learning : spec d'implémentation

> **Statut** : spec de consolidation (2026-05-31). Fusionne les décisions figées
> (REDESIGN CANONIQUE 2026-05-27, convergence 5/5 + PR4a drafté 2026-05-28) en un
> contrat implémentable, et **explicite le gate de fidélité niveau-2** — le seul
> verrou anti-hallu qui manque réellement pour rendre 0.9.0 sûr.
>
> Précondition livrée : PR1-PR3 (section `docs/AGENTS.md § anti-hallu` source
> unique + STEP 0 audit + PREAMBLE pointer + endpoints inject/sync + badge UI).
>
> **MàJ 2026-06-01 — toggle + doc-wiring (décidé avec Romuald).** Voir §15.

---

## 0. Toggle maître + doc-wiring conditionnel (décisions 2026-06-01)

Deux toggles distincts, à ne pas confondre :
- **`continual_learning_enabled`** (bool, **défaut `false` = OFF/beta au ship**) — master ON/OFF de la feature (capture/validate/promote). Pilote : le tool MCP/API `propose`, la section doc, le badge/modal UI. **Défaut OFF** car la feature écrit dans des fichiers-vérité injectés : on évite que des users abîment leur doc si un bug subsiste. On allume sur les projets perso d'abord, puis bascule ON.
- **`faithfulness_backend`** (off/nli/llm) — qualité du Gate-2, *interne* à la feature. Indépendant.

**Doc-wiring du scope PROJET = section marker-délimitée, pas un seed statique.** Vérifié (code) : Kronn n'auto-injecte PAS l'arbre `docs/` (seul `~/.kronn/user-context/*.md` est auto-lu par `runner.rs` → **le scope USER marche déjà tout seul**). Le scope PROJET (`docs/learnings.md`) n'est lu que si `docs/AGENTS.md` le pointe. Donc :
- **ON** → une section `<!-- kronn:section name="learnings" curated="ai" -->` est injectée dans `docs/AGENTS.md` = **un POINTEUR** vers `docs/learnings.md` (les learnings vivent dans le fichier dédié → zéro mutation du contenu audité → pas de drift-checksum). Injectée par l'audit (STEP, comme anti-hallu) + endpoint inject/sync. `docs/learnings.md` seedé (bloc vide).
- **OFF** → la section est **retirée** (par marker, idempotent). `docs/learnings.md` **conservé tel quel** (on ne détruit jamais les learnings déjà validés — juste plus pointé).

**Réutilise l'infra anti-hallu** : `anti_hallu_step.rs` (`find_marker_line`/`refresh_existing`/`insert_new`) + `anti_hallu_inject.rs` (inject/sync endpoints) → on mirror pour `name="learnings"` + on ajoute le chemin **remove** (cas OFF). « Faisable simplement » confirmé.

**Séquence de ship 0.9.0** : (1) toggle `continual_learning_enabled` (défaut OFF) → (2) PR4c section marker `learnings` (inject ON / remove OFF + seed) → (3) mount A+C (badge global + intercept archive manuel), gatés par le toggle → (4) PR4a-bis LLM-judge (optionnel, Gate-2 défaut OFF). Le toggle gate la CAPTURE (`propose`) ; valider/rejeter les pending existants reste possible même OFF (on draine, on ne capture plus).

---

## 1. Objectif & pourquoi l'anti-hallu est la précondition

Kronn capture aujourd'hui la vérité statique (audit code) + la config
(skills/profils/directives/MCP). Il rate le **savoir tacite** des conversations
(préférences, conventions, décisions, pièges). Continual Learning capture ça :
un agent émet une *learning* candidate pendant une discussion → validation
humaine → persistée dans `docs/AGENTS.md` (scope projet) ou
`~/.kronn/user-context/learnings.md` (scope user).

**Le risque qui dicte toute l'architecture** : ces deux fichiers sont injectés
par Kronn comme contexte dans *tout* prompt agent futur. Une learning hallucinée
qui s'y persiste devient de la **training-data permanente** pour tous les agents
en aval (stage 3 — le pire, le pushback humain en disc disparaît). Donc :

> **Aucune écriture dans un fichier-vérité sans (a) evidence qui résout
> mécaniquement ET (b) un humain qui valide.** 0.9.0 sans le loop anti-hallu
> *amplifie* les hallucinations au lieu de capturer du savoir.

---

## 2. Les DEUX gates de vérification (le cœur de cette spec)

Une learning a la forme `claim` + `evidence[]`. La vérifier, c'est répondre à
**deux** questions distinctes — et c'est là que le design figé était incomplet :

| Gate | Question | Couche | État |
|---|---|---|---|
| **Gate 1 — existence** | l'evidence citée *existe-t-elle* ? (`file:line` résout, in bounds ; URL ; user-confirmé daté) | niveau-1 (`verify_source_marker`) | **livré 0.8.7** |
| **Gate 2 — fidélité** | le `claim` *découle-t-il* de l'evidence ? (`claim ⊨ evidence`) | niveau-2 (RFC-6) | **manquant — l'objet de 0.9.0** |

**Pourquoi Gate 1 ne suffit pas** (prouvé par la repasse 4-personas 2026-05-31) :
l'existence-lint vérifie que le fichier cité existe, *jamais* que le claim
correspond à son contenu. La seule vraie hallucination du corpus — « le CSS
`nth-of-type(even)` déjà en place alterne le fond » alors que `grep` = vide —
résout en vert au niveau-1 (le `.css` existe) tout en étant **fausse**. Une
learning, c'est *exactement* ce cas : `claim` sémantique + `evidence` qui existe.
Gate 1 seul laisserait passer la paraphrase-training-data plausible.

### Posture du Gate 2 — FIGÉE : (B) score informatif (validé 2026-05-31)

Le niveau-2 = un checker scorant `claim ⊨ evidence` → {entailment, neutral,
contradiction}. **Posture retenue : (B)** — le checker produit un verdict par
paire (claim ↔ chaque evidence), stocké en `faithfulness` et affiché en chip
dans le modal humain (🟢 entailment / 🟡 neutral / 🔴 contradiction). **Le gate
reste l'humain**, mais informé au lieu d'aveugle ; un 🔴 force un clic de
confirmation explicite. Ne parie pas 0.9.0 sur la précision du checker, respecte
la désagentification, et peut être promu en bloquant en mode `enforce` plus tard.

Postures écartées (trace) : **(A) bloquant auto** — parie la release sur la
précision du checker, faux-blocages silencieux ; **(C) différer 0.9.1** — laisse
le trou prouvé (`nth-of-type` : Gate 1 vert mais claim faux) ouvert dans le modal.

**Précondition** : le **proto jetable** (PR4-0) sur ~30 paires réelles tranche si
le NLI local discrimine assez (sinon `LlmJudgeChecker`). Premier livrable code.

### Contrat `FaithfulnessChecker` — FIGÉ (décision #2, validé 2026-05-31)

Backends interchangeables derrière un trait, choisis par config
(`faithfulness_backend = "nli" | "llm" | "off"`). Le proto valide `NliChecker`
derrière la MÊME interface que la prod ; bascule sans toucher le pipeline.

```rust
pub enum Faithfulness { Entailment, Neutral, Contradiction }

pub struct FaithfulnessVerdict {
    pub verdict: Faithfulness,
    pub score: f32,            // 0.0–1.0
    pub checker: &'static str, // "nli-local" | "llm-judge" — traçabilité
}

#[async_trait]
pub trait FaithfulnessChecker: Send + Sync {
    async fn check(&self, claim: &str, evidence_quote: &str) -> FaithfulnessVerdict;
}

struct NliChecker { /* ONNX local (ort/candle), zéro token */ }
struct LlmJudgeChecker { /* Haiku/Ollama via agents::runner (reçoit le PREAMBLE) */ }
// "off" → le pipeline saute l'étape 4, faithfulness=NULL, modal sans chip Gate 2.
```

Le pipeline (§6 étape 4) appelle `checker.check(...)` sans connaître l'impl.

**Défaut recommandé — VALIDÉ PAR PROTO (2026-05-31, voir `nli-proto-findings.md`).**
Un proto sur 255 paires réelles (mining 32 conversations + 82 adversariaux subtils
vérifiés + cas-or, labels 3-juges à 84% d'unanimité) tranche :
- **`off` par défaut** au ship (le signal n'est pas assez fiable pour être actif sans tuning).
- **`llm` (LLM-judge) = checker de qualité** quand activé. Les juges LLM ont
  produit les labels à 84% d'unanimité (la barre) ; le NLI local plafonne à acc
  0.34–0.42. Pour de la *fidélité sémantique*, le LLM-judge est nettement supérieur.
- **`nli` = pré-filtre cheap / indice de confiance optionnel**, JAMAIS l'autorité.
  Signal réel uniquement **aux extrêmes** : un `ent_p` très bas (<~0.1) face à une
  source résolue = fort « à revérifier » (mDeBERTa a mis le `nth-of-type` halluciné
  à 0.004 vs 0.972 pour un vrai match). Le milieu flou reste silencieux (sinon
  spam d'ambre sur les claims descriptifs légitimes — entail-kept-green ≈ 0.15).
  Latence CPU ~4s/paire → async/opt-in, jamais par-message temps-réel sans GPU.

**Le proto confirme aussi la posture B** : un auto-gate (posture A) serait à la fois
bruyant (faux-flag des claims lâches valides) ET incomplet → écarté sur données.

---

## 3. Modèle de données — migration `063_continual_learning.sql`

```sql
CREATE TABLE learnings (
  id                TEXT PRIMARY KEY,
  claim             TEXT NOT NULL,
  evidence_json     TEXT NOT NULL,   -- JSON array, MUST be non-empty
  kind              TEXT NOT NULL,   -- 'fact'|'preference'|'inference'
  status            TEXT NOT NULL DEFAULT 'pending', -- pending|validated|rejected|stale|promoted
  scope             TEXT,            -- 'user'|'project' (NULL until routed)
  confidence        REAL,            -- self-scored, haircut applied server-side
  faithfulness      TEXT,            -- niveau-2 verdict: 'entailment'|'neutral'|'contradiction'|NULL
  discussion_id     TEXT,
  project_id        TEXT,
  source_agent      TEXT,
  promoted_target   TEXT,            -- file path the learning was written to
  created_at        DATETIME NOT NULL,
  last_validated_at DATETIME,
  validated_by      TEXT,            -- 'human' | 'rule:<name>'
  FOREIGN KEY (discussion_id) REFERENCES discussions(id) ON DELETE SET NULL,
  FOREIGN KEY (project_id)    REFERENCES projects(id)    ON DELETE SET NULL
);
CREATE INDEX idx_learnings_status ON learnings(status);
CREATE INDEX idx_learnings_disc   ON learnings(discussion_id);
CREATE INDEX idx_learnings_stale  ON learnings(status, last_validated_at);
-- Dedup: same claim, same scope, same kind → one row.
CREATE UNIQUE INDEX idx_learnings_dedup ON learnings(kind, COALESCE(scope,''), claim);

-- Negative learning (safeguard #6a): 3 rejets → auto-reject le 4e.
CREATE TABLE learning_rejections (
  claim_hash TEXT PRIMARY KEY,
  reason     TEXT,
  count      INTEGER NOT NULL DEFAULT 1,
  last_at    DATETIME NOT NULL
);
```

Register à `migrations.rs` (062 = lint_report 0.8.7, donc 063 libre). Nouveau
champ vs le draft mémoire : **`faithfulness`** (verdict niveau-2) +
`learning_rejections`.

---

## 4. Le tool MCP typé `learning_propose` (D8 — fence libre INTERDITE)

Émission via **tool MCP typé uniquement**, jamais une fence `kronn-learning`
libre (l'agent ne peut pas produire un payload sans `evidence[]`). Bridge Python
`disc-introspection-mcp.py` :

```python
{ "name": "learning_propose",
  "inputSchema": {"type": "object", "required": ["claim", "evidence", "kind"],
    "properties": {
      "claim": {"type": "string"},
      "evidence": {"type": "array", "minItems": 1,
        "items": {"type": "object", "required": ["kind", "ref"],
          "properties": {
            "kind":  {"type": "string", "enum": ["file", "url", "disc", "cmd", "user"]},
            "ref":   {"type": "string", "description": "file:line | url | disc-id | cmd | user:date"},
            "quote": {"type": "string", "description": "extrait support (pour le niveau-2)"}}}},
      "kind": {"type": "string", "enum": ["fact", "preference", "inference"]}}}}
```

Dispatcher : garde client-side (`evidence` vide → erreur immédiate), hérite
`project_id`/`source_agent`/`discussion_id` du contexte disc, POST
`/api/learnings/propose`. Le handler Rust **rejette** : `evidence` vide, `claim`
blanc, élément d'evidence blanc, `content` qui matche `core::redact` (secret).

**L'extracteur passe par `agents/runner.rs`** (donc reçoit `preamble_if_active()`
— la discipline cascade). Pin par test (invariant dur #3). Pas de raccourci
direct-call.

---

## 5. Binding type → SourceKind (méta-garde-fou, invariant dur #2)

| `kind` | Définition | SourceKind exigé | Graduation |
|---|---|---|---|
| **fact** | vérifiable mécaniquement | **≥1 evidence `file`/code `Verified`** (URL = `Unchecked` en 0.9.0 — pas de vérif réseau ; ne compte donc pas comme Verified) | OK après 1 validation humaine |
| **preference** | déclaration user explicite | `User` (**date requise**) | OK après 1 validation |
| **inference** | dérivée sans déclaration explicite | `Inferred` (Unchecked) | **0.9.0 : 1 validation humaine** + warning modal ; double-validation 2 sessions = **futur 0.9.x** |

`SourceKind::TrainingData` ⇒ **rejet auto à l'extraction**. **Enforcé backend au
`validate` (2026-06-01)** : `fact` exige ≥1 evidence `Verified` ; `preference`
exige ≥1 evidence `user` **datée** (`[src: user:YYYY-MM-DD]`). `inference` :
promue sur **une** validation humaine en 0.9.0 (le gate humain EST la validation,
posture B) avec warning ; la **double-validation 2 sessions est un safeguard
futur (0.9.x)**, PAS promis en 0.9.0 — on ne sur-vend pas l'invariant (90% des
hallucinations comportementales sont des inférences, donc le warning reste).

---

## 6. Pipeline de validation (write-path, ordre exact)

```
learning_propose(claim, evidence[], kind)
      │
      ├─ 0. redact-secret guard (core::redact) ............ reject si match
      ├─ 1. schema + evidence non-vide + kind∈enum ........ reject sinon
      ├─ 2. GATE 1 existence — verify_source_marker(ev) ... par evidence → SourceCheck
      │        any red (NotFound/OutOfBounds/Rejected) → reject candidate
      ├─ 3. binding kind→SourceKind (§5) .................. inference/training-data règles
      ├─ 4. GATE 2 fidélité — niveau-2 claim ⊨ evidence ... verdict stocké (posture B: informatif)
      ├─ 5. anti-généralisation (safeguard #2) ............ "always/never/toujours" sans scope → reformuler
      ├─ 6. contradiction-check (safeguard #3) ............ cosine sim vs learnings existantes → flag
      ├─ 7. confidence haircut (safeguard #5) ............. confidence_real = self * 0.85
      ├─ 8. negative-learning (safeguard #6a) ............. claim_hash rejeté ≥3× → auto-reject
      └─ INSERT status='pending' (rien n'est écrit en fichier ici)
                                  │
        ── modal humain (PR4b) ──┤  affiche: type chip, confidence, evidence[] (kind+ref+quote),
                                  │           GATE 2 verdict chip si présent
        [Valider] → POST /validate → guard status=pending → re-vérif Gate-1 (code courant) →
                                  │   fact⇒Verified / preference⇒user-daté → scope router (§7) →
                                  │   render entry → re-lint (fabricated>0 ⇒ REFUS, avant écriture) →
                                  │   atomic write → status='promoted'
        [Rejeter] → learning_rejections++ (scope recalculé)
```

**Invariant dur #4 (réel)** : la promotion **rend l'entry, la lint (`analyze_roots`),
et REFUSE l'écriture si `fabricated_count > 0`** (render→lint→atomic-write, donc pas
de rollback fichier). Les entries sont lintables isolément via
les marqueurs `<!-- kronn-learning-block:start/end -->`.

**Note niveau-0** : `strip_fenced_code()` strippe les fences avant lint — mais
ici l'émission est un **tool call typé**, pas une fence dans la prose, donc le
conflit #2 du design figé (payload invisible au lint) disparaît *by construction*
avec le passage au tool MCP. Le `claim + quote` est ré-injecté dans `analyze()` à
l'étape 2/4, pas lu depuis la prose.

---

## 7. Scope router (`core/learning_scope.rs`, pur)

- `preference` → `User` (écrit `~/.kronn/user-context/learnings.md`)
- `fact`/`inference` avec `project_id` → `Project` (écrit `docs/learnings.md`)
- sinon → `User`

**Promotion vers un fichier DÉDIÉ** (`learnings.md`), jamais en mutant les
fichiers audités (`docs/AGENTS.md` curated) — sinon on invalide les checksums de
drift. Re-render par marqueur-bloc idempotent, `(lc_id:N)` pour update/revert.
`scope=global/user` via une `preference` exige `[src: user: YYYY-MM-DD]`
(enforcé backend au `validate`, 2026-06-01). La double-validation 2 sessions
est un safeguard **futur (0.9.x)**, pas un invariant 0.9.0.

---

## 8. Cron staleness (`core/learning_sweep.rs`, D9)

`tokio::spawn` + `interval(3600s)`, spawn depuis `main.rs` (à côté de
`WorkflowEngine`). Chaque tick : marque `stale` les lignes où
`COALESCE(last_validated_at, created_at) < now - 7j` ET `status IN
('pending','validated')`. **Pas de suppression auto** — nudge UI au prochain
chargement. (Event-driven rejeté au R2 : pas de FileWatcher Tauri ; le hook
`streaming.rs` re-lint au resurfacing suffit.)

---

## 9. Garde-fous (mapping safeguard → étape pipeline)

| # | Garde-fou | Où |
|---|---|---|
| 1 | evidence obligatoire | étape 1 (schema) + Gate 1 |
| 2 | anti-généralisation (`always/never` sans scope) | étape 5 |
| 3 | contradiction silencieuse (cosine sim, pas de vector DB) | étape 6 → modal |
| 4 | staleness `last_validated_at` | cron §8 + nudge |
| 5 | sycophantie → confidence + haircut 0.85 | étape 7 |
| 6 | reinforcement → negative learning + skill `memory-auditor` (on-demand, 0.9.x) | étape 8 |

---

## 10. API routes

```
POST /api/learnings/propose          — MCP-facing (validation pipeline §6)
GET  /api/learnings?status=&project_id=
GET  /api/learnings/pending          — { count } pour le badge global
POST /api/learnings/{id}/validate    — guard pending + re-vérif Gate-1 + binding + scope router + promotion (render→lint→write)
POST /api/learnings/{id}/reject      — learning_rejections++ (scope recalculé)
GET  /api/discussions/{id}/learnings — pending d'une disc (pour le futur intercept archive)
POST /api/projects/{id}/learnings/sync — PR4c : inject/remove la section pointeur selon le toggle
```

---

## 11. UI (PR4b) — état réel au 2026-06-01

**Livré** :
- **Toggle Settings** (`ContinualLearningSection`) : master ON/OFF (beta), optimiste + revert.
- **Badge global** (`ChatHeader` → `LearningsBadge`) : count pending toutes discs,
  **par polling** (60s ; caché si 0). Ouvre le modal au clic.
- **Modal de validation** (`LearningsModal`) : par learning → type chip (fact vert /
  preference bleu / inference orange), confidence %, **liste evidence** (kind +
  `ref` + quote), **chip verdict Gate-2 si présent** (entailment/neutral/contradiction,
  informatif). Boutons : **Valider / Rejeter**.

**PAS encore implémenté (futur / optionnel, à ne pas présenter comme fait)** :
- SSE `LearningCandidateDetected` (le badge **poll**, pas de push temps-réel).
- **Pill SourceCheck par evidence** (Verified/Unchecked/NotFound) : le `Learning`
  stocké ne porte pas les verdicts Gate-1 par ligne (calculés au propose, non
  persistés) → le modal montre l'evidence brute, pas un statut par ligne. À
  ajouter il faudrait persister les checks ou re-vérifier à l'ouverture du modal.
- Boutons **Modifier scope / Reformuler**.
- **Intercept archive manuel** → ouvrir le modal des pending de la disc (option C).
  Aujourd'hui le badge global est la seule surface d'accès.

---

## 12. Invariants durs (à piner par test, non négociables)

1. Aucune écriture fichier-vérité sans `[src:]` qui résout (Gate 1 vert).
2. `TrainingData`/`Inferred` → pas d'extraction auto en vérité ; binding type→SourceKind.
3. Extracteur/évaluateur passent par `agents/runner.rs` (reçoivent le PREAMBLE).
4. Post-render re-lint `analyze_roots()` AVANT écriture ; `fabricated_count>0` ⇒ refus (render→lint→write, pas de rollback fichier). ✅ enforcé.
5. `fact` → ≥1 evidence `Verified` ; `preference` → ≥1 evidence `user` datée. ✅ enforcé au `validate`. `inference` → 1 validation humaine en 0.9.0 (double-validation = futur 0.9.x, non promis).
6. `validate` refuse tout statut ≠ `pending`. ✅ enforcé.

---

## 13. Décisions — TOUTES TRANCHÉES (2026-05-31)

1. **Posture Gate 2** : **(B) informatif dans le modal** ✅ (§2). L'humain reste
   le gate, 🔴 = clic de confirmation explicite. Promotion bloquante possible en
   `enforce` plus tard.
2. **Modèle niveau-2** : **trait `FaithfulnessChecker`** (NLI local / LLM / OFF
   via config) ✅ (§2). Le **proto PR4-0** tranche NLI-local vs LLM-judge sur ~30
   paires réelles (dont `nth-of-type`). NLI peut être confiné au NL↔NL (la
   learning `quote` est du NL, ça tient) si le code le met en échec.
3. **Baseline staleness** : **`COALESCE(last_validated_at, created_at)`** ✅ — un
   pending non-revu depuis 7j mérite le nudge.
4. **Coexistence fence** : **non — tool MCP `learning_propose` seul** ✅ (D8, fence
   libre INTERDITE dès 0.9.0).
5. **Spawn cron** : la feature est dans la lib `kronn::` → les 2 binaires l'ont
   gratuitement, MAIS le **spawn `LearningSweep::start()` doit être ajouté dans
   les DEUX `main.rs`** ✅ — `backend/src/main.rs` (≈:317) ET
   `desktop/src-tauri/src/main.rs` (≈:546), en miroir du spawn `WorkflowEngine`.
   Sinon le cron ne tourne pas en desktop (learnings jamais stale).

---

## 14. Séquençage — STATUT 2026-06-01

> **Livré (branche `feat/continual-learning-start`)** : PR4-0 proto ✅ · PR4a backend ✅ ·
> toggle maître ✅ · PR4c doc-wiring ✅ · PR4b UI (toggle + badge + modal) ✅ ·
> hardening review Codex 5 rounds ✅. **Non livré (optionnel, non bloquant)** :
> PR4a-bis (brancher LLM-judge) · intercept archive manuel (option C) · badge SSE.
> ~65 tests, suite lib 2969 verte, clippy clean. Toggle **OFF par défaut**.


- **PR4-0 (proto, ~0.5j)** : Python jetable NLI sur ~30 paires → tranche décisions 1+2.
- **PR4a (~4j)** : migration 063 + `models/learnings.rs` + `db/learnings.rs` +
  `api/learnings.rs` (pipeline §6 sans Gate 2 bloquant) + tool MCP + scope router
  + cron + tests. `make typegen` pour débloquer PR4b.
- **PR4a-bis (selon décision 1)** : câbler Gate 2 (verdict stocké + exposé), bloquant ou informatif.
- **PR4b (~3j)** : badge + modal + SSE + i18n FR/EN/ES + intercept archive manuel.

TDD strict : tests AVANT le code à chaque PR (parsing/dedup/evidence-reject/
scope-routing/re-lint-rollback/cron-staleness/i18n-parity).

---

## Liens

- `docs/research/provenance-rfcs.md` § RFC-6 — le niveau-2 NLI (Gate 2), avec la
  preuve empirique `nth-of-type` issue de la repasse 2026-05-31.
- `backend/docs/conventions/agents-md-format-v1.md` — grammaire `[src:]` (Gate 1).
- `backend/src/core/anti_halluc.rs` — `verify_source_marker`/`analyze` (Gate 1, livré).
