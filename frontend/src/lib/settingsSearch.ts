import {
  SETTINGS_SUB_PAGES,
  settingsSectionOf,
  settingsSectionsOf,
  type SettingsFieldDef,
  type SettingsSectionDef,
  type SettingsSubPage,
} from './routes';

/** The sections of a sub-page the reader can open, given what this Kronn has. */
export function visibleSettingsSections(page: SettingsSubPage, hasConfiguredApi: boolean): SettingsSectionDef[] {
  return settingsSectionsOf(page).filter(section => !section.requiresConfiguredApi || hasConfiguredApi);
}

/** One hit of the Settings search: a sub-page, a section, one of its settings or one of its agents. */
export type SettingsSearchHit =
  | {
    kind: 'page';
    page: SettingsSubPage;
    /** Its name, in the current locale. */
    label: string;
  }
  | {
    kind: 'section' | 'field' | 'agent';
    page: SettingsSubPage;
    section: SettingsSectionDef;
    /** What matched, in the current locale: the section's name, a setting's label or an agent's name. */
    label: string;
    /** Where the hit leads: the section, the fold that holds the setting, or the agent's row. */
    anchor: string;
  };

/** An agent the Settings draw a row for, by its name. */
export interface SettingsSearchAgent {
  agent_type: string;
  name: string;
  /** Its row can be opened (it is installed, or its runtime answers). */
  configurable: boolean;
}

export interface SettingsSearchOptions {
  /** The API call log section exists (an API plugin has a config). */
  hasConfiguredApi?: boolean;
  /** The agents of this Kronn: each is found by its name, on its row. */
  agents?: readonly SettingsSearchAgent[];
}

/** Lower case, accents and repeated spaces dropped, so « Thème » matches "theme". */
export function normalizeSettingsText(text: string): string {
  return text.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase().replace(/\s+/g, ' ').trim();
}

/** The agents the Agents section shows as connections, not rows (KT-339). */
const CONNECTION_AGENTS: readonly string[] = ['LiteLlm', 'Nvidia'];

/** The anchor an agent's name leads to: its row, or the local models for the local agent. */
export function settingsAgentAnchor(agent: SettingsSearchAgent): string {
  if (agent.agent_type === 'Ollama') return 'settings-local-models';
  return agent.configurable ? `agent-config-${agent.agent_type}` : 'settings-agent-config';
}

const fieldKeys = (field: SettingsFieldDef): { label: string; also: readonly string[] } =>
  typeof field === 'string' ? { label: field, also: [] } : { label: field.label, also: field.also };

/**
 * KT-1923 — the sub-pages, the sections, the settings and the agents whose
 * name holds every word of the query, in the current locale (`t`), in the
 * order of the pages. A setting is also found by the words of the hint drawn
 * in its label (`also`), and shown by its label. A name given twice in one
 * section is one hit.
 */
export function searchSettings(
  query: string,
  t: (key: string) => string,
  { hasConfiguredApi = false, agents = [] }: SettingsSearchOptions = {},
): SettingsSearchHit[] {
  const words = normalizeSettingsText(query).split(' ').filter(Boolean);
  if (words.length === 0) return [];
  const matches = (...texts: string[]) => {
    const text = normalizeSettingsText(texts.join(' '));
    return words.every(word => text.includes(word));
  };
  const hits: SettingsSearchHit[] = [];
  const agentHits = agents
    .filter(agent => !CONNECTION_AGENTS.includes(agent.agent_type))
    .map(agent => ({ agent, anchor: settingsAgentAnchor(agent) }));
  for (const page of SETTINGS_SUB_PAGES) {
    const pageName = t(`settings.pages.${page}.title`);
    if (matches(pageName)) hits.push({ kind: 'page', page, label: pageName });
    for (const section of settingsSectionsOf(page)) {
      if (section.requiresConfiguredApi && !hasConfiguredApi) continue;
      const title = t(section.title);
      const seen = new Set<string>([normalizeSettingsText(title)]);
      if (matches(title)) hits.push({ kind: 'section', page, section, label: title, anchor: section.id });
      const fields = [
        ...section.fields.map(field => ({ ...fieldKeys(field), anchor: section.id })),
        ...(section.folds ?? []).flatMap(fold => fold.fields.map(field => ({ ...fieldKeys(field), anchor: fold.id }))),
      ];
      for (const { label: key, also, anchor } of fields) {
        const label = t(key);
        const normalized = normalizeSettingsText(label);
        if (seen.has(normalized)) continue;
        seen.add(normalized);
        if (matches(label, ...also.map(hint => t(hint)))) hits.push({ kind: 'field', page, section, label, anchor });
      }
      for (const { agent, anchor } of agentHits) {
        if (settingsSectionOf(anchor) !== section) continue;
        const normalized = normalizeSettingsText(agent.name);
        if (seen.has(normalized)) continue;
        seen.add(normalized);
        if (matches(agent.name)) hits.push({ kind: 'agent', page, section, label: agent.name, anchor });
      }
    }
  }
  return hits;
}
