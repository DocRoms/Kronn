// ProjectCard — header accessibility regression.
//
// The card header used to be a `<button>` containing a nested `<button>`
// (the drift "Update stale sections" CTA). That's invalid HTML and
// produced a React dev warning. We converted the outer wrapper to
// `<div role="button" tabIndex={0}>` with explicit Enter/Space keyboard
// handlers, while keeping the same toggle behaviour.
//
// This test pins the contract:
// 1. The header is keyboard-focusable.
// 2. Pressing Enter or Space toggles the card.
// 3. The header is not a `<button>` element (would create the nested-
//    button HTML violation that this fix removed).

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, act } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { ProjectCard } from '../ProjectCard';
import type { Project, AgentDetection } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-1',
  name: 'demo',
  path: '/repos/demo',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code',
  agent_type: 'ClaudeCode',
  installed: true,
  enabled: true,
  path: '/usr/bin/claude',
  version: '1.0.0',
  latest_version: null,
  origin: 'host',
  install_command: null,
  host_managed: false,
  host_label: null,
  runtime_available: false,
  rtk_available: false,
  rtk_hook_configured: false,
};

function renderCard(opts: { onToggleOpen?: () => void } = {}) {
  return render(
    <ProjectCard
      project={PROJECT}
      isOpen={false}
      onToggleOpen={opts.onToggleOpen ?? noop}
      discussions={[]}
      driftStatus={undefined}
      agents={[AGENT]}
      allSkills={[]}
      mcpConfigs={[]}
      workflows={[]}
      configLanguage="fr"
      toast={vi.fn()}
      onNavigate={noop}
      onSetDiscPrefill={noop}
      onAutoRunDiscussion={noop}
      onOpenDiscussion={noop}
      onRefetch={noop}
      onRefetchDiscussions={noop}
      onRefetchSkills={noop}
      onRefetchDrift={noop}
    />
  );
}

describe('ProjectCard — header accessibility', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('header uses div role=button (not a real <button>) to avoid nested-button HTML violation', () => {
    renderCard();
    const header = document.querySelector('.dash-card-header');
    expect(header).not.toBeNull();
    // Must NOT be an actual <button> element — that's the bug we fixed
    // (would create a nested-button HTML violation with the inner drift
    // update button).
    expect(header!.tagName).toBe('DIV');
    expect(header!.getAttribute('role')).toBe('button');
    expect(header!.getAttribute('tabindex')).toBe('0');
  });

  it('clicking the header calls onToggleOpen', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header')!;
    act(() => { fireEvent.click(header); });
    expect(onToggleOpen).toHaveBeenCalledTimes(1);
  });

  it('Enter key on the header toggles the card', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    act(() => { fireEvent.keyDown(header, { key: 'Enter' }); });
    expect(onToggleOpen).toHaveBeenCalledTimes(1);
  });

  it('Space key on the header toggles the card', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    act(() => { fireEvent.keyDown(header, { key: ' ' }); });
    expect(onToggleOpen).toHaveBeenCalledTimes(1);
  });

  it('Tab key on the header does not toggle (browser handles focus)', () => {
    const onToggleOpen = vi.fn();
    renderCard({ onToggleOpen });
    const header = document.querySelector('.dash-card-header') as HTMLElement;
    act(() => { fireEvent.keyDown(header, { key: 'Tab' }); });
    expect(onToggleOpen).not.toHaveBeenCalled();
  });
});
