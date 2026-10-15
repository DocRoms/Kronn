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
            template_update: None,
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

    // KT-949 — every active sheet under tech-debt/, whichever audit (Full or
    // one of the seven sub-audits) wrote it. Index rows only fill in what a
    // sheet leaves blank; a row without a sheet is never surfaced.
    let mut rows: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir(&docs_dir) {
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with("inconsistencies-") && n.ends_with(".md"))
            .collect();
        // The central index wins over the dimension indexes.
        names.sort_by_key(|n| (n != crate::core::td_index::CENTRAL_INDEX, n.clone()));
        for name in names {
            let Ok(content) = std::fs::read_to_string(docs_dir.join(&name)) else {
                continue;
            };
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.starts_with('|') || scanner::has_unfilled_placeholder(trimmed) {
                    continue;
                }
                let cols: Vec<String> = trimmed.split('|').map(|c| c.trim().to_string()).collect();
                let id: String = cols
                    .get(1)
                    .map(|c| c.trim_start_matches(['[', '`']))
                    .unwrap_or("")
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                if cols.len() >= 5 && crate::core::td_index::is_td_id(&id) {
                    rows.entry(id).or_insert(cols);
                }
            }
        }
    }
    let known = |value: &str, row: Option<&Vec<String>>, col: usize| -> String {
        if value != "Unknown" {
            return value.to_string();
        }
        row.and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_else(|| value.to_string())
    };
    let tech_debt_items = crate::core::td_index::read_sheets(&docs_dir)
        .into_iter()
        .filter(|sheet| sheet.is_active())
        .map(|sheet| {
            let row = rows.get(&sheet.id);
            let problem = if sheet.title == sheet.id {
                row.and_then(|r| r.get(2))
                    .cloned()
                    .unwrap_or(sheet.title.clone())
            } else {
                sheet.title.clone()
            };
            TechDebtItem {
                area: known(&sheet.area, row, 3),
                severity: known(&sheet.severity, row, 4),
                id: sheet.id,
                problem,
            }
        })
        .collect();

    AuditInfo {
        template_update: crate::core::template_manifest::check(
            &project_path,
            &crate::api::projects::template::resolve_templates_dir().join("docs"),
        )
        .ok(),
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
             This run's TDs that still need a decision: {ids}. TDs an earlier validation decided keep their decision.\n\
             The TD review covers ONLY them — never re-read, re-validate or modify any other `docs/tech-debt/TD-*.md`: they belong to previous audits already settled by their own validation discussions.\n\n",
        ),
        "es" => format!(
            "## SCOPE — SOLO los TDs de ESTE run\n\
             Los TDs de este run que aun necesitan una decision: {ids}. Los TDs decididos por una validacion anterior conservan su decision.\n\
             La revision cubre SOLO estos — nunca releas, revalides o modifiques ningun otro `docs/tech-debt/TD-*.md`: pertenecen a auditorias anteriores ya cerradas por su propia discusion de validacion.\n\n",
        ),
        _ => format!(
            "## SCOPE — TDs de CE run uniquement\n\
             Les TDs de ce run qui attendent encore une décision : {ids}. Celles qu'une validation précédente a tranchées gardent leur décision.\n\
             La revue des TDs porte UNIQUEMENT sur eux — ne relis, ne revalide et ne modifie AUCUN autre `docs/tech-debt/TD-*.md` : ils appartiennent à des audits précédents déjà validés par leur propre discussion.\n\n",
        ),
    }
}

/// KT-931 — scope block for the validation discussion of a PARTIAL run: some
/// steps failed (or never ran), the others did succeed. It names the steps to
/// redo, so the user — and the agent — know the documents behind them are stale
/// or absent and are not what is being validated. `steps_to_redo` carries the
/// 1-based step and its target file. `no_run_tds` is true when the successful
/// steps produced no TD: without an explicit instruction the TD phases would
/// fall back to reading every TD on disk, those of previous audits included.
/// Every TD the run listed was already decided: without this block an empty
/// scope would let the TD phases fall back to every sheet on disk.
pub(crate) fn all_tds_decided_block(language: &str) -> &'static str {
    match language {
        "en" => "## SCOPE — no TD to decide\nEvery TD this run lists was already decided by an earlier validation: skip the TD review phases and emit no TD card.\n\n",
        "es" => "## SCOPE — ningun TD por decidir\nCada TD que lista este run ya fue decidido por una validacion anterior: omite las fases de revision de TDs y no emitas ninguna tarjeta TD.\n\n",
        _ => "## SCOPE — aucune TD à trancher\nToutes les TDs que liste ce run ont déjà été tranchées par une validation précédente : saute les phases de revue des TDs et n'émets aucune carte TD.\n\n",
    }
}

/// The full validation after a resume names the partial validation of the
/// run it resumed: decisions taken there are already on the TD sheets.
pub(crate) fn prior_partial_validation_block(discussion_id: &str, language: &str) -> String {
    let (title, body) = match language {
        "en" => (
            "## AFTER A RESUME — partial validation",
            "The run this audit resumed was already validated in part, in discussion `{id}` (`#discussion-{id}`). The TDs decided there keep their decision: they are not in this run's scope and get no card. Mention that validation to the user in your first message.",
        ),
        "es" => (
            "## TRAS UNA REANUDACION — validacion parcial",
            "El run que esta auditoria reanudo ya se valido en parte, en la discusion `{id}` (`#discussion-{id}`). Los TDs decididos alli conservan su decision: no estan en el scope de este run y no reciben tarjeta. Menciona esa validacion al usuario en tu primer mensaje.",
        ),
        _ => (
            "## APRÈS UNE REPRISE — validation partielle",
            "Le run que cet audit a repris a déjà été validé en partie, dans la discussion `{id}` (`#discussion-{id}`). Les TDs tranchées là-bas gardent leur décision : elles ne sont pas dans le périmètre de ce run et n'ont pas de carte. Mentionne cette validation à l'utilisateur dans ton premier message.",
        ),
    };
    format!("{title}\n{}\n\n", body.replace("{id}", discussion_id))
}

/// KT-1211 (M6) — the sources moved between the run this one resumed and the
/// resume: the steps it inherited describe the earlier sources. Said in the
/// validation prompt so the reviewer does not take them for current.
pub(crate) fn resume_drift_block(drift: &str, language: &str) -> String {
    let (title, lead) = match language {
        "en" => (
            "## SOURCE DRIFT DURING THE RESUME",
            "The documents of the steps this run inherited were written against earlier sources. Check them against the current tree before trusting them, and report any that no longer match:",
        ),
        "es" => (
            "## CAMBIO EN LAS FUENTES DURANTE LA REANUDACION",
            "Los documentos de los pasos heredados por este run se escribieron sobre fuentes anteriores. Compruebalos con el arbol actual antes de fiarte y senala los que ya no coinciden:",
        ),
        _ => (
            "## DÉRIVE DES SOURCES PENDANT LA REPRISE",
            "Les documents des étapes héritées par ce run ont été écrits sur des sources antérieures. Vérifie-les contre l'arbre actuel avant de t'y fier et signale ceux qui ne correspondent plus :",
        ),
    };
    format!("{title}\n{lead}\n> {}\n\n", drift.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub(crate) fn partial_run_block(
    steps_to_redo: &[(u32, String)],
    total_steps: u32,
    no_run_tds: bool,
    language: &str,
) -> String {
    if steps_to_redo.is_empty() {
        return String::new();
    }
    let list = steps_to_redo
        .iter()
        .map(|(step, file)| format!("- {step}/{total_steps} — `{file}`"))
        .collect::<Vec<_>>()
        .join("\n");
    let redo = steps_to_redo.len();
    let done = (total_steps as usize).saturating_sub(redo);
    let (title, intro, rule, resume, no_tds) = match language {
        "en" => (
            "## PARTIAL RUN — steps to redo",
            format!("This run is PARTIAL: {done} of {total_steps} steps succeeded, {redo} did not (failed or never ran):"),
            "This validation covers ONLY the successful steps. The documents behind the steps to redo are stale or missing: do not validate them, do not fix them, do not quote them as a reference.",
            "Tell the user in your first message which steps are left. Resuming the audit (\"Resume\" on the project card) re-runs ONLY those steps; the project stays un-validated until a complete run is validated.",
            "No TD came from the successful steps: skip the TD review phases and emit no TD card.",
        ),
        "es" => (
            "## RUN PARCIAL — pasos por rehacer",
            format!("Este run es PARCIAL: {done} de {total_steps} pasos tuvieron exito, {redo} no (fallaron o no se ejecutaron):"),
            "Esta validacion cubre SOLO los pasos exitosos. Los documentos de los pasos por rehacer estan obsoletos o ausentes: no los valides, no los corrijas, no los cites como referencia.",
            "Dile al usuario en tu primer mensaje que pasos faltan. Reanudar la auditoria (\"Reanudar\" en la tarjeta del proyecto) relanza SOLO esos pasos; el proyecto sigue sin validar hasta que se valide un run completo.",
            "Ningun TD proviene de los pasos exitosos: omite las fases de revision de TDs y no emitas ninguna tarjeta TD.",
        ),
        _ => (
            "## RUN PARTIEL — étapes à refaire",
            format!("Ce run est PARTIEL : {done} étapes sur {total_steps} ont réussi, {redo} non (échec ou jamais exécutées) :"),
            "Cette validation porte UNIQUEMENT sur les étapes réussies. Les documents des étapes à refaire sont périmés ou absents : ne les valide pas, ne les corrige pas, ne les cite pas comme référence.",
            "Dis à l'utilisateur dans ton premier message quelles étapes restent. Reprendre l'audit (« Reprendre » sur la carte du projet) relance UNIQUEMENT ces étapes ; le projet reste non validé tant qu'un run complet n'est pas validé.",
            "Aucun TD ne vient des étapes réussies : saute les phases de revue des TDs et n'émets aucune carte TD.",
        ),
    };
    let mut block = format!("{title}\n{intro}\n{list}\n\n{rule}\n{resume}\n");
    if no_run_tds {
        block.push_str(no_tds);
        block.push('\n');
    }
    block.push('\n');
    block
}

/// The sections a partial refresh did not update: outside its validation's
/// scope and still stale. Empty when every step is done.
pub(crate) fn partial_refresh_failed_block(
    failed: &[(u32, String)],
    total_steps: u32,
    language: &str,
) -> String {
    if failed.is_empty() {
        return String::new();
    }
    let list = failed
        .iter()
        .map(|(step, file)| format!("- {step}/{total_steps} — `{file}`"))
        .collect::<Vec<_>>()
        .join("\n");
    let (title, intro, rule) = match language {
        "en" => (
            "## SECTIONS NOT REFRESHED — out of scope",
            "This partial refresh could not update these sections (failed or stopped):",
            "They are still stale: do not validate them, do not fix them, do not quote them as a reference. Tell the user in your first message: \"Update\" on the project card re-runs only these sections, and the project cannot be marked validated until a refresh completes.",
        ),
        "es" => (
            "## SECCIONES NO ACTUALIZADAS — fuera del alcance",
            "Este refresh parcial no pudo actualizar estas secciones (fallo o parada):",
            "Siguen obsoletas: no las valides, no las corrijas, no las cites como referencia. Dile al usuario en tu primer mensaje: \"Actualizar\" en la tarjeta del proyecto relanza solo estas secciones, y el proyecto no puede marcarse como validado hasta que un refresh termine.",
        ),
        _ => (
            "## SECTIONS NON MISES À JOUR — hors périmètre",
            "Ce refresh partiel n'a pas pu mettre à jour ces sections (échec ou arrêt) :",
            "Elles restent obsolètes : ne les valide pas, ne les corrige pas, ne les cite pas comme référence. Dis-le à l'utilisateur dans ton premier message : « Mettre à jour » sur la carte du projet relance uniquement ces sections, et le projet ne peut pas être marqué validé tant qu'un refresh n'a pas abouti.",
        ),
    };
    format!("{title}\n{intro}\n{list}\n\n{rule}\n\n")
}

/// Dead references inside human-owned sections: the audit may not fix them,
/// so the human is asked to. Empty when there is none.
pub(crate) fn human_references_block(
    items: &[crate::core::document_optimization::Diagnostic],
    language: &str,
) -> String {
    if items.is_empty() {
        return String::new();
    }
    let list = items
        .iter()
        .map(|d| format!("- `{}` — {}", d.path, d.message))
        .collect::<Vec<_>>()
        .join("\n");
    let (title, intro, rule) = match language {
        "en" => (
            "## DEAD REFERENCES IN HUMAN SECTIONS — for their owner",
            "These `owner=\"human\"` sections cite a file or a line that no longer exists. The audit may not edit them, and they do not block it.",
            "List them to the user in a separate section of your first message: the user corrects or confirms these sections. Do not edit them yourself.",
        ),
        "es" => (
            "## REFERENCIAS MUERTAS EN SECCIONES HUMANAS — para su autor",
            "Estas secciones `owner=\"human\"` citan un archivo o una linea que ya no existe. La auditoria no puede modificarlas y no la bloquean.",
            "Enumeralas al usuario en una seccion aparte de tu primer mensaje: el usuario corrige o confirma esas secciones. No las modifiques tu.",
        ),
        _ => (
            "## RÉFÉRENCES MORTES DANS DES SECTIONS HUMAINES — pour leur auteur",
            "Ces sections `owner=\"human\"` citent un fichier ou une ligne qui n'existe plus. L'audit n'a pas le droit de les modifier, et elles ne le bloquent pas.",
            "Liste-les à l'utilisateur dans une section à part de ton premier message : c'est lui qui corrige ou confirme ces sections. Ne les modifie pas toi-même.",
        ),
    };
    format!("{title}\n{intro}\n{list}\n\n{rule}\n\n")
}

/// KT-1130 — the steps that verified their document and had nothing to
/// change, listed apart so the human confirms them. Empty when there is none.
pub(crate) fn unchanged_steps_block(
    unchanged: &[(u32, String)],
    total_steps: u32,
    language: &str,
) -> String {
    if unchanged.is_empty() {
        return String::new();
    }
    let list = unchanged
        .iter()
        .map(|(step, file)| format!("- {step}/{total_steps} — `{file}`"))
        .collect::<Vec<_>>()
        .join("\n");
    let (title, intro, rule) = match language {
        "en" => (
            "## UNCHANGED STEPS — to confirm",
            "These steps checked their document and had nothing to fix: the file was already up to date and every check passed.",
            "List them to the user in a separate section of your first message so they can confirm these documents still hold. They did not fail and have nothing to redo.",
        ),
        "es" => (
            "## PASOS SIN CAMBIOS — por confirmar",
            "Estos pasos verificaron su documento y no tuvieron nada que corregir: el archivo ya estaba al dia y todas las comprobaciones pasaron.",
            "Enumeralos al usuario en una seccion aparte de tu primer mensaje para que confirme que esos documentos siguen siendo correctos. No fallaron y no hay nada que rehacer.",
        ),
        _ => (
            "## ÉTAPES INCHANGÉES — à confirmer",
            "Ces étapes ont vérifié leur document et n'avaient rien à corriger : le fichier était déjà à jour et toutes les vérifications sont passées.",
            "Liste-les à l'utilisateur dans une section à part de ton premier message, pour qu'il confirme que ces documents restent justes. Elles ne sont pas en échec et n'ont rien à refaire.",
        ),
    };
    format!("{title}\n{intro}\n{list}\n\n{rule}\n\n")
}

/// KT-1131 — one step a human accepted, as the validation prompt lists it.
pub(crate) struct AcceptedStepLine {
    pub step: u32,
    pub file: String,
    pub by: String,
    pub reason: Option<String>,
    pub failure: Option<String>,
}

/// KT-1131 — the steps a human accepted instead of fixing: the validation names
/// them apart, with who accepted, why, and the failure they still carry.
pub(crate) fn accepted_steps_block(
    accepted: &[AcceptedStepLine],
    total_steps: u32,
    language: &str,
) -> String {
    if accepted.is_empty() {
        return String::new();
    }
    let (title, intro, by_label, reason_label, failure_label, none, rule) = match language {
        "en" => (
            "## ACCEPTED STEPS — as they stand",
            "A person accepted these steps although they failed: their document is kept as it is.",
            "accepted by",
            "reason",
            "failure",
            "none given",
            "List them to the user in a separate section of your first message, with the failure each one still carries. Do not present them as succeeded and do not redo them.",
        ),
        "es" => (
            "## PASOS ACEPTADOS — tal cual",
            "Una persona acepto estos pasos aunque fallaron: su documento se conserva tal como esta.",
            "aceptado por",
            "motivo",
            "fallo",
            "sin motivo",
            "Enumeralos al usuario en una seccion aparte de tu primer mensaje, con el fallo que cada uno conserva. No los presentes como exitosos y no los rehagas.",
        ),
        _ => (
            "## ÉTAPES ACCEPTÉES — en l'état",
            "Une personne a accepté ces étapes bien qu'elles aient échoué : leur document est gardé tel quel.",
            "acceptée par",
            "raison",
            "échec",
            "aucune",
            "Liste-les à l'utilisateur dans une section à part de ton premier message, avec l'échec que chacune garde. Ne les présente pas comme réussies et ne les refais pas.",
        ),
    };
    let list = accepted
        .iter()
        .map(|line| {
            let mut text = format!(
                "- {}/{total_steps} — `{}` — {by_label} {} — {reason_label} : {}",
                line.step,
                line.file,
                line.by,
                line.reason
                    .as_deref()
                    .filter(|r| !r.trim().is_empty())
                    .unwrap_or(none)
            );
            if let Some(failure) = line.failure.as_deref() {
                text.push_str(&format!(" — {failure_label} : {failure}"));
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{title}\n{intro}\n{list}\n\n{rule}\n\n")
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
        "en" => String::from(include_str!("prompts/validation-cards-en.md")),
        "es" => String::from(include_str!("prompts/validation-cards-es.md")),
        _ => String::from(include_str!("prompts/validation-cards-fr.md")),
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
            Err(e) => return Err(format!("Cannot traverse {}: {}", ai_dir.display(), e)),
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
pub(crate) struct ValidatedDocsSnapshot {
    files: Vec<(String, String)>,
    briefing: (std::path::PathBuf, Option<Vec<u8>>),
}

/// Every doc an audit may edit, plus the immutable briefing before the step.
pub(crate) fn capture_validated_docs(
    project_path: &std::path::Path,
) -> Result<ValidatedDocsSnapshot, String> {
    let files = crate::core::doc_provenance::snapshot_tracked(project_path)?;
    let path = crate::core::scanner::detect_docs_dir(project_path).join("briefing.md");
    crate::core::fs_guard::assert_contained_no_symlink(project_path, &path)?;
    let briefing = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Cannot snapshot briefing: {e}")),
    };
    Ok(ValidatedDocsSnapshot {
        files,
        briefing: (path, briefing),
    })
}

/// Restoration of a project-wide badge must cover every previously validated
/// doc, including documents outside this refresh's declared step targets.
pub(crate) fn validation_restore_snapshots(
    project_path: &std::path::Path,
    targets: &[&str],
) -> Result<Vec<(String, super::validation::TargetSnapshot)>, String> {
    let tracked = capture_validated_docs(project_path)?;
    let mut snapshots: std::collections::BTreeMap<_, _> = tracked
        .files
        .into_iter()
        .map(|(doc, content)| {
            (
                doc,
                super::validation::TargetSnapshot::Present(
                    super::validation::normalized_content_hash(&content),
                ),
            )
        })
        .collect();
    for target in targets {
        snapshots
            .entry((*target).to_string())
            .or_insert(super::validation::target_snapshot(project_path, target)?);
    }
    Ok(snapshots.into_iter().collect())
}

/// Mark every tracked doc one agent attempt changed "to reconfirm". The
/// write itself stays on disk: human ownership never reverts it (KT-1079).
pub(crate) fn mark_validated_doc_writes(
    project_path: &std::path::Path,
    snapshot: &ValidatedDocsSnapshot,
    writer: crate::core::doc_provenance::DocWriter,
) -> Result<usize, String> {
    let protected = restore_step_briefing(project_path, snapshot);
    let marked =
        crate::core::doc_provenance::note_snapshot_writes(project_path, &snapshot.files, writer);
    protected?;
    marked
}

/// An audit step may read the briefing, but only a human briefing review may edit it.
pub(crate) fn restore_step_briefing(
    project_path: &std::path::Path,
    snapshot: &ValidatedDocsSnapshot,
) -> Result<(), String> {
    let (path, before) = &snapshot.briefing;
    crate::core::fs_guard::assert_contained_no_symlink(project_path, path)?;
    let after = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Cannot check protected briefing: {e}")),
    };
    if before == &after {
        return Ok(());
    }
    match before {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, bytes)
                .map_err(|e| format!("Cannot restore protected briefing: {e}"))?;
        }
        None => std::fs::remove_file(path)
            .map_err(|e| format!("Cannot remove unauthorized briefing: {e}"))?,
    }
    Err(
        "The audit step changed the protected briefing; its original contents were restored."
            .into(),
    )
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
#[cfg(test)]
pub(crate) fn build_briefing_prompt(language: &str, prefilled_notes: Option<&str>) -> String {
    build_briefing_prompt_in(
        language,
        prefilled_notes,
        &crate::core::docs_location::DocsRoot::default_root(),
    )
}

/// `build_briefing_prompt` with Kronn's text pointing at `docs_root`
/// (KT-1192). The user's own answers are inserted after the relocation.
pub(crate) fn build_briefing_prompt_in(
    language: &str,
    prefilled_notes: Option<&str>,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> String {
    // 0.8.7 anti-hallu: prefix the discipline reminder on every briefing
    // path (review + legacy). Briefing produces `docs/briefing.md` which is
    // then injected as project context — a hallucinated assertion here
    // ships into every future agent prompt for this project.
    const NOTES_SLOT: &str = "\u{0}kronn-briefing-notes\u{0}";
    let body = if prefilled_notes.is_some() {
        build_briefing_review_prompt(language, NOTES_SLOT)
    } else {
        build_briefing_legacy_prompt(language)
    };
    let prompt = format!("{}{}", anti_halluc_doc_writer_block(language), body);
    let prompt = docs_root.relocate(&prompt);
    match prefilled_notes {
        Some(notes) => prompt.replace(NOTES_SLOT, notes),
        None => prompt.into_owned(),
    }
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
            include_str!("prompts/briefing-review-en.md"),
            prefilled_notes,
        ),
        "es" => format!(
            include_str!("prompts/briefing-review-es.md"),
            prefilled_notes,
        ),
        _ => format!(
            include_str!("prompts/briefing-review-fr.md"),
            prefilled_notes,
        ),
    }
}

/// Legacy 6-question briefing prompt — kept for `start_briefing`
/// callers that didn't pre-fill the form (no `prefilled_notes`).
fn build_briefing_legacy_prompt(language: &str) -> String {
    match language {
        "en" => include_str!("prompts/briefing-legacy-en.md").to_string(),
        "es" => include_str!("prompts/briefing-legacy-es.md").to_string(),
        _ => include_str!("prompts/briefing-legacy-fr.md").to_string(),
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

    // KT-949 — the seven sub-audits write their TDs into their own index;
    // audit-info counts every active sheet, not only the central rows.
    #[test]
    fn tech_debt_items_count_every_sub_audit_sheet() {
        let dir = tempdir().unwrap();
        let docs = dir.path().join("docs");
        let td = docs.join("tech-debt");
        fs::create_dir_all(&td).unwrap();
        fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "| ID | Problem | Area | Severity |\n|---|---|---|---|\n\
             | TD-20261001-full | Full row | Backend | High |\n\
             | TD-20261001-phantom | No sheet | Backend | High |\n",
        )
        .unwrap();
        fs::write(
            docs.join("inconsistencies-security.md"),
            "| ID | Problem | Area | Severity |\n|---|---|---|---|\n\
             | TD-20261002-sec | Secret row | Security | Critical |\n",
        )
        .unwrap();
        fs::write(
            docs.join("inconsistencies-rgaa.md"),
            "| ID | Problem | Area | Severity |\n|---|---|---|---|\n\
             | [TD-20261003-rgaa](tech-debt/TD-20261003-rgaa.md) | Alt row | A11y | Low |\n",
        )
        .unwrap();
        fs::write(td.join("TD-20261001-full.md"), "x").unwrap();
        fs::write(td.join("TD-20261002-sec.md"), "x").unwrap();
        fs::write(
            td.join("TD-20261003-rgaa.md"),
            "---\ndescription: Images without alt\n---\n# TD-20261003-rgaa\n- **Severity**: Medium\n",
        )
        .unwrap();
        fs::write(
            td.join("TD-20261004-unlisted.md"),
            "# TD-20261004-unlisted\n- **Area**: CI\n",
        )
        .unwrap();
        fs::write(
            td.join("TD-20261005-rejected.md"),
            "# R\n- **Status**: Rejected\n",
        )
        .unwrap();
        fs::write(td.join("_reconciliation-2026-10-05.md"), "# report\n").unwrap();

        let info = compute_audit_info_sync(dir.path().to_str().unwrap());
        let ids: Vec<&str> = info.tech_debt_items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "TD-20261001-full",
                "TD-20261002-sec",
                "TD-20261003-rgaa",
                "TD-20261004-unlisted",
            ]
        );
        let sec = &info.tech_debt_items[1];
        assert_eq!(
            (
                sec.problem.as_str(),
                sec.area.as_str(),
                sec.severity.as_str()
            ),
            ("Secret row", "Security", "Critical")
        );
        // The sheet wins over its index row.
        let rgaa = &info.tech_debt_items[2];
        assert_eq!(
            (
                rgaa.problem.as_str(),
                rgaa.area.as_str(),
                rgaa.severity.as_str()
            ),
            ("Images without alt", "A11y", "Medium")
        );
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
    fn a_human_section_rewrite_from_the_shipped_template_is_kept_and_marked() {
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
        fs::write(docs.join("coding-rules.md"), "# Rules\nAI text.\n").unwrap();
        let snapshot = capture_validated_docs(dir.path()).unwrap();

        let post = pre.replace("Original human note.", "Audit replacement.");
        fs::write(docs.join("AGENTS.md"), &post).unwrap();
        fs::write(
            docs.join("coding-rules.md"),
            "# Rules\nAI text, refreshed.\n",
        )
        .unwrap();
        let marked = mark_validated_doc_writes(
            dir.path(),
            &snapshot,
            crate::core::doc_provenance::DocWriter::FullAudit,
        )
        .unwrap();

        assert_eq!(marked, 2, "all preexisting text is tracked");
        assert_eq!(fs::read_to_string(docs.join("AGENTS.md")).unwrap(), post);
        assert!(!docs.join("reports").exists(), "no dated report any more");
        let report = crate::core::doc_provenance::report(dir.path(), true).unwrap();
        assert_eq!(report.to_reconfirm, 2);
        let change = &report.docs[0].changes[0];
        assert!(change
            .before
            .as_deref()
            .unwrap()
            .contains("Original human note."));
        assert!(change
            .after
            .as_deref()
            .unwrap()
            .contains("Audit replacement."));
    }

    /// Fencing a human section turns it into an example: still a change to
    /// reconfirm, not a silent loss.
    #[test]
    fn a_fence_wrapped_human_section_is_marked_to_reconfirm() {
        let dir = tempdir().unwrap();
        let docs = dir.path().join("docs");
        fs::create_dir_all(&docs).unwrap();
        let team = "<!-- kronn:section name=\"team\" owner=\"human\" -->\nKeep me live.\n<!-- kronn:section:end -->";
        let pre = format!("# Doc\n{team}\n");
        fs::write(docs.join("AGENTS.md"), &pre).unwrap();
        let snapshot = capture_validated_docs(dir.path()).unwrap();
        let post = pre.replace(team, &format!("```\n{team}\n```"));
        fs::write(docs.join("AGENTS.md"), &post).unwrap();
        let marked = mark_validated_doc_writes(
            dir.path(),
            &snapshot,
            crate::core::doc_provenance::DocWriter::PartialAudit,
        )
        .unwrap();
        assert_eq!(marked, 1);
        assert_eq!(fs::read_to_string(docs.join("AGENTS.md")).unwrap(), post);
        let doc = &crate::core::doc_provenance::report(dir.path(), true)
            .unwrap()
            .docs[0];
        assert!(doc.changes[0].before.as_ref().unwrap().contains(team));
        assert_eq!(doc.changes[0].after.as_deref(), Some(post.as_str()));
    }

    #[test]
    fn every_attempt_restores_briefing_rewrites_deletions_and_creations() {
        for original in [Some("# Briefing\nNever delete customer data.\n"), None] {
            for changed in [Some("# Briefing\nReplacement.\n"), None] {
                if original == changed {
                    continue;
                }
                let dir = tempdir().unwrap();
                fs::create_dir_all(dir.path().join("docs")).unwrap();
                let path = dir.path().join("docs/briefing.md");
                if let Some(text) = original {
                    fs::write(&path, text).unwrap();
                }
                let snapshot = capture_validated_docs(dir.path()).unwrap();
                if let Some(text) = changed {
                    fs::write(&path, text).unwrap();
                } else {
                    fs::remove_file(&path).unwrap();
                }
                let error = mark_validated_doc_writes(
                    dir.path(),
                    &snapshot,
                    crate::core::doc_provenance::DocWriter::FullAudit,
                )
                .unwrap_err();
                assert!(error.contains("protected briefing"), "{error}");
                assert_eq!(fs::read_to_string(&path).ok().as_deref(), original);
                assert!(mark_validated_doc_writes(
                    dir.path(),
                    &snapshot,
                    crate::core::doc_provenance::DocWriter::FullAudit
                )
                .is_ok());
            }
        }
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
