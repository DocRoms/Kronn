// 0.7.0 UX pass — 3 présets workflow non-triviaux pour la 1ère
// découverte du wizard. Chaque préset montre 4-5 steps + plusieurs
// primitives (loop, state, exec, gate, rollback) pour que l'utilisateur
// apprenne le pattern *en lisant le résultat*, pas en cherchant dans
// la doc. Les prompts sont pré-remplis avec les bonnes instructions
// (notamment l'écriture de `---STATE:---` pour les boucles).
//
// Pourquoi 3 et pas 5 ? Marie a besoin de pédagogie, pas d'inventaire.
// Antony aura ses 5+ patterns une fois qu'on aura validé l'effet
// pédagogique. Cyndie aura ses templates partagés team-wide en 0.8+.
//
// 0.6.0 i18n fix — les prompts sont buildés via `t()` au moment du
// click sur la carte, pour respecter la langue de l'UI. Avant, tout
// était hardcodé en FR — un user en EN voyait des prompts en FR.

import type { WorkflowStep, PromptVariable } from '../../types/generated';

export interface WorkflowPreset {
  /** Stable id for keying & i18n */
  id: 'auto-dev' | 'pr-gate' | 'deploy-rollback' | 'feature-planner' | 'daily-host-audit' | 'ticket-to-pr' | 'feasibility-autopilot';
  /** Icon emoji shown on the card */
  icon: string;
  /** i18n key for the card title */
  titleKey: string;
  /** i18n key for the 1-line description on the card */
  descKey: string;
  /** Comma-separated list of primitives shown as chip below the title */
  primitives: string[];
  /** Steps the preset will apply when picked. */
  steps: WorkflowStep[];
  /** Optional rollback chain (`workflow.on_failure`). */
  onFailure?: WorkflowStep[];
  /** Optional Exec allowlist (`workflow.exec_allowlist`). */
  execAllowlist?: string[];
  /** When true, the workflow's `workspace_config.require_isolation` is set so a
   *  run ABORTS if its git worktree can't be created — instead of silently
   *  running agents that push/mutate code in the developer's main checkout.
   *  Set on code-pushing presets (Ticket→PR, AutoDev, PR-Gate, Feasibility). */
  requireIsolation?: boolean;
  /** Optional launch-time variables. Used by presets like Feature Planner
   *  that take a single source URL/key at launch (epic URL, GitHub issue
   *  link, etc.). The wizard will populate the workflow's `variables`
   *  field, which surfaces a form when the user clicks Run. */
  variables?: PromptVariable[];
  /** 2026-06-11 — child workflows shipped alongside the parent as a bundle.
   *  When set, the wizard routes the save to `POST /api/workflows/bundle`:
   *  the children are created first, then the parent whose `SubWorkflow`
   *  steps reference them via `sub_workflow_id: "@bundle:<bundleId>"`.
   *  Children inherit the parent's `project_id` AND its git worktree
   *  (Phase 2 handoff), so a parent step like `create_pr` sees the child's
   *  implementation. See `docs/design/decomposed-autopilot-presets.md`. */
  childWorkflows?: ChildWorkflowPreset[];
}

/** A child workflow declared by a decomposed preset (see `childWorkflows`). */
export interface ChildWorkflowPreset {
  /** Sentinel referenced by the parent's `sub_workflow_id: "@bundle:<bundleId>"`. */
  bundleId: string;
  /** i18n'd display name of the child workflow. */
  name: string;
  /** The child's steps (its own internal loop — no Gate allowed in a child). */
  steps: WorkflowStep[];
  /** Exec allowlist for the child (it runs the tests). */
  execAllowlist?: string[];
}

type Translator = (key: string, ...args: (string | number)[]) => string;

/** Helper: build a minimal WorkflowStep with all the field defaults. */
function step(over: Partial<WorkflowStep> & { name: string; step_type: WorkflowStep['step_type'] }): WorkflowStep {
  return {
    description: null,
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
    output_format: { type: 'FreeText' },
    on_result: [],
    agent_settings: null,
    stall_timeout_secs: null,
    retry: null,
    delay_after_secs: null,
    mcp_config_ids: [],
    skill_ids: [],
    profile_ids: [],
    directive_ids: [],
    batch_quick_prompt_id: null,
    batch_items_from: null,
    batch_wait_for_completion: null,
    batch_max_items: null,
    batch_workspace_mode: null,
    batch_chain_prompt_ids: [],
    notify_config: null,
    api_plugin_slug: null,
    api_config_id: null,
    api_endpoint_path: null,
    api_method: null,
    api_query: null,
    api_path_params: null,
    api_headers: null,
    api_body: null,
    api_extract: null,
    api_pagination: null,
    api_timeout_ms: null,
    api_max_retries: null,
    api_output_var: null,
    gate_message: null,
    gate_request_changes_target: null,
    gate_notify_url: null,
    exec_command: null,
    exec_args: [],
    exec_timeout_secs: null,
    ...over,
  };
}

/** 2026-06-11 — deterministic `commit` step (0 token) for a sub-workflow
 *  child. Runs LAST so the validated implementation is committed to the
 *  parent's branch (the child shares it via Phase 2 handoff) and therefore
 *  SURVIVES worktree cleanup — without this, the agent's files stay
 *  uncommitted and are deleted when the run ends. Idempotent: a no-op commit
 *  (nothing staged) soft-skips. `git` runs as a bash subprocess (bash is in
 *  the allowlist) + `git` is added to the child allowlist for clarity. */
function commitStep(t: Translator): WorkflowStep {
  return step({
    name: 'commit',
    step_type: { type: 'Exec' },
    description: t('wiz.preset.shared.commitStepDesc'),
    exec_command: 'bash',
    exec_args: [
      '-c',
      [
        'set -e',
        'git add -A',
        'if git diff --cached --quiet; then echo "nothing to commit"; echo "[SIGNAL: SKIPPED]"; exit 0; fi',
        "tid=''",
        'if [ -f .kronn/current_task.json ]; then tid="$(grep -o \'"id"[[:space:]]*:[[:space:]]*"[^"]*"\' .kronn/current_task.json | head -1 | sed \'s/.*: *"//; s/"$//\')"; fi',
        'subject="Kronn AutoPilot${tid:+ [$tid]} — implementation (KRONN-traced)"',
        'body="$(git diff --cached --name-only)"',
        'git -c user.email="autopilot@kronn.local" -c user.name="Kronn AutoPilot" commit --no-verify -m "$subject" -m "$body"',
        'echo "→ committed $(git rev-parse --short HEAD) ${tid:+[$tid]}"',
        'echo "[SIGNAL: OK]"',
      ].join('\n'),
    ],
    exec_timeout_secs: 120,
  });
}

/** 2026-06-11 (Phase 3a) — deterministic `scope_check` (0 token, ADVISORY).
 *  Flags files changed outside the manifest's declared scope
 *  (`.kronn/files_touched.txt`) into `.kronn/decisions.md`. The run-2 live
 *  audit proved a prompt alone doesn't constrain scope — this makes drift
 *  VISIBLE without an agent. Mirror of the Rust `build_scope_check_step`. */
function scopeCheckStep(t: Translator): WorkflowStep {
  return step({
    name: 'scope_check',
    step_type: { type: 'Exec' },
    description: t('wiz.preset.shared.scopeCheckDesc'),
    exec_command: 'bash',
    exec_args: ['-c', [
      'set -e',
      "allow='.kronn/files_touched.txt'",
      'if [ ! -f "$allow" ]; then echo "no files_touched.txt — skip"; echo "[SIGNAL: SKIPPED]"; exit 0; fi',
      'base="$(git merge-base HEAD origin/main 2>/dev/null || git rev-parse HEAD 2>/dev/null)"',
      'changed="$( { git diff --name-only "$base" 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | sort -u )"',
      "extra=''",
      'while IFS= read -r f; do',
      '  [ -z "$f" ] && continue',
      '  if ! grep -qF -- "$f" "$allow" 2>/dev/null && ! awk -v p="$f" \'index(p,$0)==1{found=1} END{exit !found}\' "$allow" 2>/dev/null; then extra="$extra- $f\\n"; fi',
      'done <<EOF\n$changed\nEOF',
      'if [ -n "$extra" ]; then',
      "  { echo ''; echo '## Out-of-scope files (changed but NOT in manifest files_touched — review)'; printf '%b' \"$extra\"; } >> .kronn/decisions.md",
      '  echo "scope: out-of-scope files flagged in .kronn/decisions.md"',
      'else echo "scope: all changes within declared files_touched"; fi',
      'echo "[SIGNAL: OK]"',
      'exit 0',
    ].join('\n')],
    exec_timeout_secs: 60,
  });
}

/** 2026-06-11 (Phase 3a) — deterministic `completeness_check` (0 token,
 *  ENFORCING). For each id in `.kronn/decision_ids.txt`, greps for its
 *  `KRONN-*(<id>)` marker; a missing one ⇒ `[SIGNAL: MISSING]` ⇒ loop back to
 *  `implement` (capped). 0-token anti-skip replacing an agent review. Mirror
 *  of the Rust `build_completeness_check_step`. */
function completenessCheckStep(t: Translator): WorkflowStep {
  return step({
    name: 'completeness_check',
    step_type: { type: 'Exec' },
    description: t('wiz.preset.shared.completenessCheckDesc'),
    exec_command: 'bash',
    exec_args: ['-c', [
      'set -e',
      "ids='.kronn/decision_ids.txt'",
      'if [ ! -f "$ids" ]; then echo "no decision_ids.txt — skip"; echo "[SIGNAL: OK]"; exit 0; fi',
      "missing=''",
      'while IFS= read -r id; do',
      '  [ -z "$id" ] && continue',
      '  if ! grep -rqE "KRONN-(ASSUMED|MOCKED|TODO)\\($id\\)" --include=\'*.php\' --include=\'*.ts\' --include=\'*.tsx\' --include=\'*.js\' --include=\'*.scss\' --include=\'*.css\' --include=\'*.twig\' --include=\'*.yaml\' --include=\'*.yml\' --exclude-dir=node_modules --exclude-dir=vendor --exclude-dir=.git . 2>/dev/null; then missing="$missing $id"; fi',
      'done < "$ids"',
      'if [ -n "$missing" ]; then',
      "  { echo ''; echo \"## Missing markers — sub-tasks possibly skipped:$missing\"; } >> .kronn/decisions.md",
      '  echo "completeness: MISSING markers for:$missing"; exit 3',
      'fi',
      'echo "completeness: every decision id has its KRONN-* marker"',
      'echo "[SIGNAL: OK]"',
      'exit 0',
    ].join('\n')],
    exec_timeout_secs: 120,
    on_result: [
      { contains: 'exit_3', action: { type: 'Goto', step_name: 'implement', max_iterations: 2 } },
    ],
  });
}

/** 0.6.0 — build the 3 presets in the user's UI language.
 *  Called from the wizard at click-time so locale changes between
 *  preset clicks reflect immediately. */
export function buildV07Presets(t: Translator): WorkflowPreset[] {
  const AUTO_DEV: WorkflowPreset = {
    id: 'auto-dev',
    icon: '🔁',
    titleKey: 'wiz.preset.autoDev.title',
    descKey: 'wiz.preset.autoDev.desc',
    primitives: ['JsonData', 'Agent', 'Exec', 'Loop', 'State', 'Notify'],
    execAllowlist: ['cargo', 'npm', 'make', 'pytest'],
    requireIsolation: true,
    // 0.7+ — pas de launch variables par défaut : le step `fetch_issue`
    // est en mode JsonData (fixture), il n'utilise aucune variable. Si
    // l'utilisateur swap fetch_issue en ApiCall et tape `{{issue_key}}`
    // dans `api_path_params`, le scanner live-warning du wizard flagge la
    // var non-déclarée et propose un bouton "+ Add variable" inline. Pas
    // de bruit "config" alors que rien n'est utilisé.
    steps: [
      // 0.7+ — Étape 1 en JsonData (fixture) pour que le préset soit
      // testable immédiatement, SANS plugin tracker installé. L'utilisateur
      // édite le payload pour adapter sa demande, OU swap le step en
      // ApiCall (+ pointe son QuickApi via `quick_api_id`) quand il a
      // un tracker branché. Voir `json_data_step.rs` — use case "fixture".
      // `output_format: Structured` requis : le step suivant lit
      // `{{steps.fetch_issue.data}}` et `validate_step_references` ne
      // laisse passer que les producteurs Structured.
      step({
        name: 'fetch_issue',
        step_type: { type: 'JsonData' },
        output_format: { type: 'Structured' },
        description: t('wiz.preset.autoDev.fetchIssueDesc'),
        json_data_payload: {
          key: 'DEMO-1',
          title: 'Exemple : ajoute un toggle clair / sombre dans le header',
          description: 'Ajoute un bouton ☀/🌙 dans le header global qui bascule la classe `theme-dark` sur `<html>`. Persistance via localStorage. À tester sur les 3 pages principales.',
        },
      }),
      step({
        name: 'implement',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.autoDev.implementPrompt'),
      }),
      step({
        name: 'run_tests',
        step_type: { type: 'Exec' },
        description: t('wiz.preset.shared.runTestsDesc'),
        exec_command: 'cargo',
        exec_args: ['test'],
        exec_timeout_secs: 600,
        // Tests fail (non-zero exit) → loop back to `implement` instead
        // of triggering on_failure. The Exec executor emits `[SIGNAL: ERROR]`
        // when exit_code≠0; the runner honours this Goto even though the
        // step status is `Failed`. Capped at 5 to mirror the review loop.
        on_result: [
          { contains: 'ERROR', action: { type: 'Goto', step_name: 'implement', max_iterations: 5 } },
        ],
      }),
      step({
        name: 'review',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.autoDev.reviewPrompt'),
        on_result: [
          { contains: 'NEEDS_CHANGES', action: { type: 'Goto', step_name: 'implement', max_iterations: 5 } },
          { contains: 'APPROVED', action: { type: 'Stop' } },
        ],
      }),
      step({
        name: 'notify_done',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.autoDev.notifyDoneBody') }),
        },
      }),
    ],
    onFailure: [
      step({
        name: 'rollback_notify',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.autoDev.rollbackBody') }),
        },
      }),
    ],
  };

  const PR_GATE: WorkflowPreset = {
    id: 'pr-gate',
    icon: '✋',
    titleKey: 'wiz.preset.prGate.title',
    descKey: 'wiz.preset.prGate.desc',
    primitives: ['JsonData', 'Agent', 'Exec', 'Gate', 'Webhook', 'Rollback'],
    execAllowlist: ['cargo', 'npm', 'pytest'],
    requireIsolation: true,
    // 0.7+ — pas de launch variables par défaut. Cf. AUTO_DEV.
    steps: [
      // 0.7+ — Même pattern qu'AUTO_DEV : JsonData (fixture) par défaut
      // pour que le préset soit testable sans plugin tracker. Voir le
      // commentaire sur AUTO_DEV.fetch_issue.
      step({
        name: 'fetch_issue',
        step_type: { type: 'JsonData' },
        output_format: { type: 'Structured' },
        description: t('wiz.preset.prGate.fetchIssueDesc'),
        json_data_payload: {
          key: 'DEMO-1',
          title: 'Exemple : ajoute un toggle clair / sombre dans le header',
          description: 'Ajoute un bouton ☀/🌙 dans le header global qui bascule la classe `theme-dark` sur `<html>`. Persistance via localStorage. À tester sur les 3 pages principales.',
        },
      }),
      step({
        name: 'implement',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.prGate.implementPrompt'),
      }),
      step({
        name: 'run_tests',
        step_type: { type: 'Exec' },
        description: t('wiz.preset.shared.runTestsDesc'),
        exec_command: 'cargo',
        exec_args: ['test'],
        exec_timeout_secs: 600,
        // Tests fail (non-zero exit) → loop back to `implement` instead
        // of triggering on_failure. The Exec executor emits `[SIGNAL: ERROR]`
        // when exit_code≠0; the runner honours this Goto even though the
        // step status is `Failed`. Capped at 5 to mirror the review loop.
        on_result: [
          { contains: 'ERROR', action: { type: 'Goto', step_name: 'implement', max_iterations: 5 } },
        ],
      }),
      step({
        name: 'pre_merge_gate',
        step_type: { type: 'Gate' },
        gate_message: t('wiz.preset.prGate.gateMessage'),
        gate_request_changes_target: 'implement',
        gate_notify_url: 'https://hooks.slack.com/services/REPLACE_ME',
      }),
      step({
        name: 'merge_branch',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.prGate.mergePrompt'),
      }),
    ],
    onFailure: [
      step({
        name: 'rollback_notify',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.prGate.rollbackBody') }),
        },
      }),
    ],
  };

  const DEPLOY_ROLLBACK: WorkflowPreset = {
    id: 'deploy-rollback',
    icon: '🚀',
    titleKey: 'wiz.preset.deployRollback.title',
    descKey: 'wiz.preset.deployRollback.desc',
    primitives: ['Exec', 'Rollback chain', 'Agent post-mortem'],
    // Allowlist alignée sur les binaires réellement invoqués par les steps
    // (3× `make`). Si tu ajoutes un step qui appelle un autre binaire,
    // pense à étendre cette liste — sinon le runner refusera l'exécution.
    execAllowlist: ['make'],
    steps: [
      step({
        name: 'build',
        step_type: { type: 'Exec' },
        exec_command: 'make',
        exec_args: ['build'],
        exec_timeout_secs: 600,
      }),
      step({
        name: 'smoke_tests',
        step_type: { type: 'Exec' },
        exec_command: 'make',
        exec_args: ['smoke'],
        exec_timeout_secs: 300,
      }),
      step({
        name: 'deploy',
        step_type: { type: 'Exec' },
        exec_command: 'make',
        exec_args: ['deploy'],
        exec_timeout_secs: 900,
      }),
      step({
        name: 'notify_success',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.deployRollback.successBody') }),
        },
      }),
    ],
    onFailure: [
      step({
        name: 'alert_ops',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.deployRollback.alertBody') }),
        },
      }),
      step({
        name: 'post_mortem',
        step_type: { type: 'Agent' },
        output_format: { type: 'FreeText' },
        prompt_template: t('wiz.preset.deployRollback.postMortemPrompt'),
      }),
    ],
  };

  // 0.6.0 — Feature Planner. Takes an epic / feature / issue URL, breaks it
  // down into a tree of sub-tasks tagged `auto_ai` / `human_action`, with
  // blocking links between them. Tracker-agnostic: the agent picks the right
  // MCP (Atlassian, GitHub, Linear, …) from the URL host. The Gate step
  // gives the human a chance to review the plan AND the autonomous decisions
  // the agent took before any ticket is actually created.
  const FEATURE_PLANNER: WorkflowPreset = {
    id: 'feature-planner',
    icon: '🗺️',
    titleKey: 'wiz.preset.featurePlanner.title',
    descKey: 'wiz.preset.featurePlanner.desc',
    primitives: ['Agent', 'BatchApi', 'Gate', 'Notify', 'Variables'],
    variables: [
      {
        name: 'epic_url',
        label: 'Epic / feature URL',
        placeholder: 'https://your.atlassian.net/browse/EW-7247  ·  github.com/org/repo/issues/123  ·  linear.app/...',
        description: 'Source URL of the epic / feature / issue to break down. The agent picks the right MCP from the host.',
        required: true,
      },
    ],
    steps: [
      step({
        name: 'analyze_epic',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.featurePlanner.analyzePrompt'),
      }),
      step({
        name: 'review_plan',
        step_type: { type: 'Gate' },
        gate_message: t('wiz.preset.featurePlanner.gateMessage'),
        gate_request_changes_target: 'analyze_epic',
      }),
      // Pure mechanical fan-out: 1 POST /issue per planned sub-task,
      // in parallel, zero tokens. The plugin/endpoint/method below are
      // pre-set for Jira; the user can swap them post-creation in the
      // wizard for GitHub or Linear (the planner's `tracker` field
      // helps decide). The body template is intentionally Jira-shaped;
      // for non-Jira, the user edits it after picking the preset.
      // `Goto self (max 2)` retries on transient HTTP failures
      // (PARTIAL signal). The agent in step 1 is responsible for
      // pre-filtering items_from to exclude already-existing tickets,
      // so retries don't duplicate.
      step({
        name: 'create_tickets',
        step_type: { type: 'BatchApiCall' },
        batch_items_from: '{{steps.analyze_epic.data.sub_tasks}}',
        batch_concurrent_limit: 5,
        batch_max_items: 50,
        api_method: 'POST',
        api_endpoint_path: '/rest/api/3/issue',
        api_body: JSON.stringify({
          fields: {
            project: { key: '{{steps.analyze_epic.data.source_key}}' },
            summary: '{{batch.item.title}}',
            description: '{{batch.item.description}}',
            labels: ['{{batch.item.type}}'],
            issuetype: { name: 'Task' },
          },
        }),
        api_extract: { path: '$.key', fallback: null, fail_on_empty: false },
        on_result: [
          { contains: 'PARTIAL', action: { type: 'Goto', step_name: 'create_tickets', max_iterations: 2 } },
        ],
      }),
      // Second pass with the agent: read the created keys back from
      // the batch outcome + the original plan, set blocks/blocked_by
      // links via MCP. Only ~3-5 MCP calls for a 30-ticket plan
      // (one per blocking relationship), so token cost is tiny.
      step({
        name: 'set_links',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.featurePlanner.setLinksPrompt'),
      }),
      step({
        name: 'notify_done',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.featurePlanner.notifyBody') }),
        },
      }),
    ],
  };

  // 0.7+ — Daily host audit. Démontre la brique `JsonData` (déterministe,
  // 0 token, 0 réseau) qui alimente un BatchQuickPrompt sur une liste
  // figée. Use case canonique : "tous les matins, audit de N hosts /
  // domaines / locales" sans monter d'API juste pour la liste.
  // L'utilisateur complète le batch_quick_prompt_id avec un QP existant
  // après application du préset.
  const DAILY_HOST_AUDIT: WorkflowPreset = {
    id: 'daily-host-audit',
    icon: '🌍',
    titleKey: 'wiz.preset.dailyHostAudit.title',
    descKey: 'wiz.preset.dailyHostAudit.desc',
    primitives: ['JsonData', 'BatchQuickPrompt', 'Notify'],
    steps: [
      // Liste figée — l'utilisateur édite pour adapter à son contexte.
      // 5 hosts par défaut pour rester compact dans le wizard ; on peut
      // monter à 50 sans souci (cf. batch_max_items downstream).
      step({
        name: 'host-list',
        step_type: { type: 'JsonData' },
        output_format: { type: 'Structured' },
        description: t('wiz.preset.dailyHostAudit.hostListDesc'),
        json_data_payload: [
          { host: 'fr.example.com' },
          { host: 'de.example.com' },
          { host: 'en.example.com' },
          { host: 'es.example.com' },
          { host: 'it.example.com' },
        ],
      }),
      // BatchQuickPrompt fan-out : l'utilisateur picke son QP audit
      // après application. `batch_items_from` est pré-câblé sur le
      // payload du step précédent. Chaque enfant verra `{{batch.item.host}}`
      // dans le prompt rendu via le QP.
      step({
        name: 'audit-each-host',
        step_type: { type: 'BatchQuickPrompt' },
        description: t('wiz.preset.dailyHostAudit.auditEachDesc'),
        batch_items_from: '{{steps.host-list.data}}',
        batch_max_items: 50,
        batch_wait_for_completion: true,
      }),
      // Récapitulatif Notify (Slack / Teams / webhook custom).
      // L'utilisateur remplace l'URL et adapte le body à sa cible.
      step({
        name: 'notify-done',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({
            text: t('wiz.preset.dailyHostAudit.notifyBody'),
          }),
        },
      }),
    ],
  };

  // 0.7+ — Ticket Autopilot. Compose les briques 0.6.0 + les skills
  // méthodologiques externes vendored (test-driven-development,
  // systematic-debugging, writing-plans, brainstorming, verification-
  // before-completion, requesting-code-review, receiving-code-review,
  // finishing-a-development-branch — toutes adaptées de obra/superpowers
  // MIT, voir backend/src/skills/external/) pour livrer un pipeline
  // "ticket en entrée → PR créée et prête au merge".
  //
  // Limites assumées en v1 (Sprint 1) :
  //   - Pas d'attente CI auto (Sprint 3 — step Wait/Poll). L'humain
  //     valide la PR via `ready_gate` avant le merge éventuel.
  //   - Pas de gestion auto des review comments humaines après merge
  //     (relance manuelle du workflow). Sprint 4 — Webhook receiver.
  //   - Pas de "Agent asks human mid-run" dynamique (Sprint 2 —
  //     skip_if + Ask Human pattern).
  //   - Pas d'auto-merge ApiCall (à packager en v2 quand l'utilisateur
  //     a un plugin GitHub/GitLab actif).
  //
  // Le step `fetch_issue` démarre en JsonData (fixture) pour que le
  // préset tourne immédiatement, sans plugin tracker installé. Cf.
  // AUTO_DEV pour le même pattern.
  const TICKET_TO_PR: WorkflowPreset = {
    id: 'ticket-to-pr',
    icon: '🎫',
    titleKey: 'wiz.preset.ticketToPr.title',
    descKey: 'wiz.preset.ticketToPr.desc',
    primitives: ['JsonData', 'Agent', 'SubWorkflow', 'Gate', 'Loop', 'State', 'Notify'],
    // The parent has no Exec step (the test loop lives in the child workflow,
    // which carries its own execAllowlist). requireIsolation = true so the
    // parent creates the git worktree that the child INHERITS (Phase 2 handoff).
    requireIsolation: true,
    steps: [
      // Étape 1 — Source du ticket. JsonData fixture par défaut, swap
      // en ApiCall (+ quick_api_id) quand un plugin tracker est actif.
      step({
        name: 'fetch_issue',
        step_type: { type: 'JsonData' },
        output_format: { type: 'Structured' },
        description: t('wiz.preset.ticketToPr.fetchIssueDesc'),
        json_data_payload: {
          key: 'DEMO-1',
          title: 'Exemple : ajoute un toggle clair / sombre dans le header',
          description: 'Ajoute un bouton ☀/🌙 dans le header global qui bascule la classe `theme-dark` sur `<html>`. Persistance via localStorage. À tester sur les 3 pages principales.',
          labels: ['feature', 'ui'],
          priority: 'medium',
        },
      }),
      // Étape 2 — Analyse + plan. Le `writing-plans` skill apprend à
      // produire un plan structured, `brainstorming` force à explorer
      // l'intent avant l'implémentation, `verification` interdit les
      // claims sans evidence.
      step({
        name: 'analyze',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.ticketToPr.analyzePrompt'),
        skill_ids: [
          'writing-plans',
          'brainstorming',
          'verification-before-completion',
        ],
      }),
      // Étape 3 — Validation humaine du plan. `gate_request_changes_target`
      // pointe vers `analyze` pour que le user puisse demander un re-plan.
      step({
        name: 'plan_gate',
        step_type: { type: 'Gate' },
        gate_message: t('wiz.preset.ticketToPr.planGateMessage'),
        gate_request_changes_target: 'analyze',
      }),
      // Étape 4 — Sous-workflow « implement-verify » : la boucle
      // implement ↔ run_tests ↔ review tourne dans un RUN ENFANT qui
      // PARTAGE le worktree du parent (handoff Phase 2). L'enfant lit le
      // plan validé dans `.kronn/plan.md` (écrit par analyze) et journalise
      // ses écarts dans `.kronn/decisions.md` — deux fichiers que parent ET
      // enfant voient via le worktree partagé (pas de canal de données
      // backend). Les Gotos internes (test ERROR → implement, review
      // NEEDS_CHANGES → implement) vivent ENTIÈREMENT dans l'enfant ; aucun
      // Gate dedans (interdit en sous-workflow). Sur échec du run enfant,
      // SUBWF_FAILED → on relance le sous-workflow une fois, puis on_failure.
      step({
        name: 'implement_verify',
        step_type: { type: 'SubWorkflow' },
        sub_workflow_id: '@bundle:implement-verify',
        description: t('wiz.preset.ticketToPr.implementVerifyDesc'),
        on_result: [
          { contains: 'SUBWF_FAILED', action: { type: 'Goto', step_name: 'implement_verify', max_iterations: 1 } },
        ],
      }),
      // Étape 7 — Validation humaine AVANT le push + PR (gate-before-effect).
      // Volontairement placée AVANT create_pr : pousser une branche et créer
      // une PR sont des effets externes ~irréversibles. Ici l'humain voit le
      // plan, l'implémentation ET le résultat des tests — y compris un
      // `[SIGNAL: SKIPPED]` (tests non lancés) — et décide en connaissance de
      // cause. request_changes → retour à implement. Tant qu'il n'a pas
      // approuvé, AUCUN push n'a eu lieu (contrairement à l'ordre inverse, où
      // le gate ne protégeait plus que la notif finale).
      step({
        name: 'ready_gate',
        step_type: { type: 'Gate' },
        gate_message: t('wiz.preset.ticketToPr.readyGateMessage'),
        // INV-1 : un Gate parent ne peut pas cibler un step INTERNE à
        // l'enfant. « Demander des changements » relance donc tout le
        // sous-workflow `implement_verify` (qui ré-entre dans sa boucle
        // implement→test→review, en relisant decisions.md).
        gate_request_changes_target: 'implement_verify',
      }),
      // Étape 8 — Création de la PR, UNIQUEMENT après l'approbation humaine.
      // `finishing-a-development-branch` guide le pattern push + PR. Le prompt
      // refuse de pousser si les tests ont été skippés/échoués (cf.
      // createPrPrompt) — double garde avec le gate ci-dessus.
      step({
        name: 'create_pr',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.ticketToPr.createPrPrompt'),
        skill_ids: [
          'finishing-a-development-branch',
          'verification-before-completion',
        ],
      }),
      // Étape 9 — Notification finale (Slack par défaut, à adapter).
      step({
        name: 'notify_done',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.ticketToPr.notifyDoneBody') }),
        },
      }),
    ],
    // En cas d'échec à n'importe quel step (RunStatus::Failed après
    // exhaustion des Goto loops par exemple), notif d'alerte.
    onFailure: [
      step({
        name: 'rollback_notify',
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hooks.slack.com/services/REPLACE_ME',
          method: 'POST',
          headers: {},
          body_template: JSON.stringify({ text: t('wiz.preset.ticketToPr.rollbackBody') }),
        },
      }),
    ],
    // 2026-06-11 — the implement↔test↔review loop is a CHILD workflow. The
    // parent's `implement_verify` SubWorkflow step references it via
    // `@bundle:implement-verify`; the bundle endpoint creates it first and the
    // child inherits the parent's project_id + worktree (Phase 2 handoff).
    // No Gate inside (forbidden in a child) — both human gates stay in the
    // parent. The internal Gotos target `implement` (child-internal, valid).
    childWorkflows: [
      {
        bundleId: 'implement-verify',
        name: t('wiz.preset.ticketToPr.childName'),
        // `bash` lets `run_tests` use a generic detection script that adapts
        // to any project (Cargo / npm-pnpm-yarn / composer / pytest / make).
        // `git` for the final commit step.
        execAllowlist: ['bash', 'cargo', 'pnpm', 'npm', 'yarn', 'pytest', 'make', 'composer', 'git'],
        steps: [
          // implement — lit le plan validé dans `.kronn/plan.md` (écrit par
          // analyze côté parent, visible via le worktree partagé) et journalise
          // ses écarts dans `.kronn/decisions.md`.
          step({
            name: 'implement',
            step_type: { type: 'Agent' },
            output_format: { type: 'Structured' },
            prompt_template: t('wiz.preset.ticketToPr.implementPrompt'),
            skill_ids: [
              'test-driven-development',
              'systematic-debugging',
              'verification-before-completion',
              'receiving-code-review',
            ],
            retry: { max_retries: 1, backoff: 'exponential' },
            stall_timeout_secs: 1800,
          }),
          // run_tests — auto-détecte le framework (Make / Cargo / pnpm-yarn-npm
          // / composer / pytest). Sur ERROR → retour implement (max 2).
          step({
            name: 'run_tests',
            step_type: { type: 'Exec' },
            description: t('wiz.preset.shared.runTestsDesc'),
            exec_command: 'bash',
            exec_args: [
              '-c',
              [
                'set -e',
                "if [ -f Makefile ] && grep -qE '^test:' Makefile; then echo '→ make test'; exec make test; fi",
                "if [ -f Cargo.toml ]; then echo '→ cargo test --lib'; exec cargo test --lib; fi",
                'if [ -f package.json ] && grep -q \'"test"\' package.json; then',
                '  if [ ! -d node_modules ]; then echo "⚠ node_modules absent dans le worktree — skip"; echo "[SIGNAL: SKIPPED]"; exit 0; fi',
                "  if [ -f pnpm-lock.yaml ]; then echo '→ pnpm test'; exec pnpm test",
                "  elif [ -f yarn.lock ]; then echo '→ yarn test'; exec yarn test",
                "  else echo '→ npm test'; exec npm test; fi",
                'fi',
                'if [ -f composer.json ]; then',
                '  if [ ! -d vendor ]; then echo "⚠ vendor/ absent — skip"; echo "[SIGNAL: SKIPPED]"; exit 0; fi',
                '  if grep -q \'"test"\' composer.json; then echo "→ composer test"; exec composer test',
                "  elif [ -x vendor/bin/phpunit ]; then echo '→ vendor/bin/phpunit'; exec vendor/bin/phpunit",
                "  else echo 'no PHP test runner'; echo '[SIGNAL: SKIPPED]'; exit 0; fi",
                'fi',
                "if [ -f pyproject.toml ] || [ -f setup.py ]; then echo '→ pytest'; exec pytest; fi",
                "echo '→ aucun framework de tests détecté — skip'",
                "echo '[SIGNAL: SKIPPED]'",
                'exit 0',
              ].join('\n'),
            ],
            exec_timeout_secs: 900,
            on_result: [
              { contains: 'ERROR', action: { type: 'Goto', step_name: 'implement', max_iterations: 2 } },
            ],
          }),
          // review — lit le DIFF réel + `.kronn/plan.md` + `.kronn/decisions.md`.
          // Sur NEEDS_CHANGES → écrit state.last_review puis Goto implement.
          step({
            name: 'review',
            step_type: { type: 'Agent' },
            output_format: { type: 'Structured' },
            prompt_template: t('wiz.preset.ticketToPr.reviewPrompt'),
            skill_ids: [
              'requesting-code-review',
              'verification-before-completion',
            ],
            retry: { max_retries: 1, backoff: 'exponential' },
            stall_timeout_secs: 1800,
            on_result: [
              { contains: 'NEEDS_CHANGES', action: { type: 'Goto', step_name: 'implement', max_iterations: 2 } },
            ],
          }),
          // commit the reviewed implementation to the parent branch so the
          // parent's create_pr step has something to push.
          commitStep(t),
        ],
      },
    ],
  };

  // 0.8.3 — Feasibility-Gated AutoPilot. 7-step mixed-primitives
  // pattern for big tickets. Designed against EW-7247 (Africanews→
  // Euronews migration Epic) — see [[project_feasibility_gated_
  // implementation]] memory. Every "freedom" the agent takes is traced
  // via a manifest (decided / mocked / blocked categories) AND a
  // KRONN-(ASSUMED|MOCKED|TODO) marker in the code. Production
  // path: AutoPilot CTA on validation discussion → this preset.
  //
  // Token-cost discipline: only triage / implement / pr_draft are
  // Agent. fetch_issue (JsonData → ApiCall via wizard transform),
  // review_triage (Gate), run_tests + drift_check (Exec) are all
  // 0-token. See [[feedback_kronn_deagentify_first]].
  const FEASIBILITY_AUTOPILOT: WorkflowPreset = {
    id: 'feasibility-autopilot',
    icon: '🎯',
    titleKey: 'wiz.preset.feasibilityAutopilot.title',
    descKey: 'wiz.preset.feasibilityAutopilot.desc',
    primitives: ['JsonData', 'Agent', 'Gate', 'Exec', 'SubWorkflow'],
    // Two-brains pipeline (2026-06-12) : plan_lint + plan_review challengent
    // le plan AVANT la gate ; les suites de tests complètes + l'audit drift
    // tournent au PARENT sur le worktree mergé (l'enfant ne fait qu'un check
    // syntaxique par tâche). requireIsolation = true so the parent creates
    // the worktree the child INHERITS (Phase 2 handoff).
    requireIsolation: true,
    execAllowlist: ['bash', 'grep', 'git', 'yarn', 'npm', 'php'],
    steps: [
      // Étape 1 — Source du ticket. Comme `ticket-to-pr`, démarre en
      // JsonData fixture, swap en ApiCall par le wizard quand un
      // plugin tracker est actif (voir WorkflowWizard.tsx transform).
      step({
        name: 'fetch_issue',
        step_type: { type: 'JsonData' },
        output_format: { type: 'Structured' },
        description: t('wiz.preset.feasibilityAutopilot.fetchIssueDesc'),
        json_data_payload: {
          key: 'DEMO-1',
          body: 'Exemple : refactor le bouton de connexion en composant brand-aware. Le composant doit lire BrandContext.getBrand() au runtime et appliquer la classe `btn-{{brand}}-primary`. Touche le header + le footer.',
        },
      }),
      // Étape 2 — Triage. Agent + TypedSchema(manifest). Le marker
      // `[TRIAGE]` dans description déclenche l'addendum "audit,
      // don't code" côté runner (cf. triage::is_triage_step).
      step({
        name: 'triage',
        step_type: { type: 'Agent' },
        description: t('wiz.preset.feasibilityAutopilot.triageDesc'),
        prompt_template: t('wiz.preset.feasibilityAutopilot.triagePrompt'),
        // Schema validation is `Fail` strict — an invalid manifest
        // never reaches `implement`. Frontend doesn't see the
        // `on_invalid` enum directly; the runner reads it from the
        // backend default. Triage schema lives in
        // backend/src/workflows/triage.rs::triage_manifest_schema.
        output_format: {
          type: 'TypedSchema',
          on_invalid: 'Fail',
          schema: {
            type: 'object',
            required: ['clear', 'decided', 'mocked', 'blocked', 'files_touched'],
            properties: {
              clear: { type: 'array' },
              decided: { type: 'array' },
              mocked: { type: 'array' },
              blocked: { type: 'array' },
              files_touched: { type: 'array' },
              unchanged: { type: 'array' },
            },
          },
        },
        // « Deux cerveaux » via DÉBAT (2026-06-13) — le PLAN sort du tier
        // reasoning, PUIS un reviewer (autre famille de modèle) le challenge
        // dans une discussion partagée jusqu'à accord. Remplace l'ancienne
        // boucle plan_review→Goto (relais de fichiers).
        agent_settings: { model: null, tier: 'reasoning', reasoning_effort: null, max_tokens: null },
        multi_agent_review: {
          reviewer_agent: 'Codex',
          reviewer_tier: 'reasoning',
          debate_prompt: t('wiz.preset.feasibilityAutopilot.triageDebatePrompt'),
          max_rounds: 2,
        },
        stall_timeout_secs: 900,
      }),
      // plan_lint (Exec, 0 token) : rapport de forme du plan écrit par le
      // moteur depuis l'envelope validée (plan_lint.txt) — garde déterministe.
      step({
        name: 'plan_lint',
        step_type: { type: 'Exec' },
        description: t('wiz.preset.feasibilityAutopilot.planLintDesc'),
        exec_command: 'bash',
        exec_args: ['-c', "cat .kronn/plan_lint.txt 2>/dev/null || echo 'no lint report (manifest derive missing?)'"],
        exec_timeout_secs: 30,
      }),
      // Étape 3 — Gate humain. Le manifest s'affiche dans
      // gate_message via {{steps.triage.data}}. Sur RequestChanges,
      // boucle vers triage (request_changes_target).
      step({
        name: 'review_triage',
        step_type: { type: 'Gate' },
        description: t('wiz.preset.feasibilityAutopilot.gateDesc'),
        gate_message: t('wiz.preset.feasibilityAutopilot.gateMessage'),
        gate_request_changes_target: 'triage',
      }),
      // Étape 4 — Sous-workflow « fa-implement-verify » : implement →
      // run_tests → drift_check, exécuté comme run ENFANT partageant le
      // worktree du parent (Phase 2). L'enfant lit le manifeste validé
      // dans `.kronn/triage-manifest.md` (écrit par triage côté parent),
      // journalise ses écarts dans `.kronn/decisions.md` et insère les
      // marqueurs KRONN-*. Boucle interne test ERROR → implement. Aucun
      // Gate dedans (le Gate humain reste au parent). Si le run enfant
      // échoue (tests non verts après retries, ou blocage dur),
      // SUBWF_FAILED → re-triage parent (cap 3).
      // test_baseline (2026-06-13) — records tests already RED on the approved
      // base (.kronn/known-failing.txt) so per-task item_tests only loops on
      // NET-NEW failures, never on the repo's pre-existing test debt.
      step({
        name: 'test_baseline',
        step_type: { type: 'Exec' },
        description: t('wiz.preset.feasibilityAutopilot.testBaselineDesc'),
        exec_command: 'bash',
        exec_args: [
          '-c',
          [
            'set +e',
            'main="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)")"',
            'wt="$(git rev-parse --show-toplevel 2>/dev/null)"',
            'hosttr() { printf \'%s\' "${1/#\\/host-home/${KRONN_HOST_HOME:-/host-home}}"; }',
            'mkdir -p .kronn; : > .kronn/known-failing.txt',
            '[ ! -e node_modules ] && [ -d "$main/node_modules" ] && ln -s "$main/node_modules" node_modules 2>/dev/null',
            'if [ -f package.json ] && grep -q \'"test"\' package.json && [ -e node_modules ]; then',
            '  npx --no-install jest --coverage=false >/tmp/base_js.out 2>&1',
            "  grep -oE '●[^\\n]+' /tmp/base_js.out | sed 's/[[:space:]]*$//' | sort -u >> .kronn/known-failing.txt",
            'fi',
            "phpdir=''; for d in . application app; do [ -f \"$d/phpunit.xml.dist\" ] && { phpdir=\"$d\"; break; }; done",
            "compose=''; for c in \"$main/docker-compose.yml\" \"$main/docker-compose.yaml\" \"$main/compose.yml\"; do [ -f \"$c\" ] && { compose=\"$c\"; break; }; done",
            'if [ -n "$phpdir" ] && [ -n "$compose" ] && command -v docker >/dev/null 2>&1; then',
            "  svc=\"$(grep -oE '^  [a-zA-Z0-9_-]+:' \"$compose\" | tr -d ' :' | grep -iE '^php' | head -1)\"; [ -z \"$svc\" ] && svc='php'",
            '  sub="${phpdir#./}"; if [ "$sub" = \'.\' ] || [ -z "$sub" ]; then mnt="$(hosttr "$wt")"; vend="$(hosttr "$main")/vendor"; else mnt="$(hosttr "$wt")/$sub"; vend="$(hosttr "$main")/$sub/vendor"; fi',
            '  docker compose -f "$compose" run --rm --no-deps -T -v "$mnt:/app" -v "$vend:/app/vendor" -w /app "$svc" vendor/bin/phpunit -c phpunit.xml.dist >/tmp/base_php.out 2>&1',
            "  grep -oE '[A-Za-z\\\\]+Test::[a-zA-Z0-9_]+' /tmp/base_php.out | sort -u >> .kronn/known-failing.txt",
            'fi',
            'n=$(wc -l < .kronn/known-failing.txt 2>/dev/null || echo 0)',
            'echo "baseline: $n pre-existing failing test(s) recorded → item_tests will ignore these"',
            "echo '[SIGNAL: OK]'; exit 0",
          ].join('\n'),
        ],
        exec_timeout_secs: 900,
      }),
      step({
        name: 'feasibility_impl',
        step_type: { type: 'SubWorkflow' },
        sub_workflow_id: '@bundle:fa-implement-verify',
        description: t('wiz.preset.feasibilityAutopilot.implStepDesc'),
        on_result: [
          { contains: 'SUBWF_FAILED', action: { type: 'Goto', step_name: 'triage', max_iterations: 3 } },
        ],
      }),
      // run_tests v3 (parent, 0 token) — JS dans le container Kronn (node
      // présent ; --coverage=false car la couverture qui dippe sur les
      // nouveaux fichiers n'est PAS un échec de test, c'est un gate CI) +
      // PHP dans le stack Docker DU PROJET (service php éphémère monté sur le
      // worktree, vendor emprunté au checkout principal — on n'installe rien
      // localement). Verdict honnête par suite, jamais d'early-exit.
      step({
        name: 'run_tests',
        step_type: { type: 'Exec' },
        description: t('wiz.preset.feasibilityAutopilot.runTestsParentDesc'),
        exec_command: 'bash',
        exec_args: [
          '-c',
          [
            'set +e',
            'main="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)")"',
            'wt="$(git rev-parse --show-toplevel 2>/dev/null)"',
            '# bind mounts resolve on the docker HOST — map container path → host path',
            'hosttr() { printf \'%s\' "${1/#\\/host-home/${KRONN_HOST_HOME:-/host-home}}"; }',
            "js='NOT-RUN'; php_v='NOT-RUN'",
            '# ---- JS: node in the Kronn container, jest on the worktree ----',
            'if [ -f package.json ] && grep -q \'"test"\' package.json; then',
            "  [ ! -e node_modules ] && [ -d \"$main/node_modules\" ] && ln -s \"$main/node_modules\" node_modules && echo '→ node_modules symlinked from main checkout'",
            '  if [ -e node_modules ]; then',
            '    if [ -f yarn.lock ] && command -v yarn >/dev/null 2>&1; then yarn test --coverage=false --silent >/tmp/js.out 2>&1; else npm test -- --coverage=false >/tmp/js.out 2>&1; fi',
            '    rc=$?; tail -15 /tmp/js.out',
            "    if grep -qE 'Tests:[^,]*[1-9][0-9]* failed' /tmp/js.out; then js='FAIL'",
            "    elif [ $rc -eq 0 ]; then js='PASS'",
            "    elif grep -qE 'Tests:[^,]*[0-9]+ passed' /tmp/js.out; then js='PASS(non-test exit — lint/coverage gate, CI-enforced)'",
            "    else js='FAIL'; fi",
            "  else js='SKIP(no node_modules — run yarn install in the main checkout)'; fi",
            'fi',
            '# ---- PHP: run in the project dockerized php service, on the worktree ----',
            "phpdir=''; for d in . application app; do [ -f \"$d/phpunit.xml.dist\" ] && { phpdir=\"$d\"; break; }; done",
            "compose=''; for c in \"$main/docker-compose.yml\" \"$main/docker-compose.yaml\" \"$main/compose.yml\"; do [ -f \"$c\" ] && { compose=\"$c\"; break; }; done",
            'if [ -n "$phpdir" ] && [ -n "$compose" ] && command -v docker >/dev/null 2>&1; then',
            "  svc=\"$(grep -oE '^  [a-zA-Z0-9_-]+:' \"$compose\" | tr -d ' :' | grep -iE '^php' | head -1)\"; [ -z \"$svc\" ] && svc='php'",
            '  sub="${phpdir#./}"',
            '  if [ "$sub" = \'.\' ] || [ -z "$sub" ]; then mnt="$(hosttr "$wt")"; vend="$(hosttr "$main")/vendor"; else mnt="$(hosttr "$wt")/$sub"; vend="$(hosttr "$main")/$sub/vendor"; fi',
            '  echo "→ PHP via docker compose service \'$svc\' (worktree mounted, main vendor borrowed)"',
            '  docker compose -f "$compose" run --rm --no-deps -T -v "$mnt:/app" -v "$vend:/app/vendor" -w /app "$svc" vendor/bin/phpunit -c phpunit.xml.dist >/tmp/php.out 2>&1',
            '  rc=$?; tail -20 /tmp/php.out',
            "  # a phpunit 'Tests: N' summary means the suite RAN: rc!=0 = real FAIL;",
            "  # no summary + rc!=0 = harness/boot error (run-11b mis-tag fix)",
            "  if [ $rc -eq 0 ]; then php_v='PASS'",
            "  elif grep -qE 'Tests: [0-9]+' /tmp/php.out; then fails=\"$(grep -oE '(Failures|Errors): [0-9]+' /tmp/php.out | paste -sd, -)\"; php_v=\"FAIL($fails)\"",
            "  elif grep -qE '(No tests executed|Cannot open|could not open|Fatal error|Class .* not found|bootstrap)' /tmp/php.out; then php_v='ERROR(php harness — not a code failure)'",
            "  else php_v='FAIL'; fi",
            'else',
            "  php_v='SKIP(no dockerized php stack at repo root — run \\`make test\\` in the project)'",
            'fi',
            'echo "TEST VERDICT — JS: $js | PHP: $php_v"',
            // Read-only integration verdict: exit 0 always, document in the PR.
            // The per-task test→fix loop is in the CHILD (item_tests), not here.
            "case \"$js$php_v\" in *FAIL*) echo '[SIGNAL: TESTS_FAILED]';; *PASS*) echo '[SIGNAL: OK]';; *) echo '[SIGNAL: SKIPPED]';; esac",
            'exit 0',
          ].join('\n'),
        ],
        exec_timeout_secs: 1500,
      }),
      // drift_check (parent, 0 token) — l'audit des marqueurs tourne sur le
      // worktree FINAL (post fan-out), pas dans l'enfant.
      step({
        name: 'drift_check',
        step_type: { type: 'Exec' },
        description: t('wiz.preset.feasibilityAutopilot.driftCheckDesc'),
        exec_command: 'bash',
        exec_args: [
          '-c',
          [
            'set -e',
            "echo '=== KRONN markers in worktree ==='",
            'echo',
            "if grep -rEn 'KRONN-(ASSUMED|MOCKED|TODO)\\([^)]+\\):' \\",
            "  --include='*.php' --include='*.ts' --include='*.tsx' \\",
            "  --include='*.js' --include='*.jsx' --include='*.rs' \\",
            "  --include='*.py' --include='*.go' --include='*.rb' \\",
            "  --include='*.scss' --include='*.css' --include='*.twig' \\",
            "  --include='*.yaml' --include='*.yml' --include='*.json' \\",
            '  --exclude-dir=node_modules --exclude-dir=vendor \\',
            '  --exclude-dir=target --exclude-dir=.git --exclude-dir=dist \\',
            '  . 2>/dev/null; then',
            '  echo',
            "  echo '(markers above — each one should match a decision_id in the triage manifest)'",
            'else',
            "  echo '(no KRONN-* markers found — implementation is fully clear-category)'",
            'fi',
            'echo',
            'exit 0',
          ].join('\n'),
        ],
        exec_timeout_secs: 60,
      }),
      // Étape finale — pr_draft. Agrège le manifest ({{steps.triage.data}}),
      // le TEST VERDICT (run_tests parent, cité verbatim), l'audit drift
      // (parent, worktree final) et `.kronn/decisions.md`.
      step({
        name: 'pr_draft',
        step_type: { type: 'Agent' },
        description: t('wiz.preset.feasibilityAutopilot.prDraftDesc'),
        prompt_template: t('wiz.preset.feasibilityAutopilot.prDraftPrompt'),
        stall_timeout_secs: 600,
      }),
    ],
    // 2026-06-11 (PR-C) — implement/test/drift loop as a CHILD workflow,
    // referenced by `feasibility_impl` via @bundle. Child inherits the
    // parent's project_id + worktree (Phase 2 handoff) so triage's manifest
    // file + the implementation are visible across the boundary. No Gate
    // (forbidden in a child); the review_triage Gate stays in the parent.
    childWorkflows: [
      {
        bundleId: 'fa-implement-verify',
        name: t('wiz.preset.feasibilityAutopilot.childName'),
        execAllowlist: ['bash', 'cargo', 'pnpm', 'npm', 'yarn', 'pytest', 'make', 'composer', 'grep', 'git'],
        steps: [
          step({
            name: 'implement',
            step_type: { type: 'Agent' },
            description: t('wiz.preset.feasibilityAutopilot.implementDesc'),
            prompt_template: t('wiz.preset.feasibilityAutopilot.implementPrompt'),
            stall_timeout_secs: 1800,
          }),
          // item_tests (per-task) — re-runs the tests scoped to what THIS item
          // changed (php -l + jest --findRelatedTests + scoped phpunit via the
          // project docker stack); red → loops back to implement until green
          // (cap 3). Failures fed back via .kronn/item-test-failures.txt.
          step({
            name: 'item_tests',
            step_type: { type: 'Exec' },
            description: t('wiz.preset.feasibilityAutopilot.itemTestsDesc'),
            exec_command: 'bash',
            exec_args: [
              '-c',
              [
                'set +e',
                'main="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)")"',
                'wt="$(git rev-parse --show-toplevel 2>/dev/null)"',
                'hosttr() { printf \'%s\' "${1/#\\/host-home/${KRONN_HOST_HOME:-/host-home}}"; }',
                'base="$(git merge-base HEAD origin/main 2>/dev/null || git rev-parse HEAD~1 2>/dev/null || git rev-parse HEAD)"',
                'changed="$( { git diff --name-only "$base" 2>/dev/null; git diff --name-only 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | sort -u )"',
                '[ ! -e node_modules ] && [ -d "$main/node_modules" ] && ln -s "$main/node_modules" node_modules 2>/dev/null',
                "fail=0; ts_files=''; php_filter=''",
                'while IFS= read -r f; do',
                '  [ -z "$f" ] && continue',
                '  case "$f" in',
                '    *.php) [ -f "$f" ] && command -v php >/dev/null 2>&1 && { php -l "$f" >/dev/null 2>/tmp/phpl.err || { echo "✗ PHP syntax: $f"; cat /tmp/phpl.err; fail=1; }; }; b=$(basename "$f" .php); case "$f" in *test*|*Test*) php_filter="${php_filter:+$php_filter|}$b";; esac ;;',
                '    *.ts|*.tsx) [ -f "$f" ] && ts_files="$ts_files $f" ;;',
                '    *.json) [ -f "$f" ] && { python3 -m json.tool "$f" >/dev/null 2>&1 || { echo "✗ invalid JSON: $f"; fail=1; }; } ;;',
                '  esac',
                'done <<EOF\n$changed\nEOF',
                'if [ -n "$ts_files" ] && [ -f package.json ] && [ -e node_modules ]; then',
                '  echo "→ jest --findRelatedTests$ts_files"',
                '  npx --no-install jest --findRelatedTests $ts_files --coverage=false --passWithNoTests >/tmp/js.out 2>&1',
                "  [ $? -ne 0 ] && { echo '✗ JS tests red'; tail -25 /tmp/js.out; fail=1; }",
                'fi',
                "phpdir=''; for d in . application app; do [ -f \"$d/phpunit.xml.dist\" ] && { phpdir=\"$d\"; break; }; done",
                "compose=''; for c in \"$main/docker-compose.yml\" \"$main/docker-compose.yaml\" \"$main/compose.yml\"; do [ -f \"$c\" ] && { compose=\"$c\"; break; }; done",
                'if [ -n "$php_filter" ] && [ -n "$phpdir" ] && [ -n "$compose" ] && command -v docker >/dev/null 2>&1; then',
                "  svc=\"$(grep -oE '^  [a-zA-Z0-9_-]+:' \"$compose\" | tr -d ' :' | grep -iE '^php' | head -1)\"; [ -z \"$svc\" ] && svc='php'",
                '  sub="${phpdir#./}"; if [ "$sub" = \'.\' ] || [ -z "$sub" ]; then mnt="$(hosttr "$wt")"; vend="$(hosttr "$main")/vendor"; else mnt="$(hosttr "$wt")/$sub"; vend="$(hosttr "$main")/$sub/vendor"; fi',
                '  echo "→ scoped phpunit --filter ($php_filter) via docker ($svc)"',
                '  docker compose -f "$compose" run --rm --no-deps -T -v "$mnt:/app" -v "$vend:/app/vendor" -w /app "$svc" vendor/bin/phpunit -c phpunit.xml.dist --filter "($php_filter)" >/tmp/php.out 2>&1',
                "  rc=$?; if [ $rc -ne 0 ] && grep -qE 'Tests: [0-9]+' /tmp/php.out; then echo '✗ PHP tests red'; tail -30 /tmp/php.out; fail=1; fi",
                'fi',
                '# baseline-aware: loop to implement ONLY on NET-NEW failures (not the repo pre-existing debt in .kronn/known-failing.txt)',
                'if [ "$fail" = 1 ]; then',
                "  { grep -oE '●[^\\n]+' /tmp/js.out 2>/dev/null | sed 's/[[:space:]]*$//'; grep -oE '[A-Za-z\\\\]+Test::[a-zA-Z0-9_]+' /tmp/php.out 2>/dev/null; } | sort -u > /tmp/cur_fail.txt",
                '  if [ -s .kronn/known-failing.txt ]; then netnew="$(grep -vxF -f .kronn/known-failing.txt /tmp/cur_fail.txt)"; else netnew="$(cat /tmp/cur_fail.txt)"; fi',
                '  if [ -n "$netnew" ]; then',
                "    { echo '=== YOUR change broke these tests — FIX them ==='; echo \"$netnew\"; echo; echo '=== pre-existing failures (NOT yours — do NOT chase) ==='; grep -xF -f .kronn/known-failing.txt /tmp/cur_fail.txt 2>/dev/null; echo; echo '--- JS tail ---'; tail -25 /tmp/js.out 2>/dev/null; echo '--- PHP tail ---'; tail -30 /tmp/php.out 2>/dev/null; } > .kronn/item-test-failures.txt",
                '    echo "item_tests FAILED — $(printf \'%s\\n\' "$netnew" | grep -c .) NET-NEW failure(s) — looping back to implement"; exit 2',
                '  fi',
                "  echo 'item_tests: all failures are PRE-EXISTING repo debt — not looping'",
                'fi',
                'rm -f .kronn/item-test-failures.txt 2>/dev/null',
                "echo '[SIGNAL: OK]'; exit 0",
              ].join('\n'),
            ],
            exec_timeout_secs: 600,
            on_result: [
              { contains: 'exit_2', action: { type: 'Goto', step_name: 'implement', max_iterations: 3 } },
            ],
          }),
          // Phase 3a — deterministic verification layer (0 token):
          // scope_check (advisory) flags out-of-scope edits; completeness_check
          // (enforcing) loops back to implement if a manifest id has no marker.
          scopeCheckStep(t),
          completenessCheckStep(t),
          // commit the validated Phase-0 implementation to the parent branch
          // (survives worktree cleanup) — last step of the child.
          commitStep(t),
        ],
      },
    ],
  };

  return [AUTO_DEV, PR_GATE, DEPLOY_ROLLBACK, FEATURE_PLANNER, DAILY_HOST_AUDIT, TICKET_TO_PR, FEASIBILITY_AUTOPILOT];
}
