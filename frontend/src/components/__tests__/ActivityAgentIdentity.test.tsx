// KT-1936 review, finding 3 — an activity bubble (pending, streaming, failed)
// names the main agent's model the way the header does: the room of the review
// (OpenCode, mode reasoning, no exact model) writes the mode's model, not
// « default model »; with no model known it writes the unknown state.
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ActivityAgentIdentity } from '../ActivityAgentIdentity';
import { discussionAgentModel } from '../../lib/agentModelResolution';

const empty = { economy: null, default: null, reasoning: null };
const familyTiers = {
  claude_code: empty, codex: empty, gemini_cli: empty, kiro: empty, vibe: empty,
  copilot_cli: empty, ollama: empty, lite_llm: empty, nvidia: empty,
  open_code: { economy: null, default: null, reasoning: 'opencode-go/kimi-k3.1-pro' },
};

describe('ActivityAgentIdentity', () => {
  it('writes the model of the discussion\'s mode', () => {
    const room = { agent: 'OpenCode' as const, tier: 'reasoning' as const, model: null, messages: [] };
    render(<ActivityAgentIdentity agent="OpenCode" role="main" model={discussionAgentModel(room, { familyTiers })} />);
    const model = screen.getByTestId('agent-identity-model');
    expect(model).toHaveTextContent('opencode-go/kimi-k3.1-pro');
    expect(model).toHaveAttribute('data-state', 'id');
  });

  it('writes the unknown state, without repeating the agent, when no model is known', () => {
    const room = { agent: 'OpenCode' as const, tier: 'default' as const, model: null, messages: [] };
    const { container } = render(
      <ActivityAgentIdentity agent="OpenCode" role="main" model={discussionAgentModel(room, { familyTiers })} />,
    );
    expect(screen.getByTestId('agent-identity-model')).toHaveAttribute('data-state', 'providerDefault');
    expect(container.textContent).toBe('OpenCode · ui.agentIdentity.model.providerDefault · ui.agentIdentity.role.main');
  });
});
