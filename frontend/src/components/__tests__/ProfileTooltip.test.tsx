import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { ProfileTooltip } from '../ProfileTooltip';
import type { AgentProfile } from '../../types/generated';

function sampleProfile(overrides: Partial<AgentProfile> = {}): AgentProfile {
  return {
    id: 'sample',
    name: 'Architect',
    persona_name: 'Kai',
    role: 'Software Architect',
    avatar: '🏗️',
    color: '#4d9fff',
    category: 'Technical',
    persona_prompt: 'You are a senior software architect with 15+ years.',
    default_engine: undefined,
    is_builtin: true,
    token_estimate: 50,
    ...overrides,
  };
}

describe('ProfileTooltip', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('does not show the tooltip by default', () => {
    render(
      <ProfileTooltip profile={sampleProfile()}>
        <span data-testid="chip">Chip</span>
      </ProfileTooltip>
    );
    expect(screen.queryByRole('tooltip')).toBeNull();
  });

  it('shows the tooltip after the open delay on hover', () => {
    render(
      <ProfileTooltip profile={sampleProfile()} openDelayMs={300}>
        <span data-testid="chip">Chip</span>
      </ProfileTooltip>
    );
    // Hover on the wrapper span — React attaches onMouseEnter to it.
    const chip = screen.getByTestId('chip');
    const wrapper = chip.parentElement!;
    fireEvent.mouseEnter(wrapper);
    // Before the delay — still hidden
    expect(screen.queryByRole('tooltip')).toBeNull();
    // Advance past the delay
    act(() => { vi.advanceTimersByTime(350); });
    const tip = screen.getByRole('tooltip');
    expect(tip).toBeInTheDocument();
    expect(tip.textContent).toContain('Kai');
    expect(tip.textContent).toContain('Software Architect');
    expect(tip.textContent).toContain('senior software architect');
  });

  it('cancels the open timer when the mouse leaves before the delay', () => {
    render(
      <ProfileTooltip profile={sampleProfile()} openDelayMs={300}>
        <span data-testid="chip">Chip</span>
      </ProfileTooltip>
    );
    const wrapper = screen.getByTestId('chip').parentElement!;
    fireEvent.mouseEnter(wrapper);
    fireEvent.mouseLeave(wrapper);
    act(() => { vi.advanceTimersByTime(500); });
    expect(screen.queryByRole('tooltip')).toBeNull();
  });

  it('hides the tooltip on mouse leave after it opens', () => {
    render(
      <ProfileTooltip profile={sampleProfile()} openDelayMs={10}>
        <span data-testid="chip">Chip</span>
      </ProfileTooltip>
    );
    const wrapper = screen.getByTestId('chip').parentElement!;
    fireEvent.mouseEnter(wrapper);
    act(() => { vi.advanceTimersByTime(50); });
    expect(screen.getByRole('tooltip')).toBeInTheDocument();

    fireEvent.mouseLeave(wrapper);
    expect(screen.queryByRole('tooltip')).toBeNull();
  });

  it('truncates long persona_prompt with an ellipsis marker', () => {
    const longPrompt = 'x'.repeat(500);
    const profile = sampleProfile({ persona_prompt: longPrompt });
    render(
      <ProfileTooltip profile={profile} openDelayMs={0} maxPromptChars={100}>
        <span data-testid="chip">Chip</span>
      </ProfileTooltip>
    );
    const wrapper = screen.getByTestId('chip').parentElement!;
    fireEvent.mouseEnter(wrapper);
    act(() => { vi.advanceTimersByTime(5); });
    const tip = screen.getByRole('tooltip');
    expect(tip.textContent).toContain('x'.repeat(100));
    expect(tip.textContent).not.toContain('x'.repeat(101));
    expect(tip.textContent).toContain('…');
  });

  it('auto-scrolls long prompts when they overflow the fixed maxHeight', () => {
    // jsdom reports 0 for scrollHeight/clientHeight — stub the Element
    // prototype so the useEffect's overflow detection sees a long prompt.
    // Restore in the test's finally block to avoid bleeding into siblings.
    const shDesc = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollHeight');
    const chDesc = Object.getOwnPropertyDescriptor(Element.prototype, 'clientHeight');
    Object.defineProperty(Element.prototype, 'scrollHeight', {
      configurable: true, get() { return 400; },
    });
    Object.defineProperty(Element.prototype, 'clientHeight', {
      configurable: true, get() { return 180; },
    });
    try {
      const longPrompt = 'Lorem ipsum dolor sit amet.\n'.repeat(25);
      const profile = sampleProfile({ persona_prompt: longPrompt });
      render(
        <ProfileTooltip profile={profile} openDelayMs={0}>
          <span data-testid="chip">Chip</span>
        </ProfileTooltip>
      );
      const wrapper = screen.getByTestId('chip').parentElement!;
      fireEvent.mouseEnter(wrapper);
      act(() => { vi.advanceTimersByTime(5); });

      const tip = screen.getByRole('tooltip');
      const content = tip.querySelector<HTMLElement>('div[style*="overflow"]');
      expect(content).not.toBeNull();
      if (!content) return;

      expect(content.scrollTop).toBe(0);

      // Cross the initial delay into the scroll-down phase — scrollTop
      // must have advanced. We don't pin an exact value because the
      // requestAnimationFrame loop is time-based; any positive value
      // proves the auto-scroll is running.
      act(() => { vi.advanceTimersByTime(1200 + 3000); });
      expect(content.scrollTop).toBeGreaterThan(0);
    } finally {
      if (shDesc) Object.defineProperty(Element.prototype, 'scrollHeight', shDesc);
      else delete (Element.prototype as { scrollHeight?: unknown }).scrollHeight;
      if (chDesc) Object.defineProperty(Element.prototype, 'clientHeight', chDesc);
      else delete (Element.prototype as { clientHeight?: unknown }).clientHeight;
    }
  });

  it('shows the avatar, persona_name, and role together', () => {
    render(
      <ProfileTooltip profile={sampleProfile({ avatar: '🦇', persona_name: 'Bat', role: 'Détective' })} openDelayMs={0}>
        <span data-testid="chip">Chip</span>
      </ProfileTooltip>
    );
    const wrapper = screen.getByTestId('chip').parentElement!;
    fireEvent.mouseEnter(wrapper);
    act(() => { vi.advanceTimersByTime(5); });
    const tip = screen.getByRole('tooltip');
    expect(tip.textContent).toContain('🦇');
    expect(tip.textContent).toContain('Bat');
    expect(tip.textContent).toContain('Détective');
  });
});
