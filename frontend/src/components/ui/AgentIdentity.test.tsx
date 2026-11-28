// KT-1936 — AgentIdentity: the agent's name in its colour, its model (an id
// or an explicit unknown state, never a blank or a tier name), its role as a
// separate chip, in three sizes; no text under the 12 px floor. Written before
// the component.
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { AgentIdentity } from './AgentIdentity';
import {
  AGENT_IDENTITY_MODEL_MAX_CHARS, AGENT_IDENTITY_ROLES, AGENT_IDENTITY_UNKNOWN_STATES,
  agentIdentityText, agentModel, agentModelText,
} from './agentIdentityKinds';
import { cssRules, declarations } from '../../styles/themeAudit';
import { describeViolations, runAxe } from '../../test/axe';

const CSS = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'AgentIdentity.css'), 'utf8');
const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}(${args.join(',')})` : key);

describe('AgentIdentity', () => {
  it('writes the name in the colour it is given, then the model id, then the role chip', () => {
    const { container } = render(
      <AgentIdentity name="Claude Code" color="var(--kr-agent-claude-text)" model={{ kind: 'id', id: 'claude-opus-4-5' }} role="main" />,
    );
    const root = container.firstElementChild as HTMLElement;
    expect(root).toHaveAttribute('data-size', 'inline');
    expect(root).toHaveAttribute('data-role', 'main');
    expect(screen.getByText('Claude Code').style.color).toBe('var(--kr-agent-claude-text)');
    expect(screen.getByTestId('agent-identity-model')).toHaveTextContent('claude-opus-4-5');
    expect(screen.getByTestId('agent-identity-model')).toHaveAttribute('data-state', 'id');
    const chip = container.querySelector('.ui-agent-identity-role')!;
    expect(chip).toHaveClass('ui-status-badge');
    expect(chip).toHaveTextContent('ui.agentIdentity.role.main');
    // Reading order: name, model, role.
    expect(root.textContent).toBe('Claude Code · claude-opus-4-5 · ui.agentIdentity.role.main');
  });

  it.each(AGENT_IDENTITY_UNKNOWN_STATES)('writes the unknown state %s in words, never a blank', (state) => {
    render(<AgentIdentity name="Ollama" model={{ kind: state }} />);
    const model = screen.getByTestId('agent-identity-model');
    expect(model).toHaveAttribute('data-state', state);
    expect(model.textContent).toBe(`ui.agentIdentity.model.${state}`);
  });

  it('does not repeat the agent in the provider default state: its name is written just before', () => {
    expect(agentModelText({ kind: 'providerDefault' }, t)).toBe('ui.agentIdentity.model.providerDefault');
    expect(agentModel('  ', 'notRecorded')).toEqual({ kind: 'notRecorded' });
    expect(agentModel(' qwen3:32b ', 'notRecorded')).toEqual({ kind: 'id', id: 'qwen3:32b' });
  });

  it.each(AGENT_IDENTITY_ROLES)('labels the role %s through its own key', (role) => {
    const { container } = render(<AgentIdentity name="Codex" model={{ kind: 'id', id: 'gpt-5' }} role={role} />);
    expect(container.querySelector('.ui-agent-identity-role')).toHaveTextContent(`ui.agentIdentity.role.${role}`);
  });

  it('writes a role label in place of the role key, and no chip without a role', () => {
    const { container, rerender } = render(<AgentIdentity name="Codex" model={{ kind: 'id', id: 'gpt-5' }} role="oneOff" roleLabel="Démo" />);
    expect(container.querySelector('.ui-agent-identity-role')).toHaveTextContent('Démo');
    rerender(<AgentIdentity name="Codex" model={{ kind: 'id', id: 'gpt-5' }} />);
    expect(container.querySelector('.ui-agent-identity-role')).toBeNull();
  });

  it('shows the tier icon, named when a label is given', () => {
    const { rerender, container } = render(<AgentIdentity name="Codex" model={{ kind: 'id', id: 'gpt-5' }} tier="reasoning" tierLabel="Raisonnement" />);
    expect(screen.getByRole('img', { name: 'Raisonnement' })).toHaveTextContent('🧠');
    rerender(<AgentIdentity name="Codex" model={{ kind: 'id', id: 'gpt-5' }} tier="economy" />);
    expect(container.querySelector('.ui-agent-identity-tier')).toHaveAttribute('aria-hidden', 'true');
  });

  it('compact: the model and the role leave the screen but stay in the accessible name', () => {
    render(
      <button type="button">
        <AgentIdentity size="compact" name="Codex" model={{ kind: 'id', id: 'gpt-5-codex' }} role="main" />
      </button>,
    );
    expect(screen.queryByTestId('agent-identity-model')).toBeNull();
    const hidden = document.querySelector('.ui-visually-hidden')!;
    expect(hidden.textContent).toBe(' · gpt-5-codex · ui.agentIdentity.role.main');
    expect(screen.getByRole('button').textContent).toBe('Codex · gpt-5-codex · ui.agentIdentity.role.main');
  });

  it('full and inline write every piece; truncateModel marks the model for the ellipsis', () => {
    const { container } = render(<AgentIdentity size="full" truncateModel name="Codex" model={{ kind: 'id', id: 'a-very-long-model-identifier-1' }} />);
    expect(container.firstElementChild).toHaveAttribute('data-size', 'full');
    // The full id stays in the DOM: only the box is cut.
    expect(screen.getByTestId('agent-identity-model')).toHaveTextContent('a-very-long-model-identifier-1');
    expect(screen.getByTestId('agent-identity-model')).toHaveAttribute('data-truncate', 'true');
  });

  it('cuts a truncated model past 17 characters, with an ellipsis', () => {
    expect(AGENT_IDENTITY_MODEL_MAX_CHARS).toBe(17);
    const rule = cssRules(CSS).find(r => r.selector === '.ui-agent-identity-model[data-truncate]')!;
    expect(new Map(declarations(rule.body)).get('max-width')).toBe(`${AGENT_IDENTITY_MODEL_MAX_CHARS}ch`);
    const model = new Map(declarations(cssRules(CSS).find(r => r.selector === '.ui-agent-identity-model')!.body));
    expect(model.get('text-overflow')).toBe('ellipsis');
    expect(model.get('color')).toBe('var(--kr-text-secondary)');
  });

  it('writes no text under the 12 px floor: every font size is --kr-type-sm or larger', () => {
    const sizes = [...CSS.matchAll(/font-size:\s*([^;]+);/g)].map(m => m[1].trim());
    expect(sizes.length).toBeGreaterThan(0);
    for (const size of sizes) expect(['var(--kr-type-sm)', 'var(--kr-type-md)', 'var(--kr-type-lg)']).toContain(size);
  });

  it('agentIdentityText gives the same identity for a title or an accessible name, model whole', () => {
    expect(agentIdentityText({ name: 'Codex', model: { kind: 'id', id: 'gpt-5-codex-max-preview' }, role: 'cli' }, t))
      .toBe('Codex · gpt-5-codex-max-preview · ui.agentIdentity.role.cli');
    expect(agentIdentityText({ name: 'Kiro', model: { kind: 'notDeclared' } }, t))
      .toBe('Kiro · ui.agentIdentity.model.notDeclared');
    expect(agentIdentityText({ name: '@ollama', model: { kind: 'providerDefault' }, roleLabel: 'X' }, t))
      .toBe('@ollama · ui.agentIdentity.model.providerDefault · X');
  });

  it('has no axe violation inside a button and in a line of text', async () => {
    const { container } = render(
      <div>
        <p><AgentIdentity name="Codex" model={{ kind: 'notRecorded' }} role="oneOff" tier="default" tierLabel="Défaut" /></p>
        <button type="button"><AgentIdentity name="Kiro" model={{ kind: 'id', id: 'auto' }} role="cli" truncateModel /></button>
      </div>,
    );
    const results = await runAxe(container);
    expect(results.violations, describeViolations(results).join('\n')).toEqual([]);
  });
});
