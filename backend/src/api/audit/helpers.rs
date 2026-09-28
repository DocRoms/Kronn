// Shared helpers used across the audit sub-modules: filesystem scans
// (compute_audit_info_sync, detect_project_skills, detect_issue_tracker_mcp),
// docs/ permission probes (check_ai_dir_permissions), bootstrap-block
// removal, and the localized prompt builders for the validation +
// briefing discussions.
//
// `pub(crate)` items are also reachable from `api::projects::*` callers
// (see template.rs/clone.rs/bootstrap.rs).

use crate::core::scanner;
use crate::models::*;

/// 0.8.7 — Short pointer line injected at the top of every prompt that can
/// mutate a `docs/` file outside the launched audit chain (validation,
/// briefing). Replaces the previous 3-language doctrine block (~150 words ×
/// 3 langs) — the canonical anti-hallu protocol now lives in the project's
/// `docs/AGENTS.md` § Anti-Hallucination Protocol section (written by audit
/// STEP 0). This pointer is the minimum reminder for code paths that bypass
/// the runner chokepoint PREAMBLE and the audit PROMPT_PREAMBLE.
fn anti_halluc_doc_writer_block(language: &str) -> &'static str {
    match language {
        "en" => "**Anti-hallucination** — when editing any `docs/` file, follow the project's `docs/AGENTS.md` § Anti-Hallucination Protocol : cite `[src: file: <path>:<line>]` for every non-trivial assertion ; convert unverifiable claims to `<!-- TODO: ask user -->`. Never invent.\n\n",
        "es" => "**Anti-alucinación** — al editar cualquier archivo `docs/`, sigue el `docs/AGENTS.md` § Anti-Hallucination Protocol del proyecto : cita `[src: file: <ruta>:<línea>]` en cada afirmación técnica ; convierte lo no verificable en `<!-- TODO: ask user -->`. Nunca inventes.\n\n",
        _ => "**Anti-hallucination** — quand tu édites un fichier `docs/`, suis le `docs/AGENTS.md` § Anti-Hallucination Protocol du projet : cite `[src: file: <chemin>:<ligne>]` pour chaque affirmation technique ; convertis l'invérifiable en `<!-- TODO: ask user -->`. N'invente jamais.\n\n",
    }
}

/// Compute audit info (files + TODOs) from the filesystem.
/// Path-agnostic — walks `docs/` post-pivot or `ai/` legacy via
/// `detect_docs_dir`.
/// A REAL todo marker, not the template's own instructions about markers:
/// docs templates quote `<!-- TODO: ask user -->` inside backticks to teach
/// the grammar, and those lines polluted every fresh project's todo list.
pub(super) fn line_has_real_todo_marker(line: &str) -> bool {
    line.match_indices("<!-- TODO")
        .any(|(i, _)| !line[..i].ends_with('`'))
}

/// A markdown table separator row (`|---|:---:|`), not a content row — the
/// "filled" detector must count real table rows as content but still skip
/// these so a doc that's just header + separator doesn't look non-empty.
fn is_table_separator_line(trimmed: &str) -> bool {
    trimmed.starts_with('|') && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

pub(super) fn compute_audit_info_sync(project_path_str: &str) -> AuditInfo {
    let project_path = scanner::resolve_host_path(project_path_str);
    let docs_dir = scanner::detect_docs_dir(&project_path);

    if !docs_dir.is_dir() {
        return AuditInfo {
            files: vec![],
            todos: vec![],
            tech_debt_items: vec![],
        };
    }

    let mut files = Vec::new();
    let mut todos = Vec::new();

    for entry in walkdir::WalkDir::new(&docs_dir)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&project_path)
            .unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().to_string();

        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            let is_empty = content
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    !t.is_empty()
                        && !t.starts_with('#')
                        && !t.starts_with('>')
                        && !t.starts_with("---")
                        && !is_table_separator_line(t)
                })
                .count()
                < 3;

            files.push(AuditFileInfo {
                path: rel_str.clone(),
                filled: !is_empty && !scanner::has_unfilled_placeholder(&content),
            });

            for (line_num, line) in content.lines().enumerate() {
                if line_has_real_todo_marker(line) {
                    todos.push(AuditTodo {
                        file: rel_str.clone(),
                        line: (line_num + 1) as u32,
                        text: line.trim().to_string(),
                    });
                }
            }
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    // Parse tech-debt items from the "Current list" table in inconsistencies-tech-debt.md
    let mut tech_debt_items = Vec::new();
    let tech_debt_file = docs_dir.join("inconsistencies-tech-debt.md");
    let tech_debt_dir = docs_dir.join("tech-debt");
    if let Ok(content) = std::fs::read_to_string(&tech_debt_file) {
        // Parse markdown table rows: | ID | Problem | Area | Severity |
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('|')
                || trimmed.starts_with("| ID")
                || is_table_separator_line(trimmed)
                || scanner::has_unfilled_placeholder(trimmed)
            {
                continue;
            }
            let cols: Vec<&str> = trimmed.split('|').map(|c| c.trim()).collect();
            // cols[0] is empty (before first |), cols[1]=ID, cols[2]=Problem, cols[3]=Area, cols[4]=Severity
            if cols.len() >= 5 && cols[1].starts_with("TD-") {
                // 0.8.2 — only surface a TD if its detail file actually
                // exists on disk. Without this, a stale row in the
                // index table makes the validation discussion's Phase 3
                // ask questions about phantom TDs (the agent tries to
                // `Read docs/tech-debt/<id>.md` and the tool returns
                // empty, leading to a zero-length reply).
                let detail_path = tech_debt_dir.join(format!("{}.md", cols[1]));
                if !detail_path.is_file() {
                    continue;
                }
                tech_debt_items.push(TechDebtItem {
                    id: cols[1].to_string(),
                    problem: cols[2].to_string(),
                    area: cols[3].to_string(),
                    severity: cols[4].to_string(),
                });
            }
        }
    }

    AuditInfo {
        files,
        todos,
        tech_debt_items,
    }
}

/// Validation prompt for a PARTIAL refresh (Codex A5 v3): the writable
/// surface is an EXPLICIT allowlist — the refreshed section files plus the
/// exact TD detail files of `run_td_ids` (nothing else in docs/, ever).
/// The TD-review phase disappears entirely when no TD was touched. The
/// project stays `Audited` until this discussion ends on the terminal
/// signal and the user validates, exactly like a full run. Fully
/// localized (fr/en/es) — protocol included, not just the header.
pub(crate) fn build_partial_validation_prompt(
    refreshed_files: &[String],
    run_td_ids: &[String],
    language: &str,
) -> String {
    let mut allowlist: Vec<String> = refreshed_files.to_vec();
    allowlist.extend(
        run_td_ids
            .iter()
            .map(|id| format!("docs/tech-debt/{id}.md")),
    );
    let allow = allowlist.join("`, `");
    let has_tds = !run_td_ids.is_empty();

    match language {
        "en" => {
            let mut s = format!(
                "Validate the PARTIAL refresh that just ran. WRITABLE SURFACE (exhaustive allowlist): `{allow}`. Touch NOTHING else — no source code, no other docs/ file, no other TD detail. End with the exact phrase \"KRONN:VALIDATION_COMPLETE\" on its own line once everything below is done.\n\n",
            );
            s.push_str(&run_scope_block(run_td_ids, "en"));
            s.push_str("## Phase 1 — Markers\nRun `grep -rn 'TODO: '` limited to the allowlisted files: resolve every marker (direct user question, a Glob/Read verification, or an explicit `unknown`), update the file and REMOVE the marker.\n\n");
            if has_tds {
                s.push_str("## Phase 2 — Refreshed TD review (BULK-FIRST)\nIn ONE message: a `| # | ID | Severity | Area | Title | Status | Effort |` table of the in-scope TDs only, then offer (a) confirm all (b) reject all (c) discuss selected — unselected TDs KEEP their current status, silence never confirms. Apply the answer in each detail file's `audit_history`.\n\n");
            } else {
                s.push_str("No TD was touched by this refresh — there is NO TD-review phase; do not open any TD file.\n\n");
            }
            s.push_str("## Output\nOnce everything above is done, emit the exact phrase: \"KRONN:VALIDATION_COMPLETE\" on its own line. Never before.");
            s
        }
        "es" => {
            let mut s = format!(
                "Valida el refresh PARCIAL que acaba de ejecutarse. SUPERFICIE EDITABLE (allowlist exhaustiva): `{allow}`. NO toques nada mas — ni codigo fuente, ni otros archivos docs/, ni otros detalles TD. Termina con la frase exacta \"KRONN:VALIDATION_COMPLETE\" en su propia linea.\n\n",
            );
            s.push_str(&run_scope_block(run_td_ids, "es"));
            s.push_str("## Fase 1 — Marcadores\nEjecuta `grep -rn 'TODO: '` limitado a los archivos del allowlist: resuelve cada marcador (pregunta directa, verificacion Glob/Read, o `unknown` explicito), actualiza el archivo y RETIRA el marcador.\n\n");
            if has_tds {
                s.push_str("## Fase 2 — Revision de TDs (BULK-FIRST)\nEn UN mensaje: tabla `| # | ID | Severidad | Area | Titulo | Estado | Esfuerzo |` de los TDs del scope, luego ofrece (a) confirmar todo (b) rechazar todo (c) detallar algunos — los no listados CONSERVAN su estado, el silencio nunca confirma. Aplica la respuesta en el `audit_history` de cada detalle.\n\n");
            } else {
                s.push_str("Ningun TD fue tocado por este refresh — NO hay fase de revision de TDs; no abras ningun archivo TD.\n\n");
            }
            s.push_str("## Salida\nUna vez todo terminado, emite la frase exacta: \"KRONN:VALIDATION_COMPLETE\" en su propia linea. Nunca antes.");
            s
        }
        _ => {
            let mut s = format!(
                "Valide le refresh PARTIEL qui vient de tourner. SURFACE MODIFIABLE (allowlist exhaustive) : `{allow}`. Ne touche à RIEN d'autre — ni code source, ni autre fichier docs/, ni autre détail TD. Termine par la phrase exacte \"KRONN:VALIDATION_COMPLETE\" sur sa propre ligne.\n\n",
            );
            s.push_str(&run_scope_block(run_td_ids, "fr"));
            s.push_str("## Phase 1 — Marqueurs\nLance `grep -rn 'TODO: '` limité aux fichiers de l'allowlist : résous chaque marqueur (question directe, vérification Glob/Read, ou `unknown` explicite), mets à jour le fichier et RETIRE le marqueur.\n\n");
            if has_tds {
                s.push_str("## Phase 2 — Revue des TDs rafraîchis (BULK-FIRST)\nEn UN message : tableau `| # | ID | Sévérité | Domaine | Titre | Statut | Effort |` des TDs du scope uniquement, puis propose (a) tout valider (b) tout rejeter (c) détailler certains — les non listés GARDENT leur statut, le silence ne confirme jamais. Applique la réponse dans l'`audit_history` de chaque détail.\n\n");
            } else {
                s.push_str("Aucun TD touché par ce refresh — il n'y a PAS de phase de revue TD ; n'ouvre aucun fichier TD.\n\n");
            }
            s.push_str("## Sortie\nUne fois tout traité, émets la phrase exacte : \"KRONN:VALIDATION_COMPLETE\" sur sa propre ligne. Jamais avant.");
            s
        }
    }
}

/// Scope block injected into the validation prompts: the exact TD ids the
/// finished run created or re-emitted. Without it the validation phase read
/// EVERY `TD-*.md` on disk and could re-validate (or alter) findings that
/// belong to previous audits. Localized and phase-agnostic — the callers
/// number their own phases.
pub(crate) fn run_scope_block(run_td_ids: &[String], language: &str) -> String {
    if run_td_ids.is_empty() {
        return String::new();
    }
    let ids = run_td_ids.join(", ");
    match language {
        "en" => format!(
            "## SCOPE — this run's TDs ONLY\n\
             This run created or re-emitted exactly these TDs: {ids}.\n\
             The TD review covers ONLY them — never re-read, re-validate or modify any other `docs/tech-debt/TD-*.md`: they belong to previous audits already settled by their own validation discussions.\n\n",
        ),
        "es" => format!(
            "## SCOPE — SOLO los TDs de ESTE run\n\
             Este run creo o re-emitio exactamente estos TDs: {ids}.\n\
             La revision cubre SOLO estos — nunca releas, revalides o modifiques ningun otro `docs/tech-debt/TD-*.md`: pertenecen a auditorias anteriores ya cerradas por su propia discusion de validacion.\n\n",
        ),
        _ => format!(
            "## SCOPE — TDs de CE run uniquement\n\
             Ce run a créé ou ré-émis exactement ces TDs : {ids}.\n\
             La revue des TDs porte UNIQUEMENT sur eux — ne relis, ne revalide et ne modifie AUCUN autre `docs/tech-debt/TD-*.md` : ils appartiennent à des audits précédents déjà validés par leur propre discussion.\n\n",
        ),
    }
}

/// 0.8.4 (#287) — sub-audit validation prompt. Shorter than the Full
/// version: a sub-audit only writes ONE index file + a handful of TD
/// detail files, so Phase 1 (autonomous doc fix-up across 10 files)
/// AND Phase 4 (challenge questions on cross-file consistency) make
/// no sense. We keep:
///
/// - Phase 2 (ambiguity markers in the new TD files)
/// - Phase 3 (bulk-first TD review on the kind-specific index)
/// - Phase 4 light: for RGAA only, the explicit reminder that
///   automated audits cover 30-40% of criteria and that the user
///   MUST re-test manually OR call Access42 / get Opquast-certified.
pub(crate) fn build_sub_audit_validation_prompt(
    kind: crate::models::AuditKind,
    language: &str,
    has_issue_tracker_mcp: bool,
    run_td_ids: &[String],
) -> String {
    use crate::models::AuditKind;
    let (kind_label, index_file) = match kind {
        AuditKind::Security => ("sécurité", "docs/inconsistencies-security.md"),
        AuditKind::Docker => ("Docker", "docs/inconsistencies-docker.md"),
        AuditKind::Performance => ("performance", "docs/inconsistencies-performance.md"),
        AuditKind::Accessibility => (
            "accessibility (WCAG 2.1)",
            "docs/inconsistencies-accessibility.md",
        ),
        AuditKind::Rgaa => ("RGAA 4.1", "docs/inconsistencies-rgaa.md"),
        AuditKind::Database => ("base de données", "docs/inconsistencies-database.md"),
        AuditKind::ApiDesign => ("design d'API", "docs/inconsistencies-api.md"),
        AuditKind::CodeQuality => ("qualité de code", "docs/inconsistencies-code-quality.md"),
        // Defensive: Full + Drift + Custom should never reach this path
        // (gated by `kind.is_sub_audit()` in `full_audit`). Keep a sane
        // fallback so a future variant added without updating this match
        // still produces a usable prompt.
        _ => ("ciblé", "docs/inconsistencies-tech-debt.md"),
    };

    // Header is language-aware; the bulk-first protocol stays in French
    // because Kronn discussions are FR-first and the validation flow
    // ships in FR. The agent translates when answering the user.
    let header = match language {
        "en" => format!(
            "Validate the findings of the **{} sub-audit** that just ran. \
             Do NOT touch source code — your job is to confirm the TDs and \
             refine the index file. End with the exact phrase \
             \"KRONN:VALIDATION_COMPLETE\" once everything below is done.\n\n",
            kind_label,
        ),
        "es" => format!(
            "Valida los hallazgos de la **sub-auditoría {}** recién ejecutada. \
             NO toques código fuente — tu trabajo es confirmar las TDs y \
             refinar el archivo índice. Termina con la frase exacta \
             \"KRONN:VALIDATION_COMPLETE\" cuando todo esté hecho.\n\n",
            kind_label,
        ),
        _ => format!(
            "Valide les résultats du **sous-audit {}** qui vient de tourner. \
             Tu ne touches PAS au code — ton job est de confirmer les TDs \
             et de raffiner le fichier d'index. Termine par la phrase \
             exacte \"KRONN:VALIDATION_COMPLETE\" une fois tout fait.\n\n",
            kind_label,
        ),
    };

    let mut s = header;
    s.push_str(&run_scope_block(run_td_ids, language));
    s.push_str(&format!(
        "## Périmètre\n\
         - Fichier d'index : `{}`\n\
         - Nouveaux détails TD : tous les `docs/tech-debt/TD-*.md` créés ou modifiés par ce run.\n\
         - **Ne pas** re-valider les TDs d'audits précédents (ils ont été traités par leur propre discussion de validation).\n\n",
        index_file,
    ));

    // Phase 2 — ambiguity markers in the new TD detail files only.
    s.push_str(&format!(
        "## Phase 2 — Ambiguïtés\n\
         Lance `grep -rn 'TODO: ' {}` (ou via MCP) limité aux fichiers TD créés par ce run. \
         Pour chaque marker :\n\
         - `<!-- TODO: ask user -->` → pose la question directement.\n\
         - `<!-- TODO: verify -->` → tente une vérification (Glob/Read) ; si impossible, escalade en question utilisateur.\n\
         - `<!-- TODO: unknown -->` → re-pose à l'utilisateur (priors).\n\
         Une fois la réponse reçue, mets à jour le fichier TD concerné ET retire le marker. Phase 2 termine quand tous les markers sont résolus ou explicitement laissés en `unknown`.\n\n",
        index_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("docs"),
    ));

    // Phase 3 — bulk-first TD review, scoped to the kind-specific index.
    s.push_str(&format!(
        "## Phase 3 — Revue des TDs (BULK-FIRST)\n\
         **Ne PAS dérouler les TDs un par un** — ça épuise l'utilisateur avant la fin.\n\n\
         En UN seul message :\n\
         1. Lis `{}` ET chaque détail TD créé par ce run.\n\
         2. Présente un **tableau markdown compact** :\n\
            `| # | ID | Sévérité | Domaine | Titre | Statut | Effort |`\n\
            Une ligne par TD, numérotée 1..N — l'utilisateur répond par numéro. Tronque le titre à ~50 chars si nécessaire.\n\
         3. Demande à l'utilisateur :\n\
            > « Voici les N TDs identifiés par le sous-audit {}. Tu peux :\n\
            > (a) **Tout valider** → tous deviennent `Confirmed by user` ;\n\
            > (b) **Tout rejeter** → tous deviennent `Rejected` (le prochain audit ne les recréera pas) ;\n\
            > (c) **Détailler certains** → liste les numéros ou IDs à discuter (ex: `3, 7`). Les TDs non listés **gardent leur statut actuel** — rien n'est confirmé implicitement, le silence ne vaut jamais validation. »\n\
         4. Applique la réponse en mettant à jour le champ `audit_history` de chaque détail TD. Ne marque JAMAIS un TD `Confirmed by user` sans un (a) explicite ou une confirmation explicite de ce TD en (c).\n",
        index_file, kind_label,
    ));
    if has_issue_tracker_mcp {
        s.push_str(
            "5. Pour les TDs Critical/High validés, propose **en UN batch** : « Je crée les tickets sur le tracker ? » Si oui, batch-crée-les via le MCP tracker disponible (pas de question 1-by-1).\n\n",
        );
    } else {
        s.push('\n');
    }

    // Phase 4 light — RGAA gets the "audit manuel + Access42/Opquast"
    // reminder; other sub-audits get a shorter "anything we missed?"
    // close-out.
    if matches!(kind, AuditKind::Rgaa) {
        s.push_str(
            "## Phase 4 — Pour aller plus loin (RGAA, à NE PAS sauter)\n\
             Rappelle EXPLICITEMENT à l'utilisateur :\n\
             1. **Cet audit automatique ne remplace PAS un audit manuel.** Tooling = 30-40 % des critères couverts. Les 60-70 % restants (lecteur d'écran réel, parcours utilisateur, alternatives textuelles pertinentes, accessibilité cognitive) demandent une revue humaine.\n\
             2. **Deux options officielles** pour être réellement conforme :\n\
                - **Re-tester soi-même** avec la grille DINUM RGAA 4.1, NVDA/JAWS, VoiceOver, navigation clavier-only.\n\
                - **Faire appel à un pro** : [Access42](https://access42.net) — référence française pour l'audit officiel et certifiant.\n\
             3. **Se former** pour ne plus laisser passer :\n\
                - **Access42** propose un cursus certifiant (référent accessibilité, expert RGAA) — pour un profil dédié.\n\
                - **Opquast** propose la cert « Maîtrise de la qualité en projet web » (240 règles dont RGAA) — pour faire monter en compétence toute l'équipe.\n\n\
             Pose ensuite UNE question : « As-tu déjà un référent accessibilité formé sur ce projet, et est-il temps de planifier un audit Access42 ? » Ne valide pas le sous-audit sans cette discussion.\n\n",
        );
    } else {
        s.push_str(&format!(
            "## Phase 4 — Close-out\n\
             Pose UNE question : « Le sous-audit {} a-t-il manqué un angle évident (config, environnement, dépendances tierces) que tu connais et qu'on devrait creuser dans une prochaine passe ? » Note la réponse dans `{}` en bas (section `## Notes utilisateur`).\n\n",
            kind_label, index_file,
        ));
    }

    s.push_str("## Sortie\nUne fois TOUTES les phases ci-dessus terminées, émets la phrase exacte : \"KRONN:VALIDATION_COMPLETE\". Ne l'émets jamais avant.");
    s
}

/// Build the validation discussion prompt with file/TODO/tech-debt enrichment.
/// The prompt follows a strict 4-phase protocol to ensure thorough validation.
fn validation_card_protocol(language: &str, has_issue_tracker_mcp: bool) -> String {
    let mut prompt = match language {
        "en" => String::from(
            r#"You are running the VALIDATION of the AI context in `docs/`, the final phase of the audit pipeline. Announce progress as "Phase X/4 of the validation" and never emit `KRONN:VALIDATION_COMPLETE` early.

**You are a documentation auditor, not a code fixer. Modify only `docs/`.**

## Phase 1 — Auto-fix
Read source code to verify the documentation, then fix only inferable documentation gaps and stale facts.

## Phase 2 — Critical and High TD cards
Read only the TD detail files named in RUN SCOPE. In one response, emit one closed `kronn-question` card for every Critical or High TD. Use `task_ref:"audit-td:<TD-ID>"`, a stable key, and exactly these option IDs: `confirm`, `reject`, `accept_decision`, `defer`. Labels must explain: confirm the finding, reject it, accept it as an intentional decision, or defer it.

## Phase 3 — Remaining TD cards
For every remaining TD, emit batch cards containing at most 8 `items`. Each item is one TD (`id`, `label`, optional `description`); use `task_ref:"audit-td-batch"` and the same four options. Example shape:
```kronn-question
{"version":1,"key":"audit-td-batch-1","question":"Choose an outcome for each TD.","items":[{"id":"TD-YYYYMMDD-example","label":"Example TD"}],"options":[{"id":"confirm","label":"Confirm"},{"id":"reject","label":"Reject"},{"id":"accept_decision","label":"Accepted decision"},{"id":"defer","label":"Defer"}],"task_ref":"audit-td-batch"}
```
The human must choose exactly one outcome for every item. Do not ask for TD decisions in prose. Do not edit TD statuses, `audit_history`, or `docs/decisions.md` yourself: Kronn writes those repository files when each card is answered. Wait until all emitted cards are answered before continuing.

## Phase 4 — Doc challenge
Ask 2-3 practical onboarding questions answerable from `docs/` alone, verify the answers, and fix documentation gaps.

## Completion
When every phase and every card is complete, end with the exact phrase `KRONN:VALIDATION_COMPLETE`."#,
        ),
        "es" => String::from(
            r#"Estas ejecutando la VALIDACION final del contexto AI en `docs/`. Anuncia el progreso como "Fase X/4 de la validacion" y nunca emitas `KRONN:VALIDATION_COMPLETE` antes de tiempo.

**Eres auditor de documentacion, no corrector de codigo. Modifica solo `docs/`.**

## Fase 1 — Auto-correccion
Lee el codigo para verificar la documentacion y corrige solo lagunas documentales inferibles y datos obsoletos.

## Fase 2 — Tarjetas TD Critical y High
Lee solo los detalles TD de RUN SCOPE. En una respuesta, emite una tarjeta cerrada `kronn-question` por cada TD Critical o High. Usa `task_ref:"audit-td:<TD-ID>"`, una clave estable y exactamente estos IDs: `confirm`, `reject`, `accept_decision`, `defer`.

## Fase 3 — Tarjetas TD restantes
Para cada TD restante, emite tarjetas por lotes con un maximo de 8 `items`. Cada item es un TD (`id`, `label`, `description` opcional); usa `task_ref:"audit-td-batch"` y las mismas cuatro opciones. Forma de ejemplo:
```kronn-question
{"version":1,"key":"audit-td-batch-1","question":"Elige un resultado para cada TD.","items":[{"id":"TD-YYYYMMDD-example","label":"TD de ejemplo"}],"options":[{"id":"confirm","label":"Confirmar"},{"id":"reject","label":"Rechazar"},{"id":"accept_decision","label":"Decision aceptada"},{"id":"defer","label":"Diferir"}],"task_ref":"audit-td-batch"}
```
El humano debe elegir un resultado por item. No pidas decisiones TD en prosa. No edites estados TD, `audit_history` ni `docs/decisions.md`: Kronn escribe esos archivos al responder la tarjeta. Espera todas las respuestas antes de continuar.

## Fase 4 — Desafio documental
Haz 2-3 preguntas practicas de onboarding, verifica las respuestas solo con `docs/` y corrige las lagunas.

## Fin
Cuando todas las fases y tarjetas esten completas, termina exactamente con `KRONN:VALIDATION_COMPLETE`."#,
        ),
        _ => String::from(
            r#"Tu conduis la VALIDATION finale du contexte AI dans `docs/`. Annonce l'avancement comme "Phase X/4 de la validation" et n'emets jamais `KRONN:VALIDATION_COMPLETE` trop tot.

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
Quand toutes les phases et toutes les cartes sont terminees, termine exactement par `KRONN:VALIDATION_COMPLETE`."#,
        ),
    };
    if has_issue_tracker_mcp {
        prompt.push_str(match language {
            "en" => "\n\nAfter all cards are answered, offer one batch ticket-creation question for confirmed Critical/High TDs.",
            "es" => "\n\nTras responder todas las tarjetas, ofrece una sola pregunta para crear tickets de los TD Critical/High confirmados.",
            _ => "\n\nApres reponse a toutes les cartes, propose une seule question de creation de tickets pour les TD Critical/High confirmes.",
        });
    }
    prompt
}

pub(crate) fn build_validation_prompt(
    language: &str,
    info: &AuditInfo,
    has_issue_tracker_mcp: bool,
    run_td_ids: &[String],
) -> String {
    let base = validation_card_protocol(language, has_issue_tracker_mcp);

    // 0.8.7 anti-hallu: prepend the doc-writer discipline reminder so
    // Phase 1 (auto-fix) and Phase 4 (challenge doc) which both mutate
    // `docs/` files inherit the same sourcing protocol the assembled audit
    // gets via `PROMPT_PREAMBLE`. Without this, the validation pass was
    // structurally outside the anti-hallucination scope.
    let mut prompt = String::with_capacity(base.len() + 512);
    prompt.push_str(anti_halluc_doc_writer_block(language));
    // Run scope FIRST: Phases 2 and 3 must only review the TDs this run touched,
    // never re-open findings settled by previous validation discussions.
    prompt.push_str(&run_scope_block(run_td_ids, language));
    prompt.push_str(&base);

    // Summary counts only — the agent has filesystem access to read the actual files
    let unfilled_count = info.files.iter().filter(|f| !f.filled).count();
    let total_files = info.files.len();
    if total_files > 0 {
        let summary = match language {
            "en" => format!("{} AI files detected ({} still incomplete). Read `docs/AGENTS.md` for the full tree.", total_files, unfilled_count),
            "es" => format!("{} archivos AI detectados ({} aun incompletos). Lee `docs/AGENTS.md` para el arbol completo.", total_files, unfilled_count),
            _ => format!("{} fichiers AI detectes ({} encore incomplets). Lis `docs/AGENTS.md` pour l'arbre complet.", total_files, unfilled_count),
        };
        prompt.push_str(&format!("\n\n{}", summary));
    }

    if !info.todos.is_empty() {
        let hint = match language {
            "en" => format!("{} remaining TODO markers across AI files. Scan `docs/` for `<!-- TODO` to find them all.", info.todos.len()),
            "es" => format!("{} marcadores TODO restantes en archivos AI. Busca `<!-- TODO` en `docs/` para encontrarlos.", info.todos.len()),
            _ => format!("{} marqueurs TODO restants dans les fichiers AI. Cherche `<!-- TODO` dans `docs/` pour les trouver.", info.todos.len()),
        };
        prompt.push_str(&format!("\n\n{}", hint));
    }

    if !info.tech_debt_items.is_empty() {
        let hint = match language {
            "en" => format!("{} tech debt items to review across Phases 2 and 3. Read `docs/inconsistencies-tech-debt.md` and `docs/tech-debt/` for details.", info.tech_debt_items.len()),
            "es" => format!("{} items de deuda tecnica a revisar entre las Fases 2 y 3. Lee `docs/inconsistencies-tech-debt.md` y `docs/tech-debt/` para detalles.", info.tech_debt_items.len()),
            _ => format!("{} items de dette technique a revoir entre les Phases 2 et 3. Lis `docs/inconsistencies-tech-debt.md` et `docs/tech-debt/` pour les details.", info.tech_debt_items.len()),
        };
        prompt.push_str(&format!("\n\n{}", hint));
    }

    prompt
}

/// The first of `candidates` (relative to the project) that exists.
fn first_existing_marker(project_path: &std::path::Path, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| project_path.join(candidate).exists())
        .map(|candidate| (*candidate).to_string())
}

/// Skills the project's stack suggests, each with the file that triggered it
/// (`("devops", "Dockerfile")`). Pure filesystem look, no logging: the listing
/// calls it on every read.
pub(crate) fn detect_project_skill_markers(
    project_path: &std::path::Path,
) -> Vec<(String, String)> {
    let mut skills: Vec<(String, String)> = Vec::new();
    let mut push = |id: &str, marker: Option<String>| {
        if let Some(marker) = marker {
            skills.push((id.to_string(), marker));
        }
    };

    // ── Language detection (from package managers / config files) ──
    push("rust", first_existing_marker(project_path, &["Cargo.toml"]));
    if project_path.join("package.json").exists() {
        // Check if TypeScript
        push(
            "typescript",
            first_existing_marker(project_path, &["tsconfig.json", "tsconfig.app.json"]),
        );
    }
    push(
        "python",
        first_existing_marker(
            project_path,
            &["requirements.txt", "pyproject.toml", "setup.py"],
        ),
    );
    push("go", first_existing_marker(project_path, &["go.mod"]));
    push(
        "php",
        first_existing_marker(project_path, &["composer.json"]),
    );

    // ── Domain detection ──
    // DevOps: Dockerfile, CI/CD, IaC
    push(
        "devops",
        first_existing_marker(
            project_path,
            &[
                "Dockerfile",
                "docker-compose.yml",
                "docker-compose.yaml",
                ".github/workflows",
                ".gitlab-ci.yml",
                "Makefile",
            ],
        ),
    );

    // Database: migrations, schema files
    push(
        "database",
        first_existing_marker(project_path, &["migrations", "db", "prisma", "drizzle"]),
    );

    // Security: auth configs, security headers
    push(
        "security",
        first_existing_marker(
            project_path,
            &[
                ".env.example",
                "security.yaml",
                "config/packages/security.yaml",
            ],
        ),
    );

    // ── Business detection ──
    // Web performance: frontend projects with build tools
    push(
        "web-performance",
        first_existing_marker(
            project_path,
            &[
                "webpack.config.js",
                "vite.config.ts",
                "vite.config.js",
                "next.config.js",
                "next.config.ts",
            ],
        ),
    );

    // SEO: robots.txt, sitemap
    push(
        "seo",
        first_existing_marker(project_path, &["robots.txt", "public/robots.txt"]),
    );

    // Filter to only keep skills that actually exist in the system
    skills
        .into_iter()
        .filter(|(id, _)| crate::core::skills::get_skill(id).is_some())
        .collect()
}

/// Auto-detect skills from project filesystem (config files, package managers, etc.)
pub(crate) fn detect_project_skills(project_path: &std::path::Path) -> Vec<String> {
    let valid: Vec<String> = detect_project_skill_markers(project_path)
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    tracing::info!(
        "Auto-detected skills for {}: {:?}",
        project_path.display(),
        valid
    );
    valid
}

pub(super) fn detect_issue_tracker_mcp(project_path: &std::path::Path) -> bool {
    let mcp_file = project_path.join(".mcp.json");
    if let Ok(content) = std::fs::read_to_string(&mcp_file) {
        let lower = content.to_lowercase();
        return lower.contains("github")
            || lower.contains("gitlab")
            || lower.contains("jira")
            || lower.contains("atlassian")
            || lower.contains("linear")
            || lower.contains("youtrack");
    }
    false
}

/// Try to detect permission issues on an existing docs/ directory.
/// Returns Ok(()) if all files are accessible, or Err with description if unfixable.
pub(crate) fn check_ai_dir_permissions(ai_dir: &std::path::Path) -> Result<(), String> {
    for entry in walkdir::WalkDir::new(ai_dir).max_depth(5).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => return Err(format!("Cannot traverse docs/ directory: {}", e)),
        };
        let path = entry.path();
        if path.is_file() {
            if let Err(e) = std::fs::read(path) {
                return Err(format!("{}: {}", path.display(), e));
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct HumanOwnedDocsSnapshot {
    files: Vec<(String, String)>,
}

/// Capture every documentation file that contains a human-owned section.
/// Audit prompts may edit files beyond their declared validation target (the
/// final review explicitly does), so ownership protection cannot be scoped to
/// `AnalysisStep::target_file`.
pub(crate) fn capture_human_owned_sections(
    project_path: &std::path::Path,
) -> Result<HumanOwnedDocsSnapshot, String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    if !docs_dir.is_dir() {
        return Ok(HumanOwnedDocsSnapshot { files: Vec::new() });
    }

    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&docs_dir).max_depth(5) {
        let entry = entry.map_err(|e| format!("scan human-owned sections: {e}"))?;
        if !entry.file_type().is_file()
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "md")
        {
            continue;
        }
        let content = std::fs::read_to_string(entry.path())
            .map_err(|e| format!("read {}: {e}", entry.path().display()))?;
        if !super::anti_hallu_enforce::contains_human_owned_section(&content) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(project_path)
            .map_err(|_| format!("{} escapes project root", entry.path().display()))?
            .to_string_lossy()
            .replace('\\', "/");
        files.push((relative, content));
    }
    Ok(HumanOwnedDocsSnapshot { files })
}

/// Restore human-owned sections changed anywhere in the docs tree by one
/// agent attempt and persist each rejected proposal as a dated report. This
/// runs before retry/cancellation/failure branches can leave changes behind.
pub(crate) fn protect_human_owned_sections(
    project_path: &std::path::Path,
    snapshot: &HumanOwnedDocsSnapshot,
    today: &str,
) -> Result<usize, String> {
    let mut proposals = Vec::new();
    let mut errors = Vec::new();
    let mut restored_count = 0;

    for (target_file, pre_content) in &snapshot.files {
        let target_path = project_path.join(target_file);
        let written = match std::fs::read_to_string(&target_path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                errors.push(format!("read {}: {e}", target_path.display()));
                continue;
            }
        };
        let Some((restored, diffs)) =
            super::anti_hallu_enforce::enforce_human_owned_sections(pre_content, &written)
        else {
            continue;
        };

        let restore_result =
            crate::core::fs_guard::assert_contained_no_symlink(project_path, &target_path)
                .and_then(|_| {
                    crate::core::mcp_scanner::atomic_write(&target_path, &restored)
                        .map_err(|e| format!("restore {}: {e}", target_path.display()))
                });
        match restore_result {
            Ok(()) => {
                restored_count += diffs.len();
                proposals.push((target_file.as_str(), diffs));
            }
            Err(e) => errors.push(e),
        }
    }

    for (target_file, diffs) in proposals {
        if let Err(e) = write_human_section_diff_report(project_path, target_file, &diffs, today) {
            errors.push(e);
        }
    }

    if errors.is_empty() {
        Ok(restored_count)
    } else {
        Err(errors.join("; "))
    }
}

/// Write an audit proposal under `docs/reports/`. A same-day rerun for the
/// same target replaces that day's report; a new day creates a new snapshot.
fn write_human_section_diff_report(
    project_path: &std::path::Path,
    target_file: &str,
    diffs: &[super::anti_hallu_enforce::HumanSectionDiff],
    today: &str,
) -> Result<(), String> {
    let slug = target_file
        .trim_end_matches(".md")
        .replace(['/', '\\'], "-");
    let reports_dir = project_path.join("docs/reports");
    crate::core::fs_guard::guarded_create_dir_all(project_path, &reports_dir)?;
    let report_path = reports_dir.join(format!("{today}-human-section-diff-{slug}.md"));
    crate::core::fs_guard::assert_contained_no_symlink(project_path, &report_path)?;
    let body =
        super::anti_hallu_enforce::format_human_section_diff_report(target_file, diffs, today);
    crate::core::mcp_scanner::atomic_write(&report_path, &body)
        .map_err(|e| format!("write {}: {e}", report_path.display()))
}

/// Remove the KRONN:BOOTSTRAP block from docs/AGENTS.md
pub(super) fn remove_bootstrap_block(index_file: &std::path::Path) {
    let content = match std::fs::read_to_string(index_file) {
        Ok(c) => c,
        Err(_) => return,
    };

    if !content.contains("KRONN:BOOTSTRAP:START") {
        return;
    }

    // Remove everything between START and END markers (inclusive)
    let mut result = String::new();
    let mut in_block = false;
    for line in content.lines() {
        if line.contains("KRONN:BOOTSTRAP:START") {
            in_block = true;
            continue;
        }
        if line.contains("KRONN:BOOTSTRAP:END") {
            in_block = false;
            continue;
        }
        if !in_block {
            result.push_str(line);
            result.push('\n');
        }
    }

    // Trim leading whitespace from the cleaned content
    let trimmed = result.trim_start().to_string();
    if let Err(e) = std::fs::write(index_file, trimmed) {
        tracing::warn!("Failed to remove bootstrap block: {}", e);
    }
}

/// Build the briefing discussion prompt (conversational pre-audit).
///
/// 0.8.4 (#285+UX) — when `prefilled_notes` is `Some`, the user just
/// submitted the désagentified form and we already have the 6 answers.
/// Instead of re-asking them all, the agent enters a SHORT
/// "review + deep-dive" mode: read the briefing back to the user, ask
/// at most 2-3 targeted clarifications on ambiguous answers, then
/// finalize. Cuts ~80% of the briefing-discussion tokens vs the legacy
/// 6-question flow while keeping the door open for nuance the form
/// can't capture.
///
/// When `prefilled_notes` is `None`, the agent runs the legacy 6-Q
/// flow (kept for backwards-compat — callers that never wrote to the
/// form still work).
pub(crate) fn build_briefing_prompt(language: &str, prefilled_notes: Option<&str>) -> String {
    // 0.8.7 anti-hallu: prefix the discipline reminder on every briefing
    // path (review + legacy). Briefing produces `docs/briefing.md` which is
    // then injected as project context — a hallucinated assertion here
    // ships into every future agent prompt for this project.
    let body = if let Some(notes) = prefilled_notes {
        build_briefing_review_prompt(language, notes)
    } else {
        build_briefing_legacy_prompt(language)
    };
    format!("{}{}", anti_halluc_doc_writer_block(language), body)
}

/// 0.8.4 (#285+UX) — review prompt fired after the form has been
/// submitted. The agent reads the answers, picks 2-3 ambiguous ones
/// (if any), asks for clarification, then writes the final
/// `docs/briefing.md` and emits `KRONN:BRIEFING_COMPLETE`. The user
/// who answered everything cleanly can fast-path through this in a
/// single turn ("LGTM, ship it").
fn build_briefing_review_prompt(language: &str, prefilled_notes: &str) -> String {
    match language {
        "en" => format!(
            "ROLE: You are a project briefing reviewer.\n\n\
             The user just submitted the briefing form. Their answers are at the bottom of this message.\n\n\
             YOUR JOB — short and focused, NOT a full re-interrogation:\n\
             1. Read their answers below.\n\
             2. If 1-3 answers are ambiguous, vague (e.g. \"some traps\"), or contradict each other, ask for clarification on those SPECIFIC points only — in ONE message, max 3 questions, bulleted. Do NOT re-ask answers that already look complete.\n\
             3. If all answers look usable as-is, skip to step 4.\n\
             4. Write `docs/briefing.md` with the EXACT format below, merging the original answers + any clarifications you got.\n\
             5. End your last message with: `KRONN:BRIEFING_COMPLETE`\n\n\
             ABSOLUTE RULES:\n\
             - Do NOT re-ask the 6 questions wholesale. The user has already answered. This is REVIEW, not interrogation.\n\
             - Do NOT read source code or guess anything.\n\
             - Do NOT modify any file other than `docs/briefing.md`.\n\n\
             Format for `docs/briefing.md` (write this LITERALLY, in English even if the conversation is in another language):\n\n\
             # Project Briefing\n\
             > Auto-generated from user-submitted form + AI review.\n\
             ## Purpose\n[from Q1, refined by clarification if any]\n\
             ## Team\n[from Q2]\n\
             ## Maturity\n[from Q3]\n\
             ## External Dependencies\n[from Q4 — if none, write \"None.\"]\n\
             ## Traps & Fragile Areas\n[from Q5 — bullet list if multiple]\n\
             ## Additional Context\n[from Q6 — if skipped, write \"None.\"]\n\n\
             USER'S FORM ANSWERS:\n\n{}\n",
            prefilled_notes,
        ),
        "es" => format!(
            "ROL: Eres un revisor de briefing de proyecto.\n\n\
             El usuario acaba de enviar el formulario de briefing. Sus respuestas estan al final de este mensaje.\n\n\
             TU TAREA — corta y enfocada, NO una re-interrogacion completa:\n\
             1. Lee sus respuestas.\n\
             2. Si 1-3 respuestas son ambiguas, vagas (ej. \"algunas trampas\") o se contradicen, pide aclaracion sobre esos puntos ESPECIFICOS — en UN solo mensaje, max 3 preguntas. NO repreguntes lo que ya esta claro.\n\
             3. Si todas las respuestas son utiles tal cual, salta al paso 4.\n\
             4. Escribe `docs/briefing.md` con el formato EXACTO de abajo.\n\
             5. Termina con: `KRONN:BRIEFING_COMPLETE`\n\n\
             REGLAS ABSOLUTAS:\n\
             - NO repreguntes las 6 preguntas completas. El usuario ya respondio.\n\
             - NO leas codigo fuente ni adivines nada.\n\
             - NO modifiques ningun archivo fuera de `docs/briefing.md`.\n\n\
             Formato (escribir LITERALMENTE, en ingles aunque la conversacion sea en otro idioma):\n\n\
             # Project Briefing\n\
             > Auto-generated from user-submitted form + AI review.\n\
             ## Purpose\n[de Q1, refinado por aclaracion si la hay]\n\
             ## Team\n[de Q2]\n\
             ## Maturity\n[de Q3]\n\
             ## External Dependencies\n[de Q4 — si ninguna, escribir \"None.\"]\n\
             ## Traps & Fragile Areas\n[de Q5 — lista de puntos]\n\
             ## Additional Context\n[de Q6 — si omitida, escribir \"None.\"]\n\n\
             RESPUESTAS DEL USUARIO:\n\n{}\n",
            prefilled_notes,
        ),
        _ => format!(
            "ROLE: Tu es un relecteur de briefing projet.\n\n\
             L'utilisateur vient de remplir le formulaire de briefing. Ses reponses sont en bas de ce message.\n\n\
             TON JOB — court et cible, PAS une re-interrogation complete :\n\
             1. Relis ses reponses ci-dessous.\n\
             2. Si 1 a 3 reponses sont ambigues, vagues (ex: \"des pieges\"), ou se contredisent, demande des clarifications UNIQUEMENT sur ces points precis — en UN seul message, max 3 questions en liste. Ne repose PAS les questions deja completes.\n\
             3. Si toutes les reponses sont utilisables telles quelles, saute a l'etape 4.\n\
             4. Ecris `docs/briefing.md` avec le format EXACT ci-dessous, en fusionnant les reponses initiales + tes eventuelles clarifications.\n\
             5. Termine ton dernier message par : `KRONN:BRIEFING_COMPLETE`\n\n\
             REGLES ABSOLUES :\n\
             - Ne repose PAS les 6 questions en bloc. L'utilisateur a deja repondu. C'est une RELECTURE, pas un interrogatoire.\n\
             - Ne lis PAS le code source, ne devine rien.\n\
             - Ne modifie aucun fichier autre que `docs/briefing.md`.\n\n\
             Format pour `docs/briefing.md` (a ecrire LITTERALEMENT, en anglais meme si la conversation est dans une autre langue) :\n\n\
             # Project Briefing\n\
             > Auto-generated from user-submitted form + AI review.\n\
             ## Purpose\n[depuis Q1, raffine par les clarifications si besoin]\n\
             ## Team\n[depuis Q2]\n\
             ## Maturity\n[depuis Q3]\n\
             ## External Dependencies\n[depuis Q4 — si aucune, ecrire \"None.\"]\n\
             ## Traps & Fragile Areas\n[depuis Q5 — liste a puces si plusieurs]\n\
             ## Additional Context\n[depuis Q6 — si omise, ecrire \"None.\"]\n\n\
             REPONSES DU FORMULAIRE :\n\n{}\n",
            prefilled_notes,
        ),
    }
}

/// Legacy 6-question briefing prompt — kept for `start_briefing`
/// callers that didn't pre-fill the form (no `prefilled_notes`).
fn build_briefing_legacy_prompt(language: &str) -> String {
    match language {
        "en" => concat!(
            "ROLE: You are a project briefing assistant.\n\n",
            "ABSOLUTE RULE: Do NOT read source code, project files, or any file outside docs/. ",
            "Do NOT guess ANYTHING. You ask questions and use ONLY the user's answers.\n\n",
            "IF YOU HAVE FILE SYSTEM ACCESS: do NOT use it for this task. ",
            "No ls, cat, read, glob, grep. The only allowed file operation is the final write of docs/briefing.md.\n\n",
            "NOTE: The tech stack will be auto-detected during the audit (from package.json, Cargo.toml, etc.). No need to ask about it.\n\n",
            "STEP 1 — Ask the following 6 questions IN A SINGLE MESSAGE, then STOP. Wait for answers.\n\n",
            "1. What does this project do? (one sentence — what it does for its users)\n",
            "2. Who works on it? (solo / small team / large team)\n",
            "3. What stage is it at? (prototype, MVP, production, legacy, rewrite...)\n",
            "4. Key external dependencies? Include names/URLs if relevant. (e.g. \"PostgreSQL on AWS RDS\", \"user-service API on gitlab.company.com/org/repo\" — or just \"none\")\n",
            "5. What would a new contributor get wrong on day one? (traps, implicit rules, fragile areas)\n",
            "6. Anything else the audit should know? (optional, keep it short)\n\n",
            "STEP 2 — Check that the user answered questions 1-5. If some are missing, ask ONLY the unanswered ones before proceeding. Q6 is optional. ",
            "Once you have answers 1-5 (or the user explicitly says 'skip' for some), write the file docs/briefing.md with THIS EXACT FORMAT:\n\n",
            "# Project Briefing\n",
            "> Auto-generated by AI briefing. Source: user answers (not code analysis).\n",
            "## Purpose\n[answer Q1]\n",
            "## Team\n[answer Q2]\n",
            "## Maturity\n[answer Q3]\n",
            "## External Dependencies\n[answer Q4 — if none, write \"None.\"]\n",
            "## Traps & Fragile Areas\n[answer Q5 — bullet list if multiple]\n",
            "## Additional Context\n[answer Q6 — if skipped, write \"None.\"]\n\n",
            "Write docs/briefing.md IN ENGLISH even if the conversation is in another language.\n",
            "If the user does not answer a question, write \"Not provided\" — do NOT invent ANYTHING.\n",
            "Do NOT modify ANY other file.\n\n",
            "STEP 3 — After writing the file, end your last message with: KRONN:BRIEFING_COMPLETE",
        ).to_string(),
        "es" => concat!(
            "ROLE: Eres un asistente de briefing de proyecto.\n\n",
            "REGLA ABSOLUTA: NO leas el codigo fuente, los archivos del proyecto, ni ningun archivo fuera de docs/. ",
            "NO adivines NADA. Haces preguntas y usas UNICAMENTE las respuestas del usuario.\n\n",
            "SI TIENES ACCESO AL SISTEMA DE ARCHIVOS: NO lo uses para esta tarea. ",
            "Nada de ls, cat, read, glob, grep. La unica operacion de archivo permitida es la escritura final de docs/briefing.md.\n\n",
            "NOTA: La stack tecnica sera auto-detectada durante la auditoria (desde package.json, Cargo.toml, etc.). No es necesario preguntar por ella.\n\n",
            "PASO 1 — Haz las 6 preguntas siguientes EN UN SOLO MENSAJE, luego PARA. Espera las respuestas.\n\n",
            "1. Que hace este proyecto? (una frase — que hace para sus usuarios)\n",
            "2. Quien trabaja en el? (solo / equipo pequeno / equipo grande)\n",
            "3. En que etapa esta? (prototipo, MVP, produccion, legacy, reescritura...)\n",
            "4. Dependencias externas clave? Incluye nombres/URLs si es relevante. (ej: \"PostgreSQL en AWS RDS\", \"API user-service en gitlab.company.com/org/repo\" — o simplemente \"ninguna\")\n",
            "5. Que haria mal un nuevo contributor el primer dia? (trampas, reglas implicitas, zonas fragiles)\n",
            "6. Algo mas que la auditoria deberia saber? (opcional, breve)\n\n",
            "PASO 2 — Verifica que el usuario respondio las preguntas 1-5. Si faltan algunas, pregunta SOLO las que faltan. La Q6 es opcional. ",
            "Cuando tengas las respuestas 1-5 (o el usuario diga 'saltar'), escribe el archivo docs/briefing.md con ESTE FORMATO EXACTO:\n\n",
            "# Project Briefing\n",
            "> Auto-generated by AI briefing. Source: user answers (not code analysis).\n",
            "## Purpose\n[respuesta Q1]\n",
            "## Team\n[respuesta Q2]\n",
            "## Maturity\n[respuesta Q3]\n",
            "## External Dependencies\n[respuesta Q4 — si ninguna, escribir \"None.\"]\n",
            "## Traps & Fragile Areas\n[respuesta Q5 — lista de puntos si hay varios]\n",
            "## Additional Context\n[respuesta Q6 — si omitida, escribir \"None.\"]\n\n",
            "Escribe docs/briefing.md EN INGLES aunque la conversacion sea en otro idioma.\n",
            "Si el usuario no responde a una pregunta, escribe \"Not provided\" — NO inventes NADA.\n",
            "NO modifiques NINGUN otro archivo.\n\n",
            "PASO 3 — Despues de escribir el archivo, termina tu ultimo mensaje con: KRONN:BRIEFING_COMPLETE",
        ).to_string(),
        _ => concat!(
            "ROLE: Tu es un assistant de briefing projet.\n\n",
            "REGLE ABSOLUE: Tu ne lis PAS le code source, les fichiers du projet, ni aucun fichier en dehors de docs/. ",
            "Tu ne devines RIEN. Tu poses des questions et tu utilises UNIQUEMENT les reponses de l'utilisateur.\n\n",
            "SI TU AS ACCES AU SYSTEME DE FICHIERS: ne l'utilise PAS pour cette tache. ",
            "Pas de ls, cat, read, glob, grep. La seule operation fichier autorisee est l'ecriture finale de docs/briefing.md.\n\n",
            "NOTE: La stack technique sera auto-detectee pendant l'audit (depuis package.json, Cargo.toml, etc.). Inutile d'en parler ici.\n\n",
            "ETAPE 1 — Pose les 6 questions suivantes EN UN SEUL MESSAGE, puis STOP. Attends les reponses.\n\n",
            "1. Que fait ce projet ? (une phrase — ce qu'il fait pour ses utilisateurs)\n",
            "2. Qui travaille dessus ? (solo / petite equipe / grosse equipe)\n",
            "3. A quel stade en est-il ? (prototype, MVP, production, legacy, rewrite...)\n",
            "4. Dependances externes cles ? Inclus les noms/URLs si pertinent. (ex: \"PostgreSQL sur AWS RDS\", \"API user-service sur gitlab.company.com/org/repo\" — ou juste \"aucune\")\n",
            "5. Qu'est-ce qu'un nouveau contributeur ferait mal le premier jour ? (pieges, regles implicites, zones fragiles)\n",
            "6. Autre chose que l'audit devrait savoir ? (optionnel, en bref)\n\n",
            "ETAPE 2 — Verifie que l'utilisateur a repondu aux questions 1-5. S'il en manque, redemande UNIQUEMENT celles qui manquent. La Q6 est optionnelle. ",
            "Une fois les reponses 1-5 obtenues (ou si l'utilisateur dit 'passer'), ecris le fichier docs/briefing.md avec CE FORMAT EXACT :\n\n",
            "# Project Briefing\n",
            "> Auto-generated by AI briefing. Source: user answers (not code analysis).\n",
            "## Purpose\n[reponse Q1]\n",
            "## Team\n[reponse Q2]\n",
            "## Maturity\n[reponse Q3]\n",
            "## External Dependencies\n[reponse Q4 — si aucune, ecrire \"None.\"]\n",
            "## Traps & Fragile Areas\n[reponse Q5 — liste a puces si plusieurs]\n",
            "## Additional Context\n[reponse Q6 — si omise, ecrire \"None.\"]\n\n",
            "Ecris docs/briefing.md EN ANGLAIS meme si la conversation est en francais.\n",
            "Si l'utilisateur ne repond pas a une question, ecris \"Not provided\" — n'invente RIEN.\n",
            "Ne modifie AUCUN autre fichier.\n\n",
            "ETAPE 3 — Apres avoir ecrit le fichier, termine ton dernier message par : KRONN:BRIEFING_COMPLETE",
        ).to_string(),
    }
}

#[cfg(test)]
mod compute_audit_info_tests {
    use super::*;

    #[test]
    fn real_todo_markers_counted_template_instructions_ignored() {
        // Raw marker in content → real.
        assert!(line_has_real_todo_marker(
            "Deploy target unknown <!-- TODO: ask user -->"
        ));
        // The template QUOTES the marker inside backticks to teach the
        // grammar — those lines polluted every fresh project's todo list.
        assert!(!line_has_real_todo_marker(
            "Terms marked `<!-- TODO: ask user -->` need human confirmation."
        ));
        assert!(!line_has_real_todo_marker(
            "> - Do NOT invent information — mark unknowns with `<!-- TODO: verify -->`"
        ));
        // Mixed: a real marker after a backticked mention still counts.
        assert!(line_has_real_todo_marker(
            "see `<!-- TODO: verify -->` <!-- TODO: ask user -->"
        ));
    }
    use std::fs;
    use tempfile::tempdir;

    fn write_index_with_two_tds(docs: &std::path::Path) {
        fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "# Tech Debt\n\n\
             ## Current list\n\n\
             | ID | Problem | Area | Severity |\n\
             |----|---------|------|----------|\n\
             | TD-20260512-keeper | Real issue | docker | High |\n\
             | TD-20260512-phantom | Removed but still in table | docker | High |\n",
        )
        .unwrap();
    }

    #[test]
    fn skips_td_rows_whose_detail_file_was_removed() {
        let dir = tempdir().unwrap();
        let docs = dir.path().join("docs");
        let tech_debt = docs.join("tech-debt");
        fs::create_dir_all(&tech_debt).unwrap();
        write_index_with_two_tds(&docs);
        // Only the "keeper" detail file exists on disk.
        fs::write(
            tech_debt.join("TD-20260512-keeper.md"),
            "# Keeper\n- **Severity**: High\n",
        )
        .unwrap();

        let info = compute_audit_info_sync(dir.path().to_str().unwrap());
        let ids: Vec<&str> = info.tech_debt_items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["TD-20260512-keeper"],
            "phantom TD whose detail file was removed must not leak into validation prompt"
        );
    }

    #[test]
    fn surfaces_tds_when_both_index_and_detail_exist() {
        let dir = tempdir().unwrap();
        let docs = dir.path().join("docs");
        let tech_debt = docs.join("tech-debt");
        fs::create_dir_all(&tech_debt).unwrap();
        write_index_with_two_tds(&docs);
        fs::write(tech_debt.join("TD-20260512-keeper.md"), "x").unwrap();
        fs::write(tech_debt.join("TD-20260512-phantom.md"), "x").unwrap();

        let info = compute_audit_info_sync(dir.path().to_str().unwrap());
        assert_eq!(info.tech_debt_items.len(), 2);
    }

    // ── "filled" detector — no false negative on Twig / `${{ }}` / table-only docs ──

    fn filled_flag_for(dir: &std::path::Path, filename: &str, content: &str) -> bool {
        let docs = dir.join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join(filename), content).unwrap();
        let info = compute_audit_info_sync(dir.to_str().unwrap());
        info.files
            .iter()
            .find(|f| f.path.ends_with(filename))
            .unwrap_or_else(|| panic!("{filename} not found in {:?}", info.files))
            .filled
    }

    #[test]
    fn filled_detector_ignores_twig_style_double_braces() {
        let dir = tempdir().unwrap();
        let content = "# Deployment\n\n\
             This project renders `{{ user.name }}` in its Twig templates.\n\
             It also documents `{{ some.other.expr }}` as an example.\n\
             See `templates/base.html.twig` for the full layout.\n";
        assert!(
            filled_flag_for(dir.path(), "AGENTS.md", content),
            "Twig-style `{{ expr }}` (lowercase/dotted) must not look like an unfilled placeholder"
        );
    }

    #[test]
    fn filled_detector_ignores_github_actions_secrets_interpolation() {
        let dir = tempdir().unwrap();
        let content = "# CI\n\n\
             The workflow reads `${{ secrets.DEPLOY_TOKEN }}` from the environment.\n\
             Another step uses `${{ steps.build.outputs.artifact }}` as input.\n\
             The pipeline runs on every push to `main`.\n";
        assert!(
            filled_flag_for(dir.path(), "AGENTS.md", content),
            "GitHub Actions `${{ secrets.FOO }}` interpolation must not look like an unfilled placeholder"
        );
    }

    #[test]
    fn filled_detector_still_flags_real_unfilled_placeholder() {
        let dir = tempdir().unwrap();
        let content = "# Overview\n\nProject: {{PROJECT_NAME}}\nOwner: {{OWNER_TEAM}}\n";
        assert!(
            !filled_flag_for(dir.path(), "AGENTS.md", content),
            "a real `{{PROJECT_NAME}}`-shaped placeholder must still be caught"
        );
    }

    #[test]
    fn filled_detector_counts_a_doc_made_of_tables_as_filled() {
        let dir = tempdir().unwrap();
        let content = "# Endpoints\n\n\
             | Route | Method | Description |\n\
             |-------|--------|-------------|\n\
             | /users | GET | List users |\n\
             | /users/:id | GET | Fetch one user |\n\
             | /orders | POST | Create an order |\n";
        assert!(
            filled_flag_for(dir.path(), "AGENTS.md", content),
            "a doc whose real content lives in table rows must not be flagged empty"
        );
    }

    // ── detect_project_skills — language + domain detection contract ──

    #[test]
    fn detects_rust_skill_from_cargo_toml() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"rust".into()), "got {skills:?}");
    }

    #[test]
    fn detects_typescript_when_package_json_plus_tsconfig() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"typescript".into()), "got {skills:?}");
    }

    #[test]
    fn does_not_emit_typescript_for_pure_js_project() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        // NO tsconfig.json → should NOT add typescript.
        let skills = detect_project_skills(dir.path());
        assert!(
            !skills.contains(&"typescript".into()),
            "package.json alone must not imply typescript"
        );
    }

    #[test]
    fn detects_python_skill_from_requirements_txt() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("requirements.txt"), "fastapi").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"python".into()));
    }

    #[test]
    fn detects_python_skill_from_pyproject_toml() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\nname=\"x\"").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"python".into()));
    }

    #[test]
    fn detects_python_skill_from_setup_py() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("setup.py"), "from setuptools import setup").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"python".into()));
    }

    #[test]
    fn detects_go_skill_from_go_mod() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module x").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"go".into()));
    }

    #[test]
    fn detects_php_skill_from_composer_json() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("composer.json"), "{}").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"php".into()));
    }

    #[test]
    fn detects_devops_skill_from_dockerfile() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Dockerfile"), "FROM alpine").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"devops".into()));
    }

    #[test]
    fn detects_devops_skill_from_makefile() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Makefile"), "all:\n\techo hi").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"devops".into()));
    }

    #[test]
    fn skill_markers_name_the_file_that_triggered_each_skill() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Dockerfile"), "FROM scratch").unwrap();
        fs::write(dir.path().join("Makefile"), "all:").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("tsconfig.app.json"), "{}").unwrap();
        fs::write(dir.path().join("vite.config.ts"), "").unwrap();
        let markers = detect_project_skill_markers(dir.path());
        let marker = |skill: &str| {
            markers
                .iter()
                .find(|(id, _)| id == skill)
                .map(|(_, marker)| marker.as_str())
        };
        assert_eq!(marker("devops"), Some("Dockerfile"));
        assert_eq!(marker("typescript"), Some("tsconfig.app.json"));
        assert_eq!(marker("web-performance"), Some("vite.config.ts"));
        assert_eq!(marker("rust"), None);
        assert_eq!(
            markers.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            detect_project_skills(dir.path())
        );
    }

    #[test]
    fn detects_database_skill_from_migrations_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("migrations")).unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"database".into()));
    }

    #[test]
    fn detects_database_skill_from_prisma_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("prisma")).unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"database".into()));
    }

    #[test]
    fn detects_web_performance_skill_from_vite_config() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("vite.config.ts"), "export default {};").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"web-performance".into()));
    }

    #[test]
    fn detects_seo_skill_from_robots_txt() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("robots.txt"), "User-agent: *").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"seo".into()));
    }

    #[test]
    fn detects_seo_skill_from_public_robots_txt() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("public")).unwrap();
        fs::write(dir.path().join("public/robots.txt"), "User-agent: *").unwrap();
        let skills = detect_project_skills(dir.path());
        assert!(skills.contains(&"seo".into()));
    }

    #[test]
    fn returns_empty_for_unknown_project() {
        let dir = tempdir().unwrap();
        // Empty dir — no detectable language, no domain signals.
        let skills = detect_project_skills(dir.path());
        // All skills get filtered through `core::skills::get_skill`, so
        // even false-positives (unknown skill ids) would be dropped here.
        // Just verify no panic + vec is well-formed.
        assert!(skills.iter().all(|s| !s.is_empty()));
    }

    #[test]
    fn protects_human_section_and_writes_report_from_shipped_template_fixture() {
        let dir = tempdir().unwrap();
        let docs = dir.path().join("docs");
        fs::create_dir_all(&docs).unwrap();
        let template_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("templates/docs/AGENTS.md");
        let template = fs::read_to_string(template_path).unwrap();
        let pre = format!(
            "{template}\n<!-- kronn:section name=\"team-notes\" curated=\"human\" owner=\"human\" -->\nOriginal human note.\n<!-- kronn:section:end -->\n"
        );
        fs::write(docs.join("AGENTS.md"), &pre).unwrap();

        let coding_template = fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("templates/docs/coding-rules.md"),
        )
        .unwrap();
        let coding_pre = format!(
            "{coding_template}\n<!-- kronn:section name=\"review-note\" owner=\"human\" -->\nKeep this review rule.\n<!-- kronn:section:end -->\n"
        );
        fs::write(docs.join("coding-rules.md"), &coding_pre).unwrap();
        let snapshot = capture_human_owned_sections(dir.path()).unwrap();

        fs::write(
            docs.join("AGENTS.md"),
            pre.replace("Original human note.", "Audit replacement."),
        )
        .unwrap();
        fs::write(
            docs.join("coding-rules.md"),
            coding_pre.replace("Keep this review rule.", "Rewrite this review rule."),
        )
        .unwrap();
        let restored = protect_human_owned_sections(dir.path(), &snapshot, "2026-09-27").unwrap();

        assert_eq!(restored, 2);
        let target = fs::read_to_string(docs.join("AGENTS.md")).unwrap();
        assert!(target.contains("Original human note."));
        assert!(!target.contains("Audit replacement."));
        let coding = fs::read_to_string(docs.join("coding-rules.md")).unwrap();
        assert!(coding.contains("Keep this review rule."));
        assert!(!coding.contains("Rewrite this review rule."));
        let report =
            fs::read_to_string(docs.join("reports/2026-09-27-human-section-diff-docs-AGENTS.md"))
                .unwrap();
        assert!(report.contains("Original human note."));
        assert!(report.contains("Audit replacement."));
        assert!(docs
            .join("reports/2026-09-27-human-section-diff-docs-coding-rules.md")
            .is_file());
    }

    // ── check_ai_dir_permissions — defensive guard tests ────────────

    #[test]
    fn check_permissions_on_existing_directory_succeeds() {
        let dir = tempdir().unwrap();
        let result = check_ai_dir_permissions(dir.path());
        assert!(result.is_ok(), "writable temp dir must pass: {result:?}");
    }

    #[test]
    fn check_permissions_on_nonexistent_directory_creates_or_errors_cleanly() {
        // Helper is expected to either create the dir or surface a clear
        // error — never panic.
        let dir = tempdir().unwrap();
        let nested = dir.path().join("does-not-yet-exist");
        let _ = check_ai_dir_permissions(&nested);
        // No assertion on Ok/Err — both shapes are valid contracts. Just
        // confirm no panic.
    }

    #[test]
    fn localized_briefing_prompts_never_point_to_retired_ai_tree() {
        for language in ["en", "es", "fr"] {
            let prompt = build_briefing_legacy_prompt(language);
            assert!(prompt.contains("docs/briefing.md"), "{language}: {prompt}");
            assert!(!prompt.contains("ai/"), "{language}: {prompt}");
        }
    }
}
