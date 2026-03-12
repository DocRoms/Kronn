import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, act, cleanup } from '@testing-library/react';
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

import { discussions as discussionsApi } from '../../lib/api';
import { Dashboard } from '../Dashboard';
import type { Discussion } from '../../types/generated';

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
  return result!;
};

/**
 * Simulates a discussion as returned by the list endpoint:
 * messages is empty, message_count has the real count.
 * This matches the real backend behavior (list doesn't load messages).
 */
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
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

describe('Dashboard — unseen badge & document.title', () => {
  it('shows unseen badge on discussions tab when on another page', async () => {
    // Mock API to return discussions with messages
    const disc = makeDiscussion('d1', 3);
    vi.mocked(discussionsApi.list).mockResolvedValue([disc]);

    // No lastSeenMsgCount → all 3 messages are "unseen"
    await wrap(<Dashboard onReset={vi.fn()} />);

    // We start on the "projects" page, not discussions
    // The badge should show unseen count
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
    // Mark all messages as seen
    localStorage.setItem('kronn:lastSeenMsgCount', JSON.stringify({ d1: 3 }));

    await wrap(<Dashboard onReset={vi.fn()} />);

    expect(document.title).toBe('Kronn');
  });

  it('refetches discussions when tab becomes visible', async () => {
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await wrap(<Dashboard onReset={vi.fn()} />);

    // Reset call count after initial render
    vi.mocked(discussionsApi.list).mockClear();

    // Simulate tab becoming visible
    const original = document.visibilityState;
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
    });

    expect(vi.mocked(discussionsApi.list)).toHaveBeenCalled();

    // Restore original value
    Object.defineProperty(document, 'visibilityState', { value: original, configurable: true });
  });
});
