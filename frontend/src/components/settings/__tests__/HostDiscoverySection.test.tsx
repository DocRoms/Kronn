// Unit tests for HostDiscoverySection — Phase 1 of inbound/outbound MCP feature.
// Covers: empty state, grouping by scope, ownership badge rendering, error path.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { DiscoveredHostMcp } from '../../../types/generated';

const { hostDiscoveryMock, adoptHostMock } = vi.hoisted(() => ({
  hostDiscoveryMock: vi.fn(),
  adoptHostMock: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  mcps: {
    hostDiscovery: hostDiscoveryMock as never,
    adoptHost: adoptHostMock as never,
  },
}));

import { HostDiscoverySection } from '../HostDiscoverySection';

const t = (key: string) => key;

function mkEntry(over: Partial<DiscoveredHostMcp> & { name: string; scope: DiscoveredHostMcp['scope'] }): DiscoveredHostMcp {
  return {
    source_file: over.source_file ?? '/tmp/fake',
    scope: over.scope,
    name: over.name,
    transport: over.transport ?? { Stdio: { command: 'npx', args: ['-y', 'pkg'] } },
    env_keys: over.env_keys ?? [],
    managed_by_kronn: over.managed_by_kronn ?? { type: 'NotManaged' },
  };
}

describe('HostDiscoverySection', () => {
  beforeEach(() => {
    hostDiscoveryMock.mockReset();
    adoptHostMock.mockReset();
  });

  it('renders empty state when no MCPs are discovered', async () => {
    hostDiscoveryMock.mockResolvedValue([]);
    render(<HostDiscoverySection t={t} />);
    await waitFor(() => {
      expect(screen.getByText(/Aucun MCP détecté/)).toBeInTheDocument();
    });
  });

  it('renders entries grouped by scope with correct counts', async () => {
    hostDiscoveryMock.mockResolvedValue([
      mkEntry({ name: 'linear', scope: { kind: 'ClaudeUser' } }),
      mkEntry({ name: 'github', scope: { kind: 'ClaudeUser' } }),
      mkEntry({ name: 'fs', scope: { kind: 'Codex' } }),
    ]);
    render(<HostDiscoverySection t={t} />);
    await waitFor(() => {
      expect(screen.getByText('linear')).toBeInTheDocument();
      expect(screen.getByText('github')).toBeInTheDocument();
      expect(screen.getByText('fs')).toBeInTheDocument();
    });
    // Two scope groups
    expect(screen.getByText(/Claude Code.*global/)).toBeInTheDocument();
    expect(screen.getByText(/Codex.*config\.toml/)).toBeInTheDocument();
    // Total summary shows 3 detected
    expect(screen.getByText(/3 MCPs détectés/)).toBeInTheDocument();
  });

  it('shows the "managed by Kronn" badge for managed entries', async () => {
    hostDiscoveryMock.mockResolvedValue([
      mkEntry({
        name: 'linear',
        scope: { kind: 'ClaudeUser' },
        managed_by_kronn: { type: 'ManagedByMarker', config_id: 'uuid-abc' },
      }),
      mkEntry({
        name: 'rogue',
        scope: { kind: 'ClaudeUser' },
        managed_by_kronn: { type: 'NotManaged' },
      }),
    ]);
    render(<HostDiscoverySection t={t} />);
    await waitFor(() => {
      expect(screen.getByText('linear')).toBeInTheDocument();
    });
    expect(screen.getByText(/Géré par Kronn/)).toBeInTheDocument();
    expect(screen.getByText(/Externe/)).toBeInTheDocument();
    // Summary text is split across <strong> tags. Match the line with both counts.
    const summary = screen.getByText(/géré.*externe/).textContent ?? '';
    expect(summary).toContain('géré');
    expect(summary).toContain('externe');
  });

  it('renders error message when API fails', async () => {
    hostDiscoveryMock.mockRejectedValue(new Error('Backend unreachable'));
    render(<HostDiscoverySection t={t} />);
    await waitFor(() => {
      expect(screen.getByText('Backend unreachable')).toBeInTheDocument();
    });
  });

  it('shows Importer button only for NotManaged entries', async () => {
    hostDiscoveryMock.mockResolvedValue([
      mkEntry({
        name: 'managed-one',
        scope: { kind: 'ClaudeUser' },
        managed_by_kronn: { type: 'ManagedByMarker', config_id: 'uuid' },
      }),
      mkEntry({
        name: 'rogue-one',
        scope: { kind: 'ClaudeUser' },
        managed_by_kronn: { type: 'NotManaged' },
      }),
    ]);
    const { container } = render(<HostDiscoverySection t={t} />);
    await waitFor(() => {
      expect(screen.getByText('rogue-one')).toBeInTheDocument();
    });
    const adoptButtons = container.querySelectorAll('button');
    const adoptVisible = Array.from(adoptButtons).filter(b => b.textContent?.includes('Importer'));
    // Exactly 1 Importer button (the rogue-one — not the managed entry)
    expect(adoptVisible).toHaveLength(1);
  });

  it('opens confirmation modal and warns the source file is not modified', async () => {
    const { fireEvent } = await import('@testing-library/react');
    hostDiscoveryMock.mockResolvedValue([
      mkEntry({
        name: 'rogue',
        scope: { kind: 'ClaudeUser' },
        source_file: '/home/me/.claude.json',
        managed_by_kronn: { type: 'NotManaged' },
      }),
    ]);
    render(<HostDiscoverySection t={t} />);
    await waitFor(() => {
      expect(screen.getByText('rogue')).toBeInTheDocument();
    });
    fireEvent.click(screen.getByText(/Importer/));
    // Modal opens with explicit warning
    await waitFor(() => {
      expect(screen.getByText(/Importer "rogue" dans Kronn/)).toBeInTheDocument();
    });
    // The "n'est pas modifié" phrase is broken across <strong> tags now.
    // Match the broader context instead — "Ce qui va se passer" + source file.
    expect(screen.getByText(/Ce qui va se passer/)).toBeInTheDocument();
    expect(screen.getAllByText('/home/me/.claude.json').length).toBeGreaterThanOrEqual(1);
  });

  it('calls adoptHost API and refreshes on confirm', async () => {
    const { fireEvent } = await import('@testing-library/react');
    hostDiscoveryMock
      .mockResolvedValueOnce([
        mkEntry({ name: 'linear', scope: { kind: 'ClaudeUser' }, managed_by_kronn: { type: 'NotManaged' } }),
      ])
      .mockResolvedValueOnce([
        mkEntry({ name: 'linear', scope: { kind: 'ClaudeUser' }, managed_by_kronn: { type: 'ManagedByMarker', config_id: 'newly-adopted' } }),
      ]);
    adoptHostMock.mockResolvedValue({ id: 'newly-adopted' });

    render(<HostDiscoverySection t={t} />);
    await waitFor(() => expect(screen.getByText(/Importer/)).toBeInTheDocument());
    fireEvent.click(screen.getByText(/Importer/));
    await waitFor(() => expect(screen.getByText(/dans Kronn ?/)).toBeInTheDocument());

    // Click the modal's "Importer" button (now there are 2 — one in row, one in modal)
    const buttons = screen.getAllByRole('button').filter(b => b.textContent === 'Importer');
    fireEvent.click(buttons[buttons.length - 1]);

    await waitFor(() => expect(adoptHostMock).toHaveBeenCalledTimes(1));
    expect(adoptHostMock.mock.calls[0][0]).toMatchObject({
      name: 'linear',
      source_file: expect.any(String),
    });
    // After adopt, refresh shows the entry as managed (no more Importer button)
    await waitFor(() => {
      expect(screen.getByText(/Géré par Kronn/)).toBeInTheDocument();
    });
  });

  it('groups ClaudeLocal entries by project_path', async () => {
    hostDiscoveryMock.mockResolvedValue([
      mkEntry({ name: 'linear-a', scope: { kind: 'ClaudeLocal', value: { project_path: '/repo-a' } } }),
      mkEntry({ name: 'linear-b', scope: { kind: 'ClaudeLocal', value: { project_path: '/repo-b' } } }),
    ]);
    render(<HostDiscoverySection t={t} />);
    await waitFor(() => {
      expect(screen.getByText('linear-a')).toBeInTheDocument();
      expect(screen.getByText('linear-b')).toBeInTheDocument();
    });
    expect(screen.getByText(/project-scoped: \/repo-a/)).toBeInTheDocument();
    expect(screen.getByText(/project-scoped: \/repo-b/)).toBeInTheDocument();
  });
});
