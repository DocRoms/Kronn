/**
 * KT-1923 — the fixture of the Settings sub-page specs: the props, the render
 * wrapper and the control counts. With the api module of `settingsApiMock.ts`,
 * one fixture, so the inventory of the base commit and the one of the
 * sub-pages are taken on the same data.
 */
import { vi } from 'vitest';
import { render, act } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';
import { TourProvider } from '../../components/tour/TourProvider';
import type { AgentDetection, AgentsConfig } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';

const noop = () => {};

export const settingsToast: ToastFn = vi.fn();

const detected = (name: string, agent_type: AgentDetection['agent_type']): AgentDetection => ({
  name, agent_type, installed: true, enabled: true, path: '/usr/bin/agent', version: '1.0.0', latest_version: null,
  origin: 'host', install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
});

/** One CLI agent and the local one, so the agents and the local models both have a card. */
export const settingsAgents: AgentDetection[] = [detected('Claude Code', 'ClaudeCode'), detected('Ollama', 'Ollama')];

export const settingsProps = {
  agents: settingsAgents,
  agentAccess: null as AgentsConfig | null,
  configLanguage: null as string | null,
  projects: [],
  refetchAgents: noop,
  refetchAgentAccess: noop,
  refetchLanguage: noop,
  refetchProjects: noop,
  refetchDiscussions: noop,
  onReset: noop,
  toast: settingsToast,
  hasConfiguredApi: true,
};

/** Renders inside the providers the page reads, then lets the loads settle. */
export async function renderSettings(ui: ReactElement): Promise<void> {
  await act(async () => {
    render(
      <I18nProvider>
        <TourProvider setPage={noop}>{ui}</TourProvider>
      </I18nProvider>,
    );
  });
  for (let i = 0; i < 3; i += 1) {
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  }
}

/** The controls a reader can reach in one region: buttons, fields, links. */
export interface ControlCount { buttons: number; fields: number; links: number }

const BUTTONS = 'button, [role="button"]:not(button)';
const FIELDS = 'input, select, textarea';
const LINKS = 'a[href]';

/** Counts the controls of a region, leaving out the ones inside `exclude`. */
export function countControls(region: Element, exclude: readonly Element[] = []): ControlCount {
  const keep = (el: Element) => !exclude.some(outside => outside.contains(el));
  const count = (selector: string) => [...region.querySelectorAll(selector)].filter(keep).length;
  return { buttons: count(BUTTONS), fields: count(FIELDS), links: count(LINKS) };
}

/** The accessible label of every field of a region (aria-label, or its <label>). */
export function fieldLabels(region: Element): string[] {
  return [...region.querySelectorAll<HTMLElement>(FIELDS)].map(field => {
    const aria = field.getAttribute('aria-label');
    if (aria) return aria;
    const by = field.getAttribute('aria-labelledby');
    if (by) return by.split(' ').map(id => document.getElementById(id)?.textContent ?? '').join(' ').trim();
    const id = field.id;
    const label = (id && document.querySelector(`label[for="${CSS.escape(id)}"]`)) || field.closest('label');
    return label?.textContent?.trim() ?? '';
  });
}
