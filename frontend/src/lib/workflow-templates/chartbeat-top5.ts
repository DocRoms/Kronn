// Starter workflow template — "Chartbeat top 5 → Agent résumé → Slack notify"
// =============================================================================
//
// This is the **aha moment** of the désagentification feature (see
// `ai/operations/deagent-apicall.md`). A one-click clone that shows an
// ApiCall step feeding an Agent step, with the BatchQuickPrompt-compatible
// shape, all in a single pre-wired flow. Users copy, plug their Chartbeat
// host + API key, save. End-to-end demo in ~30 seconds.
//
// The template is intentionally minimal:
//   1. ApiCall step hits Chartbeat `/live/toppages/v4`, extracts the top 5
//      titles (array-shaped → friendly to any downstream step).
//   2. Agent step summarises the list with a prompt referencing
//      `{{steps.fetch_top_pages.data}}`.
//   3. Notify step posts the summary to Slack (user edits the webhook URL).
//
// The wizard consumes this via a "Load template" button (to be wired in
// P1.2b); unit-tested here so the shape can't silently drift under us.

import type { Workflow, WorkflowStep } from '../../types/generated';

/** Pre-wired `ApiCall` step targeting Chartbeat's top-pages endpoint.
 *  `api_plugin_slug` stays `"chartbeat"` — the wizard re-injects the
 *  user's `api_config_id` on clone. */
export const CHARTBEAT_FETCH_TOP_PAGES: WorkflowStep = {
  name: 'fetch_top_pages',
  step_type: { type: 'ApiCall' },
  description: 'Top 5 articles en direct (Chartbeat)',
  agent: 'ClaudeCode', // Required by the model but ignored for ApiCall steps.
  prompt_template: '',
  mode: { type: 'Normal' },
  output_format: { type: 'Structured' },
  mcp_config_ids: [],
  agent_settings: null,
  on_result: [],
  stall_timeout_secs: null,
  retry: null,
  delay_after_secs: null,
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
  api_plugin_slug: 'chartbeat',
  api_config_id: null, // Injected by the wizard on clone.
  api_endpoint_path: '/live/toppages/v4',
  api_method: 'GET',
  api_query: { limit: '5' },
  api_headers: null,
  api_body: null,
  api_extract: {
    path: '$.pages[*].title',
    fallback: [],
    fail_on_empty: true,
  },
  api_pagination: { type: 'None' },
  api_timeout_ms: 15000,
  api_max_retries: 2,
  api_output_var: 'fetch_top_pages',
};

/** Agent summary step that consumes the extracted titles. */
export const AGENT_SUMMARIZE: WorkflowStep = {
  name: 'summarize',
  step_type: { type: 'Agent' },
  description: 'Synthèse rédactionnelle des titres',
  agent: 'ClaudeCode',
  prompt_template:
    'Voici les 5 titres les plus consultés à l\'instant :\n\n' +
    '{{steps.fetch_top_pages.data}}\n\n' +
    'Génère un paragraphe de 2-3 phrases qui met en perspective ces titres : ' +
    'angles communs, thématiques dominantes, éléments remarquables. Style télégraphique.',
  mode: { type: 'Normal' },
  output_format: { type: 'FreeText' },
  mcp_config_ids: [],
  agent_settings: null,
  on_result: [],
  stall_timeout_secs: null,
  retry: null,
  delay_after_secs: null,
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
  api_headers: null,
  api_body: null,
  api_extract: null,
  api_pagination: null,
  api_timeout_ms: null,
  api_max_retries: null,
  api_output_var: null,
};

/** Notify step — posts the synthesis to Slack. User must replace the
 *  placeholder URL. */
export const NOTIFY_SLACK: WorkflowStep = {
  name: 'notify_slack',
  step_type: { type: 'Notify' },
  description: 'Notification Slack',
  agent: 'ClaudeCode',
  prompt_template: '',
  mode: { type: 'Normal' },
  output_format: { type: 'FreeText' },
  mcp_config_ids: [],
  agent_settings: null,
  on_result: [],
  stall_timeout_secs: null,
  retry: null,
  delay_after_secs: null,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  batch_quick_prompt_id: null,
  batch_items_from: null,
  batch_wait_for_completion: null,
  batch_max_items: null,
  batch_workspace_mode: null,
  batch_chain_prompt_ids: [],
  notify_config: {
    url: 'https://hooks.slack.com/services/XXX/YYY/ZZZ',
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body_template:
      '{"text": "🔥 Top Chartbeat du jour\\n\\n{{steps.summarize.output}}"}',
  },
  api_plugin_slug: null,
  api_config_id: null,
  api_endpoint_path: null,
  api_method: null,
  api_query: null,
  api_headers: null,
  api_body: null,
  api_extract: null,
  api_pagination: null,
  api_timeout_ms: null,
  api_max_retries: null,
  api_output_var: null,
};

/** Full workflow skeleton. `id` + `project_id` are stamped by the wizard at
 *  clone time so the template itself stays project-agnostic. */
export interface StarterTemplate {
  id: string;
  title_fr: string;
  title_en: string;
  description_fr: string;
  description_en: string;
  /** Slug of the primary plugin this template showcases — used by the
   *  wizard to pick the right `api_config_id` on clone. */
  primary_plugin_slug: string;
  steps: WorkflowStep[];
}

export const CHARTBEAT_TOP5_TEMPLATE: StarterTemplate = {
  id: 'chartbeat-top5-to-slack',
  title_fr: 'Chartbeat top 5 → Résumé IA → Slack',
  title_en: 'Chartbeat top 5 → AI summary → Slack',
  description_fr:
    'Démo désagentification : récupère les 5 articles les plus consultés via ' +
    'l\'API Chartbeat (0 token), un agent les résume, le résultat est posté sur Slack.',
  description_en:
    'Désagentification demo: fetches the 5 most-read articles from the ' +
    'Chartbeat API (0 tokens), an agent summarises them, the result is posted to Slack.',
  primary_plugin_slug: 'chartbeat',
  steps: [
    CHARTBEAT_FETCH_TOP_PAGES,
    AGENT_SUMMARIZE,
    NOTIFY_SLACK,
  ],
};

/** Every starter template the wizard exposes. Keeping them in one array
 *  means the "Load template" picker doesn't need to enumerate files.
 *  Future templates (Jira → Agent → Jira transition, CF anomaly → ticket,
 *  etc.) drop in here. */
export const STARTER_TEMPLATES: StarterTemplate[] = [
  CHARTBEAT_TOP5_TEMPLATE,
];

/** Clone helper — returns a deep copy of the template's steps ready to
 *  be inserted into a fresh workflow. Injects the caller-supplied
 *  `api_config_id` into every ApiCall step that matches
 *  `primary_plugin_slug`, so the user doesn't have to re-pick the plugin
 *  instance for each step. Returns `null` when the template is missing a
 *  matching config. */
export function cloneTemplateSteps(
  template: StarterTemplate,
  apiConfigIdForPrimary: string | null,
): WorkflowStep[] {
  return template.steps.map(step => {
    const cloned: WorkflowStep = JSON.parse(JSON.stringify(step));
    if (
      cloned.step_type?.type === 'ApiCall' &&
      cloned.api_plugin_slug === template.primary_plugin_slug
    ) {
      cloned.api_config_id = apiConfigIdForPrimary;
    }
    return cloned;
  });
}

/** Sanity check invariants the wizard relies on. Exercised by the
 *  `chartbeat-top5.test.ts` test file; lives next to the template so a
 *  drift is caught before the wizard's "Load" button silently produces
 *  a broken step. */
export function assertTemplateInvariants(template: StarterTemplate): string[] {
  const errors: string[] = [];
  if (template.steps.length === 0) errors.push('template has no steps');
  for (const step of template.steps) {
    if (!step.name) errors.push(`step missing name`);
    if (step.step_type?.type === 'ApiCall') {
      if (!step.api_plugin_slug) errors.push(`ApiCall step "${step.name}" missing api_plugin_slug`);
      if (!step.api_endpoint_path) errors.push(`ApiCall step "${step.name}" missing api_endpoint_path`);
      if (!step.api_extract) errors.push(`ApiCall step "${step.name}" missing api_extract`);
    }
    if (step.step_type?.type === 'Notify' && !step.notify_config) {
      errors.push(`Notify step "${step.name}" missing notify_config`);
    }
  }
  return errors;
}

/** Re-exported so the wizard (to be wired) imports the `Workflow` type
 *  alongside the template without a second import line. */
export type { Workflow };
