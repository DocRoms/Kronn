/**
 * Keyboard tier picking inside the mention palette (asked for on 2026-08-19:
 * "avec les flèches droite/gauche, je puisse sélectionner le niveau de
 * raisonnement, sans avoir à déplacer ma souris pour cliquer dessus").
 *
 * Up/Down already moved between agents; the three tier buttons were mouse-only.
 * The subtle part these tests guard is the UNTOUCHED case: Enter without any
 * Left/Right must keep behaving exactly as before, because passing a tier also
 * REMEMBERS it as the agent's preferred tier — so a tier silently sent on every
 * mention would rewrite the user's preference behind their back.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ChatInput } from '../ChatInput';
import type { AgentDetection, Discussion } from '../../types/generated';
import { discussions as discussionsApi } from '../../lib/api';

vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

const baseDiscussion = {
  id: 'd-tier-keys',
  title: 'Tier keys',
  project_id: null,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  archived: false,
  pinned: false,
  workspace_mode: 'Direct',
  workspace_path: null,
  worktree_branch: null,
  tier: 'Default',
  pin_first_message: false,
  summary_cache: null,
  summary_up_to_msg_idx: null,
  shared_id: null,
  shared_with: [],
  workflow_run_id: null,
  created_at: '2026-08-19T09:00:00Z',
  updated_at: '2026-08-19T09:00:00Z',
} as unknown as Discussion;

function renderInput() {
  return render(
    <ChatInput
      discussion={baseDiscussion}
      agents={[
        {
          agent_type: 'Codex',
          installed: true,
          runtime_available: false,
          enabled: true,
        } as AgentDetection,
      ]}
      sending={false}
      disabled={false}
      ttsEnabled={false}
      ttsState="idle"
      worktreeError={null}
      availableSkills={[]}
      availableDirectives={[]}
      onSend={vi.fn()}
      onStop={vi.fn()}
      onOrchestrate={vi.fn()}
      onTtsToggle={vi.fn()}
      onWorktreeErrorDismiss={vi.fn()}
      onWorktreeRetry={vi.fn()}
      isAgentRestricted={() => false}
      contextFiles={[]}
      uploadingFiles={false}
      toast={vi.fn() as never}
      t={(k: string) => k}
    />,
  );
}

/** The tier buttons of the currently highlighted palette row. */
function tierButtons(): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>(
      '.disc-mention-item[data-highlighted="true"] .disc-mention-tier-choice',
    ),
  );
}

function keyboardSelected(): string[] {
  return tierButtons()
    .filter(b => b.getAttribute('data-keyboard-selected') === 'true')
    .map(b => b.getAttribute('data-tier') ?? '');
}

/**
 * Open the palette and move down to the first row that actually exposes tiers.
 * The top row is `@all` (a broadcast, no model of its own), so the realistic
 * gesture is exactly this: pick the agent with Up/Down, then the tier with
 * Left/Right.
 */
function openPaletteOnTierableRow(): HTMLTextAreaElement {
  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  fireEvent.change(textarea, { target: { value: '@' } });
  for (let i = 0; i < 8 && tierButtons().length === 0; i += 1) {
    fireEvent.keyDown(textarea, { key: 'ArrowDown' });
  }
  return textarea;
}

describe('mention palette — Left/Right pick the tier', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.spyOn(discussionsApi, 'participants').mockResolvedValue([]);
    vi.spyOn(discussionsApi, 'nativeAgentMode').mockResolvedValue({ disabled: false });
  });

  it('marks no tier until an arrow is pressed', () => {
    renderInput();
    openPaletteOnTierableRow();

    // The palette is open with tier buttons available…
    expect(tierButtons().length).toBe(3);
    // …and nothing is keyboard-selected yet: the state is UNTOUCHED, which is
    // what keeps Enter's behaviour identical to before this feature.
    expect(keyboardSelected()).toEqual([]);
  });

  it('ArrowRight then ArrowLeft moves the selection and comes back', () => {
    renderInput();
    const textarea = openPaletteOnTierableRow();

    const tiers = tierButtons().map(b => b.getAttribute('data-tier'));
    expect(tiers).toEqual(['economy', 'default', 'reasoning']);

    // Starts from the tier already in effect for that row (default), so one
    // press moves one visible step rather than jumping to index 0.
    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    expect(keyboardSelected()).toEqual(['reasoning']);

    fireEvent.keyDown(textarea, { key: 'ArrowLeft' });
    expect(keyboardSelected()).toEqual(['default']);

    fireEvent.keyDown(textarea, { key: 'ArrowLeft' });
    expect(keyboardSelected()).toEqual(['economy']);

    // Clamped at the low end — no wrap-around, so holding a key cannot silently
    // land on the opposite extreme.
    fireEvent.keyDown(textarea, { key: 'ArrowLeft' });
    expect(keyboardSelected()).toEqual(['economy']);
  });

  it('moving to another agent with ArrowDown forgets the tier pick', () => {
    renderInput();
    const textarea = openPaletteOnTierableRow();

    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    expect(keyboardSelected()).toEqual(['reasoning']);

    // A pick belongs to the row it was made on: carrying it to the next agent
    // would apply a tier the user never saw highlighted for that agent.
    fireEvent.keyDown(textarea, { key: 'ArrowUp' });
    fireEvent.keyDown(textarea, { key: 'ArrowDown' });
    expect(keyboardSelected()).toEqual([]);
  });

  it('keeps the highlighted agent while picking a tier (keyup must not reset it)', () => {
    renderInput();
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: '@' } });

    // Move OFF the first row, the way the user does before choosing a tier.
    fireEvent.keyDown(textarea, { key: 'ArrowDown' });
    fireEvent.keyUp(textarea, { key: 'ArrowDown' });
    const rowBefore = document.querySelector('.disc-mention-item[data-highlighted="true"] .font-semibold')?.textContent;
    expect(rowBefore).toBe('@codex');

    // The reported bug: keyup refreshes the mention query on Left/Right, and that
    // refresh resets the highlighted row to 0 — so every tier keypress threw the
    // selection back to the top of the list. The earlier tests missed it because
    // they only fired keyDown; the defect lived entirely in keyUp.
    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    fireEvent.keyUp(textarea, { key: 'ArrowRight' });

    const rowAfter = document.querySelector('.disc-mention-item[data-highlighted="true"] .font-semibold')?.textContent;
    expect(rowAfter).toBe('@codex');
    expect(keyboardSelected()).toEqual(['reasoning']);
  });

  it('still refreshes on a Left/Right that was NOT a tier pick', () => {
    renderInput();
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: '@' } });

    // Row 0 is `@all` — no tiers, so the key is NOT consumed and the caret really
    // moves. The refresh must still run, otherwise leaving a mention by walking
    // the caret out would leave a stale palette open.
    expect(tierButtons().length).toBe(0);
    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    fireEvent.keyUp(textarea, { key: 'ArrowRight' });
    // Nothing was picked: the guard is scoped to the consumed case only.
    expect(keyboardSelected()).toEqual([]);
  });

  it('keeps the mention query open — Left/Right must not move the caret out', () => {
    renderInput();
    const textarea = openPaletteOnTierableRow();

    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    // The palette is still open: preventDefault kept the caret inside the
    // mention, otherwise the query would have been dropped and the whole
    // interaction would end after one keypress.
    expect(tierButtons().length).toBe(3);
    expect(keyboardSelected()).toEqual(['reasoning']);
  });
});
