import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';
import { NewDiscussionForm } from '../NewDiscussionForm';
import type { Project, AgentDetection } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  skills: { list: vi.fn().mockResolvedValue([]) },
  profiles: { list: vi.fn().mockResolvedValue([]) },
  directives: { list: vi.fn().mockResolvedValue([]) },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

const PROJECT_WITH_REPO: Project = {
  id: 'proj-git', name: 'acme-frontend',
  path: '/repos/acme-frontend',
  repo_url: 'git@github.com:acme-org/acme-frontend.git',
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const PROJECT_WITHOUT_REPO: Project = {
  id: 'proj-local', name: 'local-notes',
  path: '/repos/local-notes',
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
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const mount = (projects: Project[]) => {
  const onSubmit = vi.fn();
  return render(
    <NewDiscussionForm
      projects={projects}
      agents={[AGENT]}
      configLanguage="fr"
      agentAccess={null}
      onSubmit={onSubmit}
      onClose={vi.fn()}
      onNavigate={vi.fn()}
      t={(key: string) => key}
    />
  );
};

describe('NewDiscussionForm — workspace toggle', () => {
  it('shows the workspace toggle (Direct / Isolated) when the selected project has a repo_url', async () => {
    // Regression guard for 2026-04-13: user reported the Isolated option
    // disappeared. Root cause would be either the `selectedProj?.repo_url`
    // check or the condition wrapping the whole block — both are tested here.
    mount([PROJECT_WITH_REPO]);

    // Project dropdown shows our repo-backed project — select it
    const projectSelect = screen.getAllByRole('combobox')[0];
    await act(async () => {
      fireEvent.change(projectSelect, { target: { value: PROJECT_WITH_REPO.id } });
    });

    // The workspace-toggle container renders with both buttons (data-mode)
    await waitFor(() => {
      expect(document.querySelector('.disc-workspace-toggle')).not.toBeNull();
      expect(document.querySelector('.disc-workspace-btn[data-mode="direct"]')).not.toBeNull();
      expect(document.querySelector('.disc-workspace-btn[data-mode="isolated"]')).not.toBeNull();
    });
  });

  it('shows the toggle but disables Isolated for projects without a repo_url', async () => {
    // Non-regression: for non-git projects we still display the toggle
    // so users see the option exists — but Isolated is disabled with a
    // tooltip explaining why. Hiding it silently was the bug that made
    // Marie think the feature vanished.
    mount([PROJECT_WITHOUT_REPO]);
    const projectSelect = screen.getAllByRole('combobox')[0];
    await act(async () => {
      fireEvent.change(projectSelect, { target: { value: PROJECT_WITHOUT_REPO.id } });
    });
    expect(document.querySelector('.disc-workspace-toggle')).not.toBeNull();
    const isolatedBtn = document.querySelector('.disc-workspace-btn[data-mode="isolated"]') as HTMLButtonElement;
    expect(isolatedBtn).not.toBeNull();
    expect(isolatedBtn.disabled).toBe(true);
  });

  it('reveals the branch-name / base-branch inputs when the user picks Isolated', async () => {
    mount([PROJECT_WITH_REPO]);
    const projectSelect = screen.getAllByRole('combobox')[0];
    await act(async () => {
      fireEvent.change(projectSelect, { target: { value: PROJECT_WITH_REPO.id } });
    });
    await waitFor(() => {
      expect(document.querySelector('.disc-workspace-btn[data-mode="isolated"]')).not.toBeNull();
    });
    const isolatedBtn = document.querySelector('.disc-workspace-btn[data-mode="isolated"]') as HTMLButtonElement;
    await act(async () => {
      fireEvent.click(isolatedBtn);
    });
    await waitFor(() => {
      expect(document.querySelector('.disc-workspace-branch-grid')).not.toBeNull();
    });
  });
});

describe('NewDiscussionForm — prefill profile auto-select', () => {
  it('does NOT pre-select validation profiles for an unlocked prefill', async () => {
    // Pre-fix: every prefill triggered the architect/tech-lead/qa-engineer
    // auto-select, including the unlocked "New discussion" button and the
    // "Discuss this file" CTA from the AI doc viewer. Users discovered
    // their unrelated chats were silently using validator profiles. The
    // submit payload is the cleanest observable — assert profileIds is
    // empty for unlocked prefill.
    const onSubmit = vi.fn();
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITH_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        prefill={{ projectId: PROJECT_WITH_REPO.id, title: 'Discuss file', prompt: 'go' }}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        onPrefillConsumed={vi.fn()}
        t={(key: string) => key}
      />,
    );
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    expect(createBtn).not.toBeNull();
    await act(async () => { fireEvent.click(createBtn); });
    await act(async () => { await Promise.resolve(); });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit.mock.calls[0][0].profileIds).toEqual([]);
  });

  it('pre-selects the validation triplet when prefill is locked', async () => {
    const onSubmit = vi.fn();
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITH_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        prefill={{ projectId: PROJECT_WITH_REPO.id, title: 'Validation', prompt: 'check', locked: true }}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        onPrefillConsumed={vi.fn()}
        t={(key: string) => key}
      />,
    );
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    expect(createBtn).not.toBeNull();
    await act(async () => { fireEvent.click(createBtn); });
    await act(async () => { await Promise.resolve(); });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit.mock.calls[0][0].profileIds).toEqual(
      ['architect', 'tech-lead', 'qa-engineer'],
    );
  });
});

describe('NewDiscussionForm — Ctrl+Enter submit', () => {
  it('Ctrl+Enter on the prompt textarea submits without inserting a newline', async () => {
    // Pre-fix: the card-level keyDown caught Ctrl+Enter and called
    // `handleCreate()` but did NOT call `e.preventDefault()`, so the
    // textarea also processed the keypress and inserted a literal "\n"
    // into the prompt. The submitted message ended with a stray
    // newline (visible as a blank line in agent transcripts).
    const onSubmit = vi.fn();
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITHOUT_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        t={(key: string) => key}
      />,
    );
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const promptInput = document.querySelector('textarea') as HTMLTextAreaElement;
    expect(promptInput).not.toBeNull();
    await act(async () => {
      fireEvent.change(promptInput, { target: { value: 'Hello world' } });
    });
    const agentBtn = document.querySelector('.disc-agent-btn') as HTMLButtonElement | null;
    if (agentBtn) await act(async () => { fireEvent.click(agentBtn); });

    // Fire Ctrl+Enter on the wrapping card so the form-level handler
    // catches it (matches what happens in the browser when the textarea
    // is focused and the user presses Ctrl+Enter — bubbles up to the
    // card). preventDefault must keep the textarea value clean.
    const card = document.querySelector('.disc-new-card') as HTMLElement;
    expect(card).not.toBeNull();
    await act(async () => {
      fireEvent.keyDown(card, { key: 'Enter', ctrlKey: true });
      await Promise.resolve();
    });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit.mock.calls[0][0].prompt).toBe('Hello world');
    // The submitted prompt must NOT end with a stray newline.
    expect(onSubmit.mock.calls[0][0].prompt).not.toMatch(/\n$/);
  });

  it('Ctrl+Enter is ignored while an IME composition is active', async () => {
    const onSubmit = vi.fn();
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITHOUT_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        t={(key: string) => key}
      />,
    );
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const promptInput = document.querySelector('textarea') as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.change(promptInput, { target: { value: '日本語' } });
    });
    const agentBtn = document.querySelector('.disc-agent-btn') as HTMLButtonElement | null;
    if (agentBtn) await act(async () => { fireEvent.click(agentBtn); });

    const card = document.querySelector('.disc-new-card') as HTMLElement;
    // Simulate the IME composition state: the keydown event flags
    // `nativeEvent.isComposing` while the IME is composing a candidate.
    // React's SyntheticEvent forwards `isComposing` from the native
    // KeyboardEvent.
    await act(async () => {
      const ev = new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true });
      Object.defineProperty(ev, 'isComposing', { get: () => true });
      card.dispatchEvent(ev);
      await Promise.resolve();
    });

    expect(onSubmit).not.toHaveBeenCalled();
  });
});

describe('NewDiscussionForm — re-entry guard', () => {
  it('re-enables the Create button after onSubmit rejects (no permanent wedge)', async () => {
    // Pre-fix the form set `creating=true` and never reset it because
    // onSubmit was typed `=> void` and not awaited. If `discussions.create`
    // failed (auth error, validation, network), the button stayed disabled
    // forever and the user had to close+reopen the form to retry.
    const onSubmit = vi.fn().mockRejectedValue(new Error('boom'));
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITHOUT_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        t={(key: string) => key}
      />,
    );

    // Fill the prompt textarea + pick the agent so handleCreate proceeds.
    const promptInput = screen.getByPlaceholderText(/disc\.promptPlaceholder|Promptez/i)
      ?? document.querySelector('textarea')!;
    await act(async () => {
      fireEvent.change(promptInput, { target: { value: 'Hello' } });
    });
    const agentBtn = document.querySelector('.disc-agent-btn') as HTMLButtonElement | null;
    if (agentBtn) await act(async () => { fireEvent.click(agentBtn); });

    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    expect(createBtn).not.toBeNull();
    expect(createBtn.disabled).toBe(false);

    await act(async () => { fireEvent.click(createBtn); });
    // Allow the awaited rejection to flush.
    await act(async () => { await Promise.resolve(); });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    // After failure, button must be clickable again.
    expect(createBtn.disabled).toBe(false);
  });

  it('does not call onSubmit twice on two synchronous clicks', async () => {
    const onSubmit = vi.fn().mockImplementation(() => new Promise(() => { /* hold */ }));
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITHOUT_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        t={(key: string) => key}
      />,
    );

    const promptInput = document.querySelector('textarea')!;
    await act(async () => {
      fireEvent.change(promptInput, { target: { value: 'Hello' } });
    });
    const agentBtn = document.querySelector('.disc-agent-btn') as HTMLButtonElement | null;
    if (agentBtn) await act(async () => { fireEvent.click(agentBtn); });

    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    expect(createBtn).not.toBeNull();

    await act(async () => {
      fireEvent.click(createBtn);
      fireEvent.click(createBtn);
    });

    // Ref-based guard inside handleCreate must short-circuit the second click
    // synchronously, even before React re-renders the disabled state.
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });
});

describe('NewDiscussionForm — 0.8.6 disc-first refactor', () => {
  // The checkbox "Launch an agent right away" (default ON) gates the
  // agent picker + the auto-runAgent flow. Unchecking lets the user
  // create an empty disc that they invite agents into later. These
  // tests lock the contract :
  //   * checkbox checked → agent picker visible, legacy submit
  //   * checkbox unchecked → picker hidden, submit emits
  //     launchAgentNow=false, prompt becomes optional
  //   * tooltip carries the 23-word hint validated 2026-05-20

  const findLaunchCheckbox = () =>
    document.querySelector(
      'input[aria-label="disc.launchAgentNow"]',
    ) as HTMLInputElement | null;

  it('shows the launch-agent checkbox checked by default + the legacy agent picker', async () => {
    mount([PROJECT_WITH_REPO]);
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const checkbox = findLaunchCheckbox();
    expect(checkbox).not.toBeNull();
    expect(checkbox!.checked).toBe(true);

    // Picker visible (Claude Code option).
    const agentSelect = document.querySelector(
      'select[aria-label="disc.agent"]',
    );
    expect(agentSelect).not.toBeNull();
  });

  it('hides the agent picker + shows the disc-first hint when the checkbox is unchecked', async () => {
    mount([PROJECT_WITH_REPO]);
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const checkbox = findLaunchCheckbox();
    await act(async () => { fireEvent.click(checkbox!); });

    expect(checkbox!.checked).toBe(false);
    expect(
      document.querySelector('select[aria-label="disc.agent"]'),
    ).toBeNull();
    // Hint copy fragment from disc.discFirstHint is present.
    expect(document.body.textContent).toContain('disc.discFirstHint');
  });

  it('tooltip ⓘ carries the validated 23-word hint as `title`', async () => {
    mount([PROJECT_WITH_REPO]);
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });
    const infoIcon = document.querySelector(
      '.disc-form-info-icon',
    ) as HTMLElement | null;
    expect(infoIcon).not.toBeNull();
    expect(infoIcon!.title).toBe('disc.launchAgentNowHint');
  });

  it('submit emits launchAgentNow=true with the legacy payload when checkbox stays ON', async () => {
    const onSubmit = vi.fn();
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITH_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        t={(key: string) => key}
      />,
    );
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const textarea = document.querySelector(
      'textarea[aria-label="disc.prompt"]',
    ) as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.change(textarea, { target: { value: 'investigate the bug' } });
    });
    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    await act(async () => { fireEvent.click(createBtn); });
    await act(async () => { await Promise.resolve(); });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit.mock.calls[0][0].launchAgentNow).toBe(true);
    expect(onSubmit.mock.calls[0][0].agent).toBe('ClaudeCode');
    expect(onSubmit.mock.calls[0][0].prompt).toBe('investigate the bug');
  });

  it('submit emits launchAgentNow=false when checkbox is unchecked, even with no agent installed', async () => {
    // Disc-first scenario : user opens the form on a fresh machine
    // without any CLI installed, types a brief, submits → disc gets
    // created, no runAgent kick-off. The parent (DiscussionsPage)
    // will short-circuit the streaming flow on launchAgentNow=false.
    const onSubmit = vi.fn();
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITH_REPO]}
        agents={[]}
        configLanguage="fr"
        agentAccess={null}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        t={(key: string) => key}
      />,
    );
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const checkbox = findLaunchCheckbox();
    await act(async () => { fireEvent.click(checkbox!); });

    const textarea = document.querySelector(
      'textarea[aria-label="disc.prompt"]',
    ) as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.change(textarea, { target: { value: 'topic for later' } });
    });
    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    expect(createBtn.disabled).toBe(false);
    await act(async () => { fireEvent.click(createBtn); });
    await act(async () => { await Promise.resolve(); });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    const payload = onSubmit.mock.calls[0][0];
    expect(payload.launchAgentNow).toBe(false);
    expect(payload.prompt).toBe('topic for later');
    // The agent field still carries a placeholder (the form doesn't
    // know how to send `null` — it just sends the first installed
    // agent or 'ClaudeCode'). The parent uses launchAgentNow=false
    // to skip runAgent regardless.
    expect(payload.agent).toBe('ClaudeCode');
  });

  it('disc-first mode lets the user submit with ONLY a title (no prompt)', async () => {
    // The MVP intent : create an empty topic, fill in the brief later
    // when an agent is invited. Title alone is enough.
    const onSubmit = vi.fn();
    render(
      <NewDiscussionForm
        projects={[PROJECT_WITH_REPO]}
        agents={[AGENT]}
        configLanguage="fr"
        agentAccess={null}
        onSubmit={onSubmit}
        onClose={vi.fn()}
        onNavigate={vi.fn()}
        t={(key: string) => key}
      />,
    );
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const checkbox = findLaunchCheckbox();
    await act(async () => { fireEvent.click(checkbox!); });

    const titleInput = document.querySelector(
      'input[aria-label="disc.title"]',
    ) as HTMLInputElement;
    await act(async () => {
      fireEvent.change(titleInput, { target: { value: 'RGPD audit room' } });
    });
    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    expect(createBtn.disabled).toBe(false);
    await act(async () => { fireEvent.click(createBtn); });
    await act(async () => { await Promise.resolve(); });

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit.mock.calls[0][0].title).toBe('RGPD audit room');
    expect(onSubmit.mock.calls[0][0].prompt).toBe('');
  });

  it('disc-first mode keeps submit DISABLED when both title AND prompt are blank', async () => {
    mount([PROJECT_WITH_REPO]);
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    const checkbox = findLaunchCheckbox();
    await act(async () => { fireEvent.click(checkbox!); });

    const createBtn = document.querySelector('.disc-create-btn') as HTMLButtonElement;
    expect(createBtn.disabled).toBe(true);
  });
});
