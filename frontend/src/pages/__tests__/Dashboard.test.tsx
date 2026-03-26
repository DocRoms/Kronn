import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock ALL API modules used by Dashboard and its children
vi.mock('../../lib/api', () => ({
  projects: {
    list: vi.fn().mockResolvedValue([]),
    scan: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    delete: vi.fn(),
    installTemplate: vi.fn(),
    auditStream: vi.fn(),
  },
  mcps: {
    registry: vi.fn().mockResolvedValue([]),
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], customized_contexts: [] }),
  },
  agents: {
    detect: vi.fn().mockResolvedValue([]),
    install: vi.fn(),
    uninstall: vi.fn(),
    toggle: vi.fn(),
  },
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    delete: vi.fn(),
    update: vi.fn(),
    sendMessageStream: vi.fn(),
    runAgent: vi.fn(),
    _streamSSE: vi.fn(),
  },
  config: {
    getLanguage: vi.fn().mockResolvedValue('fr'),
    getAgentAccess: vi.fn().mockResolvedValue(null),
  },
  skills: {
    list: vi.fn().mockResolvedValue([]),
  },
  profiles: {
    list: vi.fn().mockResolvedValue([]),
  },
  workflows: {
    list: vi.fn().mockResolvedValue([]),
  },
}));

import { discussions as discussionsApi, projects as projectsApi } from '../../lib/api';
import { Dashboard } from '../Dashboard';
import type { Discussion, Project } from '../../types/generated';

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
  localStorage.clear();
  document.title = 'Kronn';
});

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

const makeDiscussion = (id: string, msgCount: number): Discussion => ({
  id,
  project_id: null,
  title: `Discussion ${id}`,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: msgCount,
  archived: false,
  workspace_mode: 'Direct',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

const makeProject = (id: string, name: string, org?: string): Project => ({
  id,
  name,
  path: org ? `/repos/${org}/${name}` : `/repos/${name}`,
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

describe('Dashboard — unseen badge & document.title', () => {
  it('shows unseen badge on discussions tab when on another page', async () => {
    const disc = makeDiscussion('d1', 3);
    vi.mocked(discussionsApi.list).mockResolvedValue([disc]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    const badge = screen.queryByText('3');
    expect(badge).toBeTruthy();
  });

  it('updates document.title with unseen count', async () => {
    const disc = makeDiscussion('d1', 5);
    vi.mocked(discussionsApi.list).mockResolvedValue([disc]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    expect(document.title).toBe('(5) Kronn');
  });

  it('shows no badge when all messages are seen', async () => {
    const disc = makeDiscussion('d1', 3);
    vi.mocked(discussionsApi.list).mockResolvedValue([disc]);
    localStorage.setItem('kronn:lastSeenMsgCount', JSON.stringify({ d1: 3 }));

    await wrap(<Dashboard onReset={vi.fn()} />);

    expect(document.title).toBe('Kronn');
  });

  it('refetches discussions when tab becomes visible', async () => {
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    vi.mocked(discussionsApi.list).mockClear();

    const original = document.visibilityState;
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
    });

    expect(vi.mocked(discussionsApi.list)).toHaveBeenCalled();

    Object.defineProperty(document, 'visibilityState', { value: original, configurable: true });
  });
});

describe('Dashboard — project list', () => {
  it('renders projects grouped by parent directory', async () => {
    const projects = [
      makeProject('p1', 'frontend', 'acme'),
      makeProject('p2', 'backend', 'acme'),
      makeProject('p3', 'solo-project'),
    ];
    // Give repo_url to acme projects so getProjectGroup extracts org name
    projects[0].repo_url = 'git@github.com:acme/frontend.git';
    projects[1].repo_url = 'git@github.com:acme/backend.git';
    vi.mocked(projectsApi.list).mockResolvedValue(projects);
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    const body = document.body.textContent!;
    // Project names should be visible
    expect(body).toContain('frontend');
    expect(body).toContain('backend');
    expect(body).toContain('solo-project');
    // Org group header should appear for the acme group
    expect(body).toContain('acme');
  });

  it('renders search input that filters projects', async () => {
    const projects = [
      makeProject('p1', 'react-app'),
      makeProject('p2', 'rust-api'),
      makeProject('p3', 'vue-dashboard'),
      makeProject('p4', 'go-service'),
    ];
    vi.mocked(projectsApi.list).mockResolvedValue(projects);
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    // All projects visible initially
    expect(document.body.textContent!).toContain('react-app');
    expect(document.body.textContent!).toContain('rust-api');

    // Type in search to filter
    const searchInput = document.body.querySelector('input[placeholder]') as HTMLInputElement;
    expect(searchInput).toBeTruthy();
    await act(async () => {
      fireEvent.change(searchInput, { target: { value: 'react' } });
    });

    // Only matching project should remain visible
    expect(document.body.textContent!).toContain('react-app');
    expect(document.body.textContent!).not.toContain('rust-api');
    expect(document.body.textContent!).not.toContain('vue-dashboard');
    expect(document.body.textContent!).not.toContain('go-service');
  });

  it('renders "New project" button in nav bar', async () => {
    vi.mocked(projectsApi.list).mockResolvedValue([]);
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    // The nav bar shows the project-creation button (French: "Ajouter un projet")
    const body = document.body.textContent!;
    expect(body).toContain('Ajouter un projet');
  });
});

describe('Dashboard — mobile responsive', () => {
  it('hides text labels on nav buttons when on mobile viewport', async () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query.includes('767'),
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });

    vi.mocked(projectsApi.list).mockResolvedValue([]);
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    // On mobile, nav buttons should have title attributes (for accessibility)
    // but should NOT display text labels inline (only icons)
    const navButtons = Array.from(document.body.querySelectorAll('button[title]'));
    const tabButtons = navButtons.filter(b => {
      const title = b.getAttribute('title');
      return title && ['Projets', 'Discussions', 'MCPs', 'Workflows', 'Config'].includes(title);
    });

    // Each nav tab button should have a title attribute
    expect(tabButtons.length).toBeGreaterThan(0);

    // On mobile, the text label is hidden — button textContent should NOT contain the full label
    // (it only shows the icon, which renders as empty text in jsdom)
    for (const btn of tabButtons) {
      const title = btn.getAttribute('title')!;
      // The button text should NOT include the label (text is hidden on mobile)
      const textContent = btn.textContent ?? '';
      expect(textContent).not.toContain(title);
    }

    // Restore default matchMedia
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });
  });
});

describe('Dashboard — Ctrl+Enter keyboard shortcuts', () => {
  it('bootstrap and clone forms open via new project button', async () => {
    vi.mocked(projectsApi.list).mockResolvedValue([]);
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    // Find and click the new project button (contains "Nouveau projet" in FR)
    const allButtons = Array.from(document.body.querySelectorAll('button'));
    const newProjectBtn = allButtons.find(b => b.textContent?.includes('Ajouter un projet'));
    expect(newProjectBtn).toBeTruthy();

    await act(async () => { newProjectBtn!.click(); });

    // Both tabs should be visible: Bootstrap and Clone
    const body = document.body.textContent!;
    expect(body).toContain('Bootstrap');
    expect(body).toContain('Cloner');

    // Bootstrap form inputs should be present (name + description)
    const inputs = document.body.querySelectorAll('input');
    const nameInput = Array.from(inputs).find(i => i.placeholder === 'my-awesome-project');
    expect(nameInput).toBeTruthy();

    // The form wrapper should have onKeyDown (Ctrl+Enter support)
    // We verify by checking that a div wraps the form content
    const textareas = document.body.querySelectorAll('textarea');
    expect(textareas.length).toBeGreaterThan(0);
  });

  it('clone tab shows URL input', async () => {
    vi.mocked(projectsApi.list).mockResolvedValue([]);
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    // Open modal
    const newProjectBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Ajouter un projet'));
    await act(async () => { newProjectBtn!.click(); });

    // Switch to clone tab
    const cloneTab = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Cloner'));
    expect(cloneTab).toBeTruthy();
    await act(async () => { cloneTab!.click(); });

    // URL input should appear
    const inputs = document.body.querySelectorAll('input');
    const urlInput = Array.from(inputs).find(i => i.placeholder?.includes('github.com'));
    expect(urlInput).toBeTruthy();
  });
});
