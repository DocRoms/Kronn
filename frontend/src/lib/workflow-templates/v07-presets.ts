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
    primitives: ['JsonData', 'Agent', 'Exec', 'Gate', 'Loop', 'State', 'Notify'],
    // `bash` lets `run_tests` use a generic detection script that adapts to
    // any project (Cargo / npm-pnpm-yarn / composer / pytest / make).
    // Without it, the preset would only work on Rust projects.
    execAllowlist: ['bash', 'cargo', 'pnpm', 'npm', 'yarn', 'pytest', 'make', 'composer'],
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
      // Étape 4 — Implémentation. Combo de 4 skills méthodologiques :
      //   - tdd : red-green-refactor strict, no production code without
      //     a failing test first
      //   - systematic-debugging : root-cause à 4 phases quand un test
      //     casse en milieu de loop
      //   - verification-before-completion : interdit les "ça devrait
      //     marcher" sans evidence
      //   - receiving-code-review : si on revient ici depuis le step
      //     `review` avec NEEDS_CHANGES, l'agent sait quoi faire de
      //     `state.last_review` (technique pour appliquer feedback).
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
        // One auto-retry on heavy implements. Claude Code CLI can exit
        // silently (`exit 1`, no stderr) on long-running streamed sessions
        // — the failure mode looks deterministic from Kronn's side but is
        // intermittent from the CLI's, so a fresh retry typically gets
        // through. The agent sees the partial worktree state from the
        // first attempt; the prompt's "if review left feedback" branch
        // doubles as a "if a previous attempt left files modified" cue.
        retry: { max_retries: 1, backoff: 'exponential' },
        stall_timeout_secs: 1800,
      }),
      // Étape 5 — Tests. Generic auto-detect: probes the worktree for the
      // most likely test framework (Make / Cargo / pnpm-yarn-npm / composer /
      // pytest) and runs it. Falls back to a soft skip with `[SIGNAL: SKIPPED]`
      // when no framework matches OR the worktree's deps aren't installed —
      // this keeps the workflow alive on fresh worktrees rather than blocking
      // every run on an environment problem the workflow can't solve itself.
      // Sur ERROR, retour à implement (max 2 cycles — tokens count up).
      step({
        name: 'run_tests',
        step_type: { type: 'Exec' },
        description: t('wiz.preset.shared.runTestsDesc'),
        exec_command: 'bash',
        exec_args: [
          '-c',
          [
            'set -e',
            // Make has highest priority — projects with a Makefile usually
            // have an opinionated `test` target that already wires up linters,
            // type-checks, the right test runner with the right env vars.
            "if [ -f Makefile ] && grep -qE '^test:' Makefile; then echo '→ make test'; exec make test; fi",
            // Rust: cargo test --lib (skip integration crate tests by default
            // — they often need extra setup; the workflow author can edit).
            "if [ -f Cargo.toml ]; then echo '→ cargo test --lib'; exec cargo test --lib; fi",
            // JS/TS: respect the lockfile to pick the package manager. If the
            // worktree is fresh (no node_modules), skip rather than fail —
            // installing deps is project-specific and out of scope here.
            'if [ -f package.json ] && grep -q \'"test"\' package.json; then',
            '  if [ ! -d node_modules ]; then echo "⚠ node_modules absent dans le worktree — skip"; echo "[SIGNAL: SKIPPED]"; exit 0; fi',
            "  if [ -f pnpm-lock.yaml ]; then echo '→ pnpm test'; exec pnpm test",
            "  elif [ -f yarn.lock ]; then echo '→ yarn test'; exec yarn test",
            "  else echo '→ npm test'; exec npm test; fi",
            'fi',
            // PHP: composer test (or vendor/bin/phpunit fallback).
            'if [ -f composer.json ]; then',
            '  if [ ! -d vendor ]; then echo "⚠ vendor/ absent — skip"; echo "[SIGNAL: SKIPPED]"; exit 0; fi',
            '  if grep -q \'"test"\' composer.json; then echo "→ composer test"; exec composer test',
            "  elif [ -x vendor/bin/phpunit ]; then echo '→ vendor/bin/phpunit'; exec vendor/bin/phpunit",
            "  else echo 'no PHP test runner'; echo '[SIGNAL: SKIPPED]'; exit 0; fi",
            'fi',
            // Python: pytest if pyproject/setup.py exists.
            "if [ -f pyproject.toml ] || [ -f setup.py ]; then echo '→ pytest'; exec pytest; fi",
            // Nothing matched — soft skip.
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
      // Étape 6 — Review par un agent (idéalement DIFFÉRENT de celui
      // qui a implémenté, anti confirmation-bias). Le user peut
      // changer `agent` après création du préset. `requesting-code-review`
      // structure la review (priorité, blocking issues, YAGNI check).
      // Sur NEEDS_CHANGES, écrit `state.last_review` puis Goto implement.
      step({
        name: 'review',
        step_type: { type: 'Agent' },
        output_format: { type: 'Structured' },
        prompt_template: t('wiz.preset.ticketToPr.reviewPrompt'),
        skill_ids: [
          'requesting-code-review',
          'verification-before-completion',
        ],
        // Same auto-retry as `implement` — review on a heavy implementation
        // also runs long enough to hit the Claude Code CLI silent-exit
        // pattern. One retry is cheap insurance; subsequent attempts read
        // the same artifacts so the verdict converges.
        retry: { max_retries: 1, backoff: 'exponential' },
        stall_timeout_secs: 1800,
        // Only NEEDS_CHANGES has an explicit action (loop back to implement).
        // APPROVED has no rule because the natural fall-through (continue to
        // the next step `create_pr`) is exactly what we want — adding a Stop
        // rule here would terminate the workflow prematurely. Earlier preset
        // versions had `APPROVED → Stop` which silently skipped create_pr,
        // ready_gate and notify_done.
        on_result: [
          { contains: 'NEEDS_CHANGES', action: { type: 'Goto', step_name: 'implement', max_iterations: 2 } },
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
        gate_request_changes_target: 'implement',
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
    primitives: ['JsonData', 'Agent', 'Gate', 'Exec'],
    execAllowlist: ['bash', 'cargo', 'pnpm', 'npm', 'yarn', 'pytest', 'make', 'composer', 'grep'],
    requireIsolation: true,
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
            },
          },
        },
        stall_timeout_secs: 900,
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
      // Étape 4 — Implémentation. Contraint par le manifest validé.
      // [SIGNAL: BLOCKED <id>] → Goto(triage) si l'agent découvre
      // une impossibilité en cours (cap 3 itérations).
      step({
        name: 'implement',
        step_type: { type: 'Agent' },
        description: t('wiz.preset.feasibilityAutopilot.implementDesc'),
        prompt_template: t('wiz.preset.feasibilityAutopilot.implementPrompt'),
        stall_timeout_secs: 1800,
        on_result: [
          { contains: 'BLOCKED', action: { type: 'Goto', step_name: 'triage', max_iterations: 3 } },
        ],
      }),
      // Étape 5 — run_tests Exec. Réutilise le pattern auto-detect
      // de ticket-to-pr. 0 token, vrai verdict. ERROR → Goto
      // implement (cap 2 itérations).
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
            '  if [ ! -d node_modules ]; then echo "⚠ node_modules absent — skip"; echo "[SIGNAL: SKIPPED]"; exit 0; fi',
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
      // Étape 6 — drift_check Exec. Grep des markers KRONN-* sur le
      // worktree. 0 token. Sortie embarquée verbatim dans la PR
      // par pr_draft via {{steps.drift_check.output}}.
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
      // Étape 7 — pr_draft. Génère la PR body en agrégeant manifest +
      // run_tests + drift_check.
      step({
        name: 'pr_draft',
        step_type: { type: 'Agent' },
        description: t('wiz.preset.feasibilityAutopilot.prDraftDesc'),
        prompt_template: t('wiz.preset.feasibilityAutopilot.prDraftPrompt'),
        stall_timeout_secs: 600,
      }),
    ],
  };

  return [AUTO_DEV, PR_GATE, DEPLOY_ROLLBACK, FEATURE_PLANNER, DAILY_HOST_AUDIT, TICKET_TO_PR, FEASIBILITY_AUTOPILOT];
}
