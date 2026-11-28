import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AgentSwitchPicker } from '../AgentSwitchPicker';

describe('AgentSwitchPicker — accessible agent identity colours', () => {
  it('uses the contrast-safe NVIDIA text token while retaining the brand dot', () => {
    render(
      <AgentSwitchPicker
        currentAgent="Nvidia"
        availableAgents={['Nvidia', 'Codex']}
        onChange={vi.fn().mockResolvedValue(undefined)}
        title="Switch agent"
        ariaLabel="Switch agent"
      />,
    );

    const trigger = screen.getByRole('button', { name: /^Switch agent( — |$)/ });
    // KT-1936 — the name in the text-safe token, the brand colour on the border.
    expect(trigger.querySelector<HTMLElement>('.kr-agent-switch-name')?.style.color).toBe('var(--kr-agent-nvidia-text)');
    expect(trigger.style.getPropertyValue('--agent-switch-tone')).toBe('#76B900');

    fireEvent.click(trigger);
    const nvidiaOption = screen.getByRole('menuitem', { name: /NVIDIA/ });
    expect(nvidiaOption.querySelector<HTMLElement>('.kr-agent-switch-option-dot')?.style.background)
      .toBe('#76B900');
  });

  it('uses the same safe colour for a static NVIDIA identity', () => {
    const { container } = render(
      <AgentSwitchPicker
        currentAgent="Nvidia"
        availableAgents={['Nvidia']}
        title="NVIDIA"
        ariaLabel="NVIDIA"
      />,
    );

    expect(container.querySelector<HTMLElement>('.kr-agent-switch-static .kr-agent-switch-name')?.style.color)
      .toBe('var(--kr-agent-nvidia-text)');
  });

  // KT-1936 review, finding 4 — the visible model is cut past 17 characters:
  // the accessible name carries the action AND the identity (agent, full model
  // id or unknown state, mode or « Personnalisé », role), as the participant
  // chip does.
  const modeTiers = {
    claude_code: { economy: null, default: null, reasoning: null },
    codex: { economy: null, default: null, reasoning: null },
    open_code: { economy: null, default: null, reasoning: 'opencode-go/kimi-k3.1-pro' },
    gemini_cli: { economy: null, default: null, reasoning: null },
    kiro: { economy: null, default: null, reasoning: null },
    vibe: { economy: null, default: null, reasoning: null },
    copilot_cli: { economy: null, default: null, reasoning: null },
    ollama: { economy: null, default: null, reasoning: null },
    lite_llm: { economy: null, default: null, reasoning: null },
    nvidia: { economy: null, default: null, reasoning: null },
  };
  const headerTrigger = (props: Partial<Parameters<typeof AgentSwitchPicker>[0]> = {}) => render(
    <AgentSwitchPicker
      currentAgent="OpenCode"
      availableAgents={['OpenCode', 'Codex']}
      title="Changer d’agent ou de mode IA"
      ariaLabel="Changer d’agent ou de mode IA"
      role="agent principal"
      currentTier="reasoning"
      tierLabels={{ economy: 'Économie', default: 'Standard', reasoning: 'Avancé' }}
      modelTiers={modeTiers}
      displayName="@opencode"
      onTargetSelectionChange={vi.fn().mockResolvedValue(undefined)}
      {...props}
    />,
  );

  it('names the trigger by its action, then the agent, the full model id, the mode and the role', () => {
    headerTrigger();
    expect(screen.getByRole('button', { name: /^Changer d’agent/ })).toHaveAccessibleName(
      'Changer d’agent ou de mode IA — @opencode · opencode-go/kimi-k3.1-pro · Avancé · agent principal',
    );
  });

  it('names a custom model « Personnalisé » and an unknown model by its state', () => {
    const { unmount } = headerTrigger({
      customModel: { model: 'a-very-long-custom-model-id-2026', onSelect: vi.fn().mockResolvedValue(undefined) },
    });
    expect(screen.getByRole('button', { name: /^Changer d’agent/ })).toHaveAccessibleName(
      'Changer d’agent ou de mode IA — @opencode · a-very-long-custom-model-id-2026 · agentPicker.custom · agent principal',
    );
    unmount();
    headerTrigger({ currentTier: 'default' });
    expect(screen.getByRole('button', { name: /^Changer d’agent/ })).toHaveAccessibleName(
      'Changer d’agent ou de mode IA — @opencode · ui.agentIdentity.model.providerDefault · Standard · agent principal',
    );
  });

  it('repositions the open picker when the viewport no longer has room below', () => {
    const rectSpy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      bottom: 40,
      height: 20,
      left: 20,
      right: 120,
      top: 20,
      width: 100,
      x: 20,
      y: 20,
      toJSON: () => ({}),
    });
    const originalInnerHeight = window.innerHeight;
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 500 });

    render(
      <AgentSwitchPicker
        currentAgent="Codex"
        availableAgents={['Codex', 'ClaudeCode']}
        onChange={vi.fn().mockResolvedValue(undefined)}
        title="Switch agent"
        ariaLabel="Switch agent"
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /^Switch agent( — |$)/ }));
    const popover = document.querySelector<HTMLElement>('.kr-agent-switch-popover');
    expect(popover?.style.top).toBe('44px');

    // KT-1885 — as little room above as below: it stays under its trigger, capped
    // to the room left, rather than covering the trigger (see the placement tests).
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 60 });
    fireEvent(window, new Event('resize'));
    expect(popover?.style.top).toBe('44px');
    expect(popover?.style.maxHeight).toBe('8px');

    Object.defineProperty(window, 'innerHeight', { configurable: true, value: originalInnerHeight });
    rectSpy.mockRestore();
  });
});
