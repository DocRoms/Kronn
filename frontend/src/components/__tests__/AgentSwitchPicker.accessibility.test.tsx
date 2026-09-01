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

    const trigger = screen.getByRole('button', { name: 'Switch agent' });
    expect(trigger.style.color).toBe('var(--kr-agent-nvidia-text)');

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

    expect(container.querySelector<HTMLElement>('.kr-agent-switch-static')?.style.color)
      .toBe('var(--kr-agent-nvidia-text)');
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

    fireEvent.click(screen.getByRole('button', { name: 'Switch agent' }));
    const popover = document.querySelector<HTMLElement>('.kr-agent-switch-popover');
    expect(popover?.style.top).toBe('45px');

    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 60 });
    fireEvent(window, new Event('resize'));
    expect(popover?.style.top).toBe('8px');

    Object.defineProperty(window, 'innerHeight', { configurable: true, value: originalInnerHeight });
    rectSpy.mockRestore();
  });
});
