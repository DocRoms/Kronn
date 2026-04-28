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
  id: 'auto-dev' | 'pr-gate' | 'deploy-rollback' | 'feature-planner' | 'daily-host-audit';
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

  return [AUTO_DEV, PR_GATE, DEPLOY_ROLLBACK, FEATURE_PLANNER, DAILY_HOST_AUDIT];
}
