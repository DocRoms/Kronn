import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ApiAccessPolicy, ApiEndpoint } from '../../../types/generated';
import { PluginAccessPolicyEditor } from '../PluginAccessPolicyEditor';
import { ruleForKind, sameEndpoint } from '../accessPolicy';

const t = (key: string, ...args: (string | number)[]) => [key, ...args].join(' ');
const endpoints: ApiEndpoint[] = [
  { method: 'GET', path: '/users/me', description: '' },
  { method: 'GET', path: '/pages/{page_id}', description: '' },
];

function renderEditor(policy: ApiAccessPolicy | null, onSave = vi.fn().mockResolvedValue(true)) {
  render(<PluginAccessPolicyEditor t={t} serverId="custom-notion" endpoints={endpoints} policy={policy} onSave={onSave} />);
  return onSave;
}

describe('PluginAccessPolicyEditor', () => {
  it('saves a local-only plugin with one open endpoint', async () => {
    const onSave = renderEditor(null);
    fireEvent.click(screen.getByTestId('mcp-access-enable'));
    fireEvent.change(screen.getByTestId('mcp-access-plugin'), { target: { value: 'local_only' } });
    fireEvent.change(screen.getByLabelText('mcp.access.endpointMode GET /users/me'), { target: { value: 'own' } });
    fireEvent.change(screen.getByTestId('mcp-access-endpoint-GET-/users/me'), { target: { value: 'all' } });
    fireEvent.click(screen.getByTestId('mcp-access-save'));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(onSave).toHaveBeenCalledWith('custom-notion', {
      access: { kind: 'local_only' },
      endpoints: [{ method: 'GET', path: '/users/me', access: { kind: 'all' } }],
    });
  });

  it('restricts to chosen agents, with an optional model', async () => {
    const onSave = renderEditor({ access: { kind: 'all' }, endpoints: [] });
    fireEvent.change(screen.getByTestId('mcp-access-plugin'), { target: { value: 'agents' } });
    fireEvent.click(screen.getByLabelText('Ollama'));
    fireEvent.change(screen.getByLabelText('mcp.access.model Ollama'), { target: { value: 'qwen3:8b' } });
    fireEvent.click(screen.getByTestId('mcp-access-save'));
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][1]).toEqual({
      access: { kind: 'agents', agents: [{ agent: 'Ollama', model: 'qwen3:8b' }] },
      endpoints: [],
    });
  });

  it('removes the policy when restriction is turned off', async () => {
    const onSave = renderEditor({ access: { kind: 'blocked' }, endpoints: [] });
    fireEvent.click(screen.getByTestId('mcp-access-enable'));
    fireEvent.click(screen.getByTestId('mcp-access-save'));
    await waitFor(() => expect(onSave).toHaveBeenCalledWith('custom-notion', null));
  });

  it('says the MCP side of a hybrid plugin keeps its own access', () => {
    render(<PluginAccessPolicyEditor t={t} serverId="s" endpoints={endpoints} policy={null} onSave={vi.fn()} isHybrid />);
    expect(screen.getByTestId('mcp-access-hybrid')).toHaveTextContent('mcp.access.hybridNote');
  });

  it('shows no hybrid note on an API-only plugin', () => {
    renderEditor(null);
    expect(screen.queryByTestId('mcp-access-hybrid')).toBeNull();
  });

  it('keeps save disabled until something changes', () => {
    renderEditor({ access: { kind: 'all' }, endpoints: [] });
    expect(screen.getByTestId('mcp-access-save')).toBeDisabled();
  });

  it('matches endpoints like the backend and keeps agents when switching kind', () => {
    expect(sameEndpoint({ method: 'get', path: 'users/me/' }, { method: 'GET', path: '/users/me' })).toBe(true);
    expect(sameEndpoint({ method: 'POST', path: '/users/me' }, { method: 'GET', path: '/users/me' })).toBe(false);
    const agents = { kind: 'agents' as const, agents: [{ agent: 'Codex' as const }] };
    expect(ruleForKind('agents', agents)).toEqual(agents);
    expect(ruleForKind('blocked', agents)).toEqual({ kind: 'blocked' });
  });
});
