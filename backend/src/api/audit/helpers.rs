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
                "Ask remaining ambiguities **one by one**. After each answer, update the relevant `docs/` file (repo-map, coding-rules, architecture, etc.). ",
                "If the user reports a code issue, document it in `docs/inconsistencies-tech-debt.md` — do NOT fix the code yourself.\n",
                "If user answers 'I don't know' or 'skip', mark as `<!-- TODO: unknown -->` and move on.\n",
                "Phase 2 ends when all TODOs are addressed or explicitly skipped.\n\n",
                "## Phase 3 — Tech debt review (interactive)\n",
                "For each entry in `docs/inconsistencies-tech-debt.md`:\n",
                "1. Read its detail file in `docs/tech-debt/`\n",
                "2. Verify against source code — does the issue still exist? Is the description accurate?\n",
                "3. Present to user one by one (or grouped by area if >10). Ask: confirm/reject? correct severity? priority?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Also ask: create a ticket? (issue tracker available via MCP)\n");
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
                "Pregunta ambiguedades **una por una**. Tras cada respuesta, actualiza el archivo `docs/` correspondiente (repo-map, coding-rules, architecture, etc.). ",
                "Si el usuario reporta un problema de codigo, documentalo en `docs/inconsistencies-tech-debt.md` — NO corrijas el codigo tu mismo.\n",
                "Si el usuario responde 'no se' o 'saltar', marca como `<!-- TODO: unknown -->` y continua.\n\n",
                "## Fase 3 — Deuda tecnica (interactiva)\n",
                "Para cada entrada en `docs/inconsistencies-tech-debt.md`:\n",
                "1. Lee su archivo detalle en `docs/tech-debt/`\n",
                "2. Verifica contra el codigo fuente — ¿el problema existe? ¿la descripcion es correcta?\n",
                "3. Presenta al usuario una por una (o agrupadas por area si >10). Pregunta: ¿confirmar/rechazar? ¿severidad? ¿prioridad?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Tambien: ¿crear ticket? (gestor de issues disponible via MCP)\n");
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
                "Pose les ambiguites **une par une**. Apres chaque reponse, mets a jour le fichier `docs/` concerne (repo-map, coding-rules, architecture, etc.). ",
                "Si l'utilisateur signale un probleme de code, documente-le dans `docs/inconsistencies-tech-debt.md` — NE corrige PAS le code toi-meme.\n",
                "Si l'utilisateur repond 'je ne sais pas' ou 'passer', marque `<!-- TODO: unknown -->` et continue.\n\n",
                "## Phase 3 — Dette technique (interactif)\n",
                "Pour chaque entree dans `docs/inconsistencies-tech-debt.md` :\n",
                "1. Lis son fichier detail dans `docs/tech-debt/`\n",
                "2. Verifie dans le code source — le probleme existe-t-il ? La description est-elle exacte ?\n",
                "3. Presente a l'utilisateur un par un (ou par domaine si >10). Demande : confirmer/rejeter ? severite ? priorite ?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Aussi : creer un ticket ? (gestionnaire d'issues dispo via MCP)\n");
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

    let mut prompt = base;

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

/// Build the briefing discussion prompt (conversational pre-audit)
pub(crate) fn build_briefing_prompt(language: &str) -> String {
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
