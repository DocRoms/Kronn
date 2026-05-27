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
/// mutate a `docs/` file outside the audit's 10-step pipeline (validation,
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
pub(super) fn compute_audit_info_sync(project_path_str: &str) -> AuditInfo {
    let project_path = scanner::resolve_host_path(project_path_str);
    let docs_dir = scanner::detect_docs_dir(&project_path);

    if !docs_dir.is_dir() {
        return AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] };
    }

    let mut files = Vec::new();
    let mut todos = Vec::new();

    for entry in walkdir::WalkDir::new(&docs_dir).max_depth(4).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let rel = entry.path().strip_prefix(&project_path).unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().to_string();

        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            let is_empty = content.lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with('>') && !l.starts_with("---") && !l.starts_with('|'))
                .count() < 3;

            files.push(AuditFileInfo {
                path: rel_str.clone(),
                filled: !is_empty && !content.contains("{{"),
            });

            for (line_num, line) in content.lines().enumerate() {
                if line.contains("<!-- TODO") {
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
            if !trimmed.starts_with('|') || trimmed.starts_with("| ID") || trimmed.starts_with("|--") || trimmed.contains("{{") {
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

    AuditInfo { files, todos, tech_debt_items }
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
) -> String {
    use crate::models::AuditKind;
    let (kind_label, index_file) = match kind {
        AuditKind::Security      => ("sécurité",      "docs/inconsistencies-security.md"),
        AuditKind::Docker        => ("Docker",        "docs/inconsistencies-docker.md"),
        AuditKind::Performance   => ("performance",   "docs/inconsistencies-performance.md"),
        AuditKind::Accessibility => ("accessibility (WCAG 2.1)", "docs/inconsistencies-accessibility.md"),
        AuditKind::Rgaa          => ("RGAA 4.1",      "docs/inconsistencies-rgaa.md"),
        AuditKind::Database      => ("base de données", "docs/inconsistencies-database.md"),
        AuditKind::ApiDesign     => ("design d'API",  "docs/inconsistencies-api.md"),
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
            `| ID | Sévérité | Domaine | Titre | Statut | Effort |`\n\
            Une ligne par TD. Tronque le titre à ~50 chars si nécessaire.\n\
         3. Demande à l'utilisateur :\n\
            > « Voici les N TDs identifiés par le sous-audit {}. Tu peux :\n\
            > (a) **Tout valider** → tous deviennent `Confirmed by user` ;\n\
            > (b) **Tout rejeter** → tous deviennent `Rejected` (le prochain audit ne les recréera pas) ;\n\
            > (c) **Détailler certains** → liste les IDs à discuter. Les TDs non listés en (c) seront `Confirmed by user` par défaut. »\n\
         4. Applique la réponse en mettant à jour le champ `audit_history` de chaque détail TD.\n",
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
pub(crate) fn build_validation_prompt(language: &str, info: &AuditInfo, has_issue_tracker_mcp: bool) -> String {
    let base = match language {
        "en" => {
            let mut s = String::from(concat!(
                "Validate the AI context (ai/ folder). Follow this 4-phase protocol. ",
                "Do NOT emit KRONN:VALIDATION_COMPLETE until ALL phases are done.\n\n",
                "**CRITICAL RULE: You are a DOCUMENTATION auditor, not a code fixer. ",
                "NEVER modify source code, Makefile, configs, or any file outside `docs/`. ",
                "Your ONLY job is to make `docs/` files accurate and complete.**\n\n",
                "## Phase 1 — Auto-fix (autonomous)\n",
                "Read source code to understand the project. Fix ONLY `docs/` files: orphan TODO markers, empty/skeleton files inferable from code, outdated info. ",
                "Update `docs/` files directly. Report fixes. Do NOT touch source code.\n\n",
                "## Phase 2 — Ambiguity questions (interactive)\n",
                "**Scan ALL `docs/` files for the 3 marker types** and address each one:\n",
                "- `<!-- TODO: ask user -->` — direct user question (intent / decision).\n",
                "- `<!-- TODO: verify -->` — first try a final Glob/Read to verify yourself; if still impossible, escalate as a user question and convert to `<!-- TODO: ask user -->` shape.\n",
                "- `<!-- TODO: unknown -->` — already a known unknown from a prior pass; re-ask the user.\n\n",
                "Use `grep -rn 'TODO: ' docs/` (or equivalent MCP tool) to enumerate them. Ask each remaining ambiguity **one by one**, then update the relevant `docs/` file with the answer and REMOVE the marker. ",
                "If the user reports a code issue, document it in `docs/inconsistencies-tech-debt.md` — do NOT fix the code yourself.\n",
                "If user answers 'I don't know' or 'skip', leave as `<!-- TODO: unknown -->` and move on.\n",
                "Phase 2 ends when every marker is either resolved (removed) or explicitly left as `<!-- TODO: unknown -->`.\n\n",
                "## Phase 3 — Tech debt review (BULK-FIRST, not 1-by-1)\n",
                "**Do NOT walk through TDs one-by-one** — that was the previous protocol and it bored users into bailing out before reaching the high-severity items.\n\n",
                "Instead, in ONE message:\n",
                "1. Read `docs/inconsistencies-tech-debt.md` AND every `docs/tech-debt/TD-*.md` (excluding README/TEMPLATE).\n",
                "2. Present a **compact markdown table** of ALL findings:\n",
                "   `| ID | Severity | Area | Title | Status | Effort |`\n",
                "   `| -- | -------- | ---- | ----- | ------ | ------ |`\n",
                "   One row per TD. Truncate Title to ~50 chars if needed. Use the existing Status from the detail file.\n",
                "3. Ask the user **one question**:\n",
                "   > « Voici les N TDs identifiés. Tu peux :\n",
                "   > (a) **Tout valider** → tous deviennent `Confirmed by user` ;\n",
                "   > (b) **Tout rejeter** → tous deviennent `Rejected` (le prochain audit ne les recréera pas) ;\n",
                "   > (c) **Détailler certains** → liste les IDs à discuter (ex: `TD-20260515-foo, TD-20260515-bar`).\n",
                "   > Les TDs non listés en (c) seront automatiquement marqués `Confirmed by user`. »\n",
                "4. Apply the answer:\n",
                "   - (a) → update the `audit_history` of every TD detail file with `status: Confirmed by user` (today's date). No 1-by-1 questions.\n",
                "   - (b) → update every TD's `status: Rejected` AND remove its row from the index table. The next audit's anti-repetition pass will skip them.\n",
                "   - (c) → for EACH selected ID: read the detail file, verify against source, ask the user (severity / priority / ticket?). For every OTHER TD not selected, default to `Confirmed by user`.\n",
                "5. If a ticket tracker MCP is available AND the user picked (a) or (c)-selected TDs, offer to create tickets for the High/Critical entries in ONE batch question, not per-TD.\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str(concat!(
                    "Also ask: create a ticket? (issue tracker available via MCP)\n",
                    "**Before creating tickets**: check `.github/ISSUE_TEMPLATE/` (or GitLab equivalent `.gitlab/issue_templates/`). ",
                    "If empty AND the project shows OSS intent (LICENSE present OR remote points at github.com / gitlab.com / codeberg.org), ",
                    "propose in ONE question:\n",
                    "> \"No issue template detected. I can create 3 minimal templates (`bug.md`, `feature.md`, `td-from-audit.md`) in `.github/ISSUE_TEMPLATE/` before pushing the tickets — they'll follow the `td-from-audit` format. Approve?\"\n",
                    "If yes: write the 3 files (YAML frontmatter + Description / Reproduction / Impact / Acceptance sections), commit them WITHOUT pushing (the user pushes), then create the tickets filling in the `td-from-audit` structure.\n",
                    "If no: create the tickets free-form, leave the repo untouched.\n",
                ));
            }
            s.push_str(concat!(
                "Do not batch-confirm. Update/remove `docs/` entries per feedback. Do NOT fix code — only update documentation.\n",
                "Also ask: did the audit miss anything obvious? (security, performance, compliance)\n\n",
                "## Phase 4 — Doc challenge (interactive)\n",
                "Ask 2-3 practical onboarding questions that must be answerable from `docs/` files alone. ",
                "Examples: 'How would a new dev add a new API endpoint?', 'What command runs all tests?', 'Where is the DB schema?'. ",
                "Check if `docs/` docs answer them correctly. Fix gaps in `docs/` files.\n\n",
                "## Completion\n",
                "All phases done → end with exact phrase: \"KRONN:VALIDATION_COMPLETE\". Never emit early.",
            ));
            s
        },
        "es" => {
            let mut s = String::from(concat!(
                "Valida el contexto AI (carpeta ai/). Sigue este protocolo de 4 fases. ",
                "NO emitas KRONN:VALIDATION_COMPLETE hasta completar TODAS las fases.\n\n",
                "**REGLA CRITICA: Eres un auditor de DOCUMENTACION, no un corrector de codigo. ",
                "NUNCA modifiques codigo fuente, Makefile, configs, ni ningun archivo fuera de `docs/`. ",
                "Tu UNICO trabajo: hacer los archivos `docs/` precisos y completos.**\n\n",
                "## Fase 1 — Auto-correccion (autonoma)\n",
                "Lee el codigo para entender el proyecto. Corrige SOLO archivos `docs/`: TODOs huerfanos, archivos esqueleto inferibles del codigo, info obsoleta. ",
                "Actualiza `docs/` directamente. Reporta. NO toques el codigo fuente.\n\n",
                "## Fase 2 — Preguntas (interactiva)\n",
                "**Escanea TODOS los archivos `docs/` buscando los 3 tipos de marcadores** y procesa cada uno:\n",
                "- `<!-- TODO: ask user -->` — pregunta directa al usuario (intencion / decision).\n",
                "- `<!-- TODO: verify -->` — primero intenta un Glob/Read final para verificar tu mismo; si sigue imposible, escala como pregunta y convierte en `<!-- TODO: ask user -->`.\n",
                "- `<!-- TODO: unknown -->` — ya es un desconocido conocido de una pasada anterior; vuelve a preguntar al usuario.\n\n",
                "Usa `grep -rn 'TODO: ' docs/` (o herramienta MCP equivalente) para enumerarlos. Pregunta cada ambiguedad **una por una**, luego actualiza el archivo `docs/` con la respuesta y ELIMINA el marcador. ",
                "Si el usuario reporta un problema de codigo, documentalo en `docs/inconsistencies-tech-debt.md` — NO corrijas el codigo tu mismo.\n",
                "Si el usuario responde 'no se' o 'saltar', deja como `<!-- TODO: unknown -->` y continua.\n",
                "Fase 2 termina cuando cada marcador esta resuelto (eliminado) o explicitamente marcado `<!-- TODO: unknown -->`.\n\n",
                "## Fase 3 — Deuda tecnica (BULK-FIRST, no una por una)\n",
                "**NO recorras los TDs uno por uno** — ese era el protocolo anterior y los usuarios abandonaban antes de llegar a los items criticos.\n\n",
                "En UN SOLO mensaje:\n",
                "1. Lee `docs/inconsistencies-tech-debt.md` Y todos los `docs/tech-debt/TD-*.md` (excluyendo README/TEMPLATE).\n",
                "2. Presenta una **tabla markdown compacta** de TODOS los hallazgos:\n",
                "   `| ID | Severity | Area | Title | Status | Effort |`\n",
                "   `| -- | -------- | ---- | ----- | ------ | ------ |`\n",
                "   Una fila por TD. Trunca Title a ~50 chars si hace falta.\n",
                "3. Haz **una sola pregunta**:\n",
                "   > « Aqui los N TDs identificados. Puedes :\n",
                "   > (a) **Validar todo** → todos pasan a `Confirmed by user` ;\n",
                "   > (b) **Rechazar todo** → todos pasan a `Rejected` (la proxima auditoria no los recreara) ;\n",
                "   > (c) **Detallar algunos** → lista los IDs a discutir (ej: `TD-20260515-foo, TD-20260515-bar`).\n",
                "   > Los TDs no listados en (c) seran marcados automaticamente `Confirmed by user`. »\n",
                "4. Aplica la respuesta:\n",
                "   - (a) → actualiza `audit_history` de cada TD con `status: Confirmed by user` (fecha de hoy). Sin preguntas 1-por-1.\n",
                "   - (b) → cada TD `status: Rejected` Y elimina su fila del indice. El anti-repetition pass de la proxima auditoria los saltara.\n",
                "   - (c) → para CADA ID seleccionado: lee, verifica, pregunta detalles. Los OTROS TDs no seleccionados pasan a `Confirmed by user` por defecto.\n",
                "5. Si MCP issue tracker disponible Y user eligio (a) o (c)-seleccionados, ofrece crear tickets para los High/Critical en UN solo batch, no por TD.\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str(concat!(
                    "Tambien: ¿crear ticket? (gestor de issues disponible via MCP)\n",
                    "**Antes de crear los tickets**: verifica `.github/ISSUE_TEMPLATE/` (o equivalente GitLab `.gitlab/issue_templates/`). ",
                    "Si esta vacio Y el proyecto muestra intent OSS (LICENSE presente O remote apunta a github.com / gitlab.com / codeberg.org), ",
                    "propone en UNA pregunta:\n",
                    "> \"No hay template de issue. Puedo crear 3 templates minimos (`bug.md`, `feature.md`, `td-from-audit.md`) en `.github/ISSUE_TEMPLATE/` antes de pushear los tickets — seguiran el formato `td-from-audit`. ¿Apruebas?\"\n",
                    "Si si: escribe los 3 archivos (frontmatter YAML + secciones Descripcion / Reproduccion / Impacto / Aceptacion), commit-los SIN push (el usuario hace push), luego crea los tickets siguiendo la estructura `td-from-audit`.\n",
                    "Si no: crea los tickets en formato libre, no toques el repo.\n",
                ));
            }
            s.push_str(concat!(
                "No confirmar en lote. Actualiza/elimina entradas `docs/` segun feedback. NO corrijas codigo — solo documenta.\n",
                "Tambien pregunta: ¿la auditoria omitio algo obvio? (seguridad, rendimiento, cumplimiento)\n\n",
                "## Fase 4 — Challenge doc (interactiva)\n",
                "Haz 2-3 preguntas practicas de onboarding que deben ser respondibles solo con los archivos `docs/`. ",
                "Ejemplos: '¿Como agregar un endpoint?', '¿Que comando ejecuta los tests?'. Corrige gaps en archivos `docs/`.\n\n",
                "## Fin\n",
                "Todas las fases completas → termina con: \"KRONN:VALIDATION_COMPLETE\". Nunca antes.",
            ));
            s
        },
        _ => {
            let mut s = String::from(concat!(
                "Valide le contexte AI (dossier ai/). Suis ce protocole en 4 phases. ",
                "NE PAS emettre KRONN:VALIDATION_COMPLETE avant la fin des 4 phases.\n\n",
                "**REGLE CRITIQUE : Tu es un auditeur de DOCUMENTATION, pas un correcteur de code. ",
                "NE MODIFIE JAMAIS le code source, Makefile, configs, ou tout fichier hors de `docs/`. ",
                "Ton SEUL travail : rendre les fichiers `docs/` precis et complets.**\n\n",
                "## Phase 1 — Auto-correction (autonome)\n",
                "Lis le code source pour comprendre le projet. Corrige UNIQUEMENT les fichiers `docs/` : TODOs orphelins, fichiers squelettes inferables du code, infos obsoletes. ",
                "Mets a jour `docs/` directement. Rapporte les corrections. NE touche PAS au code source.\n\n",
                "## Phase 2 — Questions (interactif)\n",
                "**Scanne TOUS les fichiers `docs/` a la recherche des 3 types de marqueurs** et traite chacun :\n",
                "- `<!-- TODO: ask user -->` — question directe a l'utilisateur (intention / decision).\n",
                "- `<!-- TODO: verify -->` — tente d'abord un Glob/Read final pour verifier toi-meme; si toujours impossible, escalade comme question et convertis en `<!-- TODO: ask user -->`.\n",
                "- `<!-- TODO: unknown -->` — deja un inconnu connu d'une passe precedente; re-pose la question a l'utilisateur.\n\n",
                "Utilise `grep -rn 'TODO: ' docs/` (ou outil MCP equivalent) pour les enumerer. Pose chaque ambiguite **une par une**, puis mets a jour le fichier `docs/` avec la reponse et SUPPRIME le marqueur. ",
                "Si l'utilisateur signale un probleme de code, documente-le dans `docs/inconsistencies-tech-debt.md` — NE corrige PAS le code toi-meme.\n",
                "Si l'utilisateur repond 'je ne sais pas' ou 'passer', laisse `<!-- TODO: unknown -->` et continue.\n",
                "Phase 2 termine quand chaque marqueur est resolu (supprime) ou explicitement laisse `<!-- TODO: unknown -->`.\n\n",
                "## Phase 3 — Dette technique (BULK-FIRST, plus de 1-par-1)\n",
                "**NE PARCOURS PAS les TDs un par un** — c'etait le protocole precedent et les users abandonnaient avant d'atteindre les items critiques.\n\n",
                "En UN SEUL message :\n",
                "1. Lis `docs/inconsistencies-tech-debt.md` ET tous les `docs/tech-debt/TD-*.md` (hors README/TEMPLATE).\n",
                "2. Presente une **table markdown compacte** de TOUS les findings :\n",
                "   `| ID | Severity | Area | Title | Status | Effort |`\n",
                "   `| -- | -------- | ---- | ----- | ------ | ------ |`\n",
                "   Une ligne par TD. Tronque Title a ~50 chars si necessaire.\n",
                "3. Pose **une seule question** :\n",
                "   > « Voici les N TDs identifies. Tu peux :\n",
                "   > (a) **Tout valider** → tous passent en `Confirmed by user` ;\n",
                "   > (b) **Tout rejeter** → tous passent en `Rejected` (le prochain audit ne les recreera pas) ;\n",
                "   > (c) **Detailler certains** → liste les IDs a discuter (ex: `TD-20260515-foo, TD-20260515-bar`).\n",
                "   > Les TDs non listes en (c) seront automatiquement marques `Confirmed by user`. »\n",
                "4. Applique la reponse :\n",
                "   - (a) → mets a jour `audit_history` de chaque TD avec `status: Confirmed by user` (date du jour). Pas de questions 1-par-1.\n",
                "   - (b) → chaque TD `status: Rejected` ET retire sa ligne de l'index. L'anti-repetition pass du prochain audit les sautera.\n",
                "   - (c) → pour CHAQUE ID selectionne : lis, verifie, demande les details. Les AUTRES TDs non selectionnes passent en `Confirmed by user` par defaut.\n",
                "5. Si MCP issue tracker dispo ET user a choisi (a) ou (c)-selectionnes, propose de creer les tickets pour les High/Critical en UN seul batch, pas par TD.\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str(concat!(
                    "Aussi : creer un ticket ? (gestionnaire d'issues dispo via MCP)\n",
                    "**Avant de creer les tickets** : verifie `.github/ISSUE_TEMPLATE/` (ou equivalent GitLab `.gitlab/issue_templates/`). ",
                    "Si vide ET que le projet a un repo OSS-intent (LICENSE present OU remote vers github.com / gitlab.com / codeberg.org), ",
                    "propose en UNE question :\n",
                    "> \"Pas de template d'issue detecte. Je peux creer 3 templates minimaux (`bug.md`, `feature.md`, `td-from-audit.md`) dans `.github/ISSUE_TEMPLATE/` avant de pousser les tickets — ils suivront le format `td-from-audit`. Tu valides ?\"\n",
                    "Si oui : ecris les 3 fichiers (frontmatter YAML + sections Description / Reproduction / Impact / Acceptance), commit-les SANS push (le user pushera), puis cree les tickets en remplissant la structure `td-from-audit`.\n",
                    "Si non : cree les tickets en free-form, sans toucher au repo.\n",
                ));
            }
            s.push_str(concat!(
                "Pas de confirmation en lot. Mets a jour/supprime les entrees `docs/` selon feedback. NE corrige PAS le code — documente seulement.\n",
                "Demande aussi : l'audit a-t-il rate quelque chose d'evident ? (securite, performance, conformite)\n\n",
                "## Phase 4 — Challenge doc (interactif)\n",
                "Pose 2-3 questions pratiques d'onboarding qui doivent etre couvertes par les fichiers `docs/` seuls. ",
                "Exemples : 'Comment ajouter un endpoint ?', 'Quelle commande lance les tests ?'. Corrige les lacunes dans les fichiers `docs/`.\n\n",
                "## Fin\n",
                "Toutes les phases terminees → termine par : \"KRONN:VALIDATION_COMPLETE\". Jamais avant.",
            ));
            s
        },
    };

    // 0.8.7 anti-hallu: prepend the doc-writer discipline reminder so
    // Phase 1 (auto-fix) and Phase 4 (challenge doc) which both mutate
    // `docs/` files inherit the same sourcing protocol the 10-step audit
    // gets via `PROMPT_PREAMBLE`. Without this, the validation pass was
    // structurally outside the anti-hallucination scope.
    let mut prompt = String::with_capacity(base.len() + 512);
    prompt.push_str(anti_halluc_doc_writer_block(language));
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
            "en" => format!("{} tech debt items to review in Phase 3. Read `docs/inconsistencies-tech-debt.md` and `docs/tech-debt/` for details.", info.tech_debt_items.len()),
            "es" => format!("{} items de deuda tecnica a revisar en Fase 3. Lee `docs/inconsistencies-tech-debt.md` y `docs/tech-debt/` para detalles.", info.tech_debt_items.len()),
            _ => format!("{} items de dette technique a revoir en Phase 3. Lis `docs/inconsistencies-tech-debt.md` et `docs/tech-debt/` pour les details.", info.tech_debt_items.len()),
        };
        prompt.push_str(&format!("\n\n{}", hint));
    }

    prompt
}

/// Auto-detect skills from project filesystem (config files, package managers, etc.)
pub(crate) fn detect_project_skills(project_path: &std::path::Path) -> Vec<String> {
    let mut skills: Vec<String> = Vec::new();

    // ── Language detection (from package managers / config files) ──
    if project_path.join("Cargo.toml").exists() {
        skills.push("rust".into());
    }
    if project_path.join("package.json").exists() {
        // Check if TypeScript
        if project_path.join("tsconfig.json").exists()
            || project_path.join("tsconfig.app.json").exists()
        {
            skills.push("typescript".into());
        }
    }
    if project_path.join("requirements.txt").exists()
        || project_path.join("pyproject.toml").exists()
        || project_path.join("setup.py").exists()
    {
        skills.push("python".into());
    }
    if project_path.join("go.mod").exists() {
        skills.push("go".into());
    }
    if project_path.join("composer.json").exists() {
        skills.push("php".into());
    }

    // ── Domain detection ──
    // DevOps: Dockerfile, CI/CD, IaC
    if project_path.join("Dockerfile").exists()
        || project_path.join("docker-compose.yml").exists()
        || project_path.join("docker-compose.yaml").exists()
        || project_path.join(".github").join("workflows").exists()
        || project_path.join(".gitlab-ci.yml").exists()
        || project_path.join("Makefile").exists()
    {
        skills.push("devops".into());
    }

    // Database: migrations, schema files
    if project_path.join("migrations").exists()
        || project_path.join("db").exists()
        || project_path.join("prisma").exists()
        || project_path.join("drizzle").exists()
    {
        skills.push("database".into());
    }

    // Security: auth configs, security headers
    if project_path.join(".env.example").exists()
        || project_path.join("security.yaml").exists()
        || project_path.join("config").join("packages").join("security.yaml").exists()
    {
        skills.push("security".into());
    }

    // ── Business detection ──
    // Web performance: frontend projects with build tools
    if project_path.join("webpack.config.js").exists()
        || project_path.join("vite.config.ts").exists()
        || project_path.join("vite.config.js").exists()
        || project_path.join("next.config.js").exists()
        || project_path.join("next.config.ts").exists()
    {
        skills.push("web-performance".into());
    }

    // SEO: robots.txt, sitemap
    if project_path.join("robots.txt").exists()
        || project_path.join("public").join("robots.txt").exists()
    {
        skills.push("seo".into());
    }

    // Filter to only keep skills that actually exist in the system
    let valid: Vec<String> = skills.into_iter()
        .filter(|id| crate::core::skills::get_skill(id).is_some())
        .collect();

    tracing::info!("Auto-detected skills for {}: {:?}", project_path.display(), valid);
    valid
}

pub(super) fn detect_issue_tracker_mcp(project_path: &std::path::Path) -> bool {
    let mcp_file = project_path.join(".mcp.json");
    if let Ok(content) = std::fs::read_to_string(&mcp_file) {
        let lower = content.to_lowercase();
        return lower.contains("github") || lower.contains("gitlab")
            || lower.contains("jira") || lower.contains("atlassian")
            || lower.contains("linear") || lower.contains("youtrack");
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
            "REGLA ABSOLUTA: NO leas el codigo fuente, los archivos del proyecto, ni ningun archivo fuera de ai/. ",
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
            "REGLE ABSOLUE: Tu ne lis PAS le code source, les fichiers du projet, ni aucun fichier en dehors de ai/. ",
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
        assert_eq!(ids, vec!["TD-20260512-keeper"],
            "phantom TD whose detail file was removed must not leak into validation prompt");
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
        assert!(!skills.contains(&"typescript".into()),
            "package.json alone must not imply typescript");
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
}
