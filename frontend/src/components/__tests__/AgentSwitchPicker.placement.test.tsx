// KT-1885 review — the picker's popover is placed from its rendered size, not
// an estimate: in every mode, and from the API assistant's bubble, it stays
// inside the window and never covers its own trigger, at 1440 and 400 px wide.
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());

import { AgentSwitchPicker, type AgentSwitchTarget } from '../AgentSwitchPicker';
import { AssistantAgentPicker } from '../AssistantAgentPicker';

const TARGETS: AgentSwitchTarget[] = [{ agent: 'ClaudeCode' }, { agent: 'Codex' }, { agent: 'GeminiCli' }];

const MODES = {
  field: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" currentTier="default" availableAgents={[]} availableTargets={TARGETS}
      onTargetSelectionChange={vi.fn()} variant="field" title="Agent" ariaLabel="Agent" />
  ),
  compact: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS}
      onChange={vi.fn()} compact title="Agent" ariaLabel="Agent" />
  ),
  multiple: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS}
      multiple={{ selected: [{ agent: 'ClaudeCode' }], onToggle: vi.fn() }} title="Agent" ariaLabel="Agent" />
  ),
  exact: () => (
    <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={[]} availableTargets={TARGETS} title="Agent" ariaLabel="Agent"
      exactModel={{ value: undefined, onChange: vi.fn(), main: { agent: 'ClaudeCode' }, testId: 'exact' }} />
  ),
  assistant: () => (
    <AssistantAgentPicker installedAgents={['ClaudeCode', 'Codex']} current={{ agent: 'ClaudeCode', tier: null }}
      onSelect={vi.fn()} title="Agent" t={key => key} />
  ),
} as const;

interface Box { left: number; top: number; width: number; height: number }

let viewport = { width: 1440, height: 900 };
let trigger: Box = { left: 0, top: 0, width: 0, height: 0 };
let popoverSize = { width: 0, height: 0 };

const rect = ({ left, top, width, height }: Box) => ({
  left, top, width, height, x: left, y: top, right: left + width, bottom: top + height, toJSON: () => ({}),
}) as DOMRect;

beforeEach(() => {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: viewport.width });
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: viewport.height });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    // The popover's rendered size (as a browser lays it out), wherever it is.
    if (this.classList.contains('kr-agent-switch-popover')) return rect({ left: 0, top: 0, ...popoverSize });
    return rect(trigger);
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function setScene(next: { viewport: { width: number; height: number }; trigger: Box; popover: { width: number; height: number } }) {
  viewport = next.viewport;
  trigger = next.trigger;
  popoverSize = next.popover;
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: viewport.width });
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: viewport.height });
}

/** The popover's rectangle as its inline placement draws it. */
function placedBox(popover: HTMLElement): Box {
  const maxHeight = parseFloat(popover.style.maxHeight);
  const height = Math.min(popoverSize.height, maxHeight);
  const left = parseFloat(popover.style.left);
  const top = popover.style.top
    ? parseFloat(popover.style.top)
    : viewport.height - parseFloat(popover.style.bottom) - height;
  return { left, top, width: Math.min(popoverSize.width, viewport.width - 16), height };
}

function expectInsideAndClearOfTrigger(box: Box) {
  expect(box.left).toBeGreaterThanOrEqual(0);
  expect(box.top).toBeGreaterThanOrEqual(0);
  expect(box.left + box.width).toBeLessThanOrEqual(viewport.width);
  expect(box.top + box.height).toBeLessThanOrEqual(viewport.height);
  const overlapsX = box.left < trigger.left + trigger.width && trigger.left < box.left + box.width;
  const overlapsY = box.top < trigger.top + trigger.height && trigger.top < box.top + box.height;
  expect(overlapsX && overlapsY).toBe(false);
}

async function openPicker() {
  const button = screen.getAllByRole('button').find(node => node.getAttribute('aria-haspopup') === 'dialog')!;
  await act(async () => { fireEvent.click(button); });
  return document.querySelector<HTMLElement>('.kr-agent-switch-popover')!;
}

const SCENES = [
  // The Quick API page's assistant bubble, near the right edge: the tier popover is 481 px wide.
  { name: '1440 px, near the right edge', viewport: { width: 1440, height: 900 }, trigger: { left: 1036, top: 120, width: 140, height: 24 }, popover: { width: 481, height: 260 } },
  // A phone-width window: the popover takes the whole width the CSS lets it.
  { name: '400 px', viewport: { width: 400, height: 800 }, trigger: { left: 33, top: 200, width: 120, height: 24 }, popover: { width: 384, height: 300 } },
  // No room below: it opens above, its bottom on the trigger, and a short list leaves no gap.
  { name: '1440 px, at the bottom', viewport: { width: 1440, height: 900 }, trigger: { left: 600, top: 840, width: 160, height: 24 }, popover: { width: 342, height: 180 } },
  { name: '400 px, at the bottom', viewport: { width: 400, height: 800 }, trigger: { left: 300, top: 740, width: 90, height: 24 }, popover: { width: 384, height: 420 } },
];

describe('AgentSwitchPicker — measured placement (KT-1885 review)', () => {
  for (const scene of SCENES) {
    it.each(Object.keys(MODES) as (keyof typeof MODES)[])(`${scene.name}: %s stays in the window, clear of its trigger`, async mode => {
      setScene(scene);
      const Picker = MODES[mode];
      render(<Picker />);
      const popover = await openPicker();
      await waitFor(() => expect(popover.style.left).not.toBe(''));
      expectInsideAndClearOfTrigger(placedBox(popover));
    });
  }

  it('opened above, its bottom is anchored to the trigger whatever the list’s height', async () => {
    setScene(SCENES[2]);
    render(MODES.field());
    const popover = await openPicker();
    expect(popover.style.top).toBe('');
    // 900 - 840 + 4: four pixels above the trigger, however short the list is.
    expect(popover.style.bottom).toBe('64px');
    expect(popover.dataset.placement).toBe('above');
  });

  it('is placed again when the search narrows the list', async () => {
    setScene({ viewport: { width: 1440, height: 900 }, trigger: { left: 600, top: 600, width: 160, height: 24 }, popover: { width: 342, height: 400 } });
    render(MODES.field());
    const popover = await openPicker();
    expect(popover.dataset.placement).toBe('above');
    // Two rows left: the shorter list now fits under the trigger.
    popoverSize = { width: 342, height: 120 };
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'codex' } });
    expect(popover.dataset.placement).toBe('below');
    expect(popover.style.top).toBe('628px');
  });

  it('is placed again when the window is resized', async () => {
    setScene(SCENES[0]);
    render(MODES.field());
    const popover = await openPicker();
    setScene({ ...SCENES[0], viewport: { width: 400, height: 900 }, trigger: { left: 260, top: 120, width: 120, height: 24 }, popover: { width: 384, height: 260 } });
    fireEvent(window, new Event('resize'));
    expectInsideAndClearOfTrigger(placedBox(popover));
    expect(popover.style.left).toBe('8px');
  });
});
