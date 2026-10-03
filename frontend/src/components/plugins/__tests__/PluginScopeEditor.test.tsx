import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../../lib/I18nContext';
import type { HostSyncMode, Project } from '../../../types/generated';
import { useT } from '../../../lib/I18nContext';
import { PluginScopeEditor } from '../PluginScopeEditor';

const projects = [{
  id: 'project-1',
  name: 'Website',
  path: '/repos/website',
}] as Project[];

function ScopeHarness() {
  const { t } = useT();
  const [isGlobal, setIsGlobal] = useState(true);
  const [projectIds, setProjectIds] = useState(['project-1']);
  const [hostSync, setHostSync] = useState<HostSyncMode>('None');

  return (
    <PluginScopeEditor
      t={t}
      projects={projects}
      isGlobal={isGlobal}
      onToggleGlobal={() => setIsGlobal(value => !value)}
      includeGeneral
      onToggleGeneral={() => undefined}
      projectIds={projectIds}
      onToggleProject={(projectId, linked) => setProjectIds(previous => (
        linked ? previous.filter(id => id !== projectId) : [...previous, projectId]
      ))}
      supportsHostSync
      hostSync={hostSync}
      onSetHostSync={setHostSync}
    />
  );
}

function LegacyHostScope({ onSetHostSync }: { onSetHostSync: (mode: HostSyncMode) => void }) {
  const { t } = useT();
  return (
    <PluginScopeEditor
      t={t}
      projects={[]}
      isGlobal
      onToggleGlobal={() => undefined}
      includeGeneral
      onToggleGeneral={() => undefined}
      projectIds={[]}
      onToggleProject={() => undefined}
      supportsHostSync
      hostSync="MirrorAll"
      onSetHostSync={onSetHostSync}
    />
  );
}

describe('PluginScopeEditor', () => {
  it('keeps project scope read-only in all-project mode without losing the saved selection', () => {
    render(<I18nProvider><ScopeHarness /></I18nProvider>);

    expect(screen.getByRole('button', { name: 'Website' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Tous les projets' }));
    const project = screen.getByRole('button', { name: 'Website' });
    expect(project).toBeEnabled();
    expect(project).toHaveAttribute('aria-pressed', 'true');
  });

  it('separates local CLI sync and shows the host-file preview only after opt-in', () => {
    render(<I18nProvider><ScopeHarness /></I18nProvider>);

    expect(screen.queryByText(/\.codex\/config\.toml/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Aussi disponible dans mes CLIs locaux' }));
    expect(screen.getByText(/\.codex\/config\.toml/)).toBeVisible();
  });

  it('does not rewrite a legacy MirrorAll value unless the operator changes the CLI toggle', () => {
    const onSetHostSync = vi.fn();
    render(<I18nProvider><LegacyHostScope onSetHostSync={onSetHostSync} /></I18nProvider>);

    const toggle = screen.getByRole('checkbox', { name: 'Aussi disponible dans mes CLIs locaux' });
    expect(toggle).toBeChecked();
    expect(onSetHostSync).not.toHaveBeenCalled();
    fireEvent.click(toggle);
    expect(onSetHostSync).toHaveBeenCalledWith('None');
  });
});
