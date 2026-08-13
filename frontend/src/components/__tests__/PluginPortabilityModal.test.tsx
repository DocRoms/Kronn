import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import type { McpConfigDisplay, PluginBundlePreview, Project } from '../../types/generated';

const mocks = vi.hoisted(() => ({
  previewBundle: vi.fn(),
  exportBundle: vi.fn(),
  importBundle: vi.fn(),
  updateConfig: vi.fn(),
  setConfigProjects: vi.fn(),
  triggerDownload: vi.fn(),
  toast: vi.fn(),
}));

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock({
    mcps: {
      previewBundle: mocks.previewBundle,
      exportBundle: mocks.exportBundle,
      importBundle: mocks.importBundle,
      updateConfig: mocks.updateConfig,
      setConfigProjects: mocks.setConfigProjects,
    },
  });
});
vi.mock('../../lib/downloadBlob', () => ({
  triggerDownload: mocks.triggerDownload,
}));
vi.mock('../../hooks/useToast', () => ({
  useToast: () => ({ toast: mocks.toast, ToastContainer: () => null }),
}));

import { PluginPortabilityModal } from '../PluginPortabilityModal';

const config: McpConfigDisplay = {
  id: 'config-fastly',
  server_id: 'mcp-fastly',
  server_name: 'Fastly',
  label: 'Fastly production',
  env_keys: ['API_TOKEN', 'SERVICE_ID'],
  env_masked: [],
  args_override: null,
  is_global: false,
  include_general: true,
  config_hash: 'hash',
  project_ids: [],
  project_names: [],
  secrets_broken: false,
  host_sync: 'None',
  preferred_interface: 'api',
};

const preview: PluginBundlePreview = {
  plugins: [{
    config_id: config.id,
    server_id: config.server_id,
    label: config.label,
    server_name: config.server_name,
    cli_credential: false,
    values: [
      { key: 'API_TOKEN', sensitive: true, exportable: true },
      { key: 'SERVICE_ID', sensitive: false, exportable: true },
    ],
  }],
  value_count: 2,
  sensitive_value_count: 1,
  confirmation_phrase: 'EXPORTER LES SECRETS',
  minimum_passphrase_length: 12,
};

const project = {
  id: 'project-1',
  name: 'Website',
} as Project;

const renderModal = (mode: 'export' | 'import') => render(
  <I18nProvider>
    <PluginPortabilityModal
      mode={mode}
      configs={[config]}
      projects={[project]}
      onClose={vi.fn()}
      onImported={vi.fn()}
    />
  </I18nProvider>,
);

describe('PluginPortabilityModal', () => {
  it('exports configuration only by default', async () => {
    mocks.previewBundle.mockResolvedValue(preview);
    const blob = new Blob(['{}'], { type: 'application/json' });
    mocks.exportBundle.mockResolvedValue({
      filename: 'plugins.kronn-plugins.json',
      blob,
    });
    renderModal('export');

    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.click(screen.getByRole('button', { name: 'Vérifier la sélection' }));
    await screen.findByText(/Export sûr/);
    fireEvent.click(screen.getByRole('button', { name: /Télécharger le bundle/ }));

    await waitFor(() => expect(mocks.exportBundle).toHaveBeenCalledWith({
      config_ids: ['config-fastly'],
      include_values: false,
      passphrase: null,
      confirmation: null,
    }));
    expect(mocks.triggerDownload).toHaveBeenCalledWith(
      'plugins.kronn-plugins.json',
      blob,
    );
  });

  it('keeps the danger export locked until confirmation and passphrase', async () => {
    mocks.previewBundle.mockResolvedValue(preview);
    mocks.exportBundle.mockResolvedValue({
      filename: 'plugins.kronn-plugins.json',
      blob: new Blob(['{}']),
    });
    renderModal('export');

    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.click(screen.getByRole('button', { name: 'Vérifier la sélection' }));
    await screen.findByText(/Export sûr/);
    fireEvent.click(screen.getByRole('checkbox', {
      name: /DANGER — inclure les valeurs/,
    }));

    const download = screen.getByRole('button', { name: /Télécharger le bundle/ });
    expect(download).toBeDisabled();
    expect(screen.getByText(/API_TOKEN · sensible/)).toBeVisible();
    const inputs = screen.getAllByRole('textbox');
    fireEvent.change(inputs[0], { target: { value: 'EXPORTER LES SECRETS' } });
    const password = document.querySelector<HTMLInputElement>('input[type="password"]')!;
    fireEvent.change(password, { target: { value: 'long-passphrase' } });
    expect(download).not.toBeDisabled();
    fireEvent.click(download);

    await waitFor(() => expect(mocks.exportBundle).toHaveBeenCalledWith({
      config_ids: ['config-fastly'],
      include_values: true,
      passphrase: 'long-passphrase',
      confirmation: 'EXPORTER LES SECRETS',
    }));
  });

  it('requires a passphrase before importing an encrypted bundle', async () => {
    mocks.importBundle.mockResolvedValue({
      bundle_id: 'bundle-1',
      already_imported: false,
      imported_config_ids: ['imported-1'],
      imported_configs: [{
        config_id: 'imported-1',
        server_id: 'api-fastly',
        label: 'Fastly production',
        server_name: 'Fastly',
      }],
      skipped_plugins: 0,
      includes_values: true,
      warnings: [],
      conflicts: [],
    });
    renderModal('import');
    const bundle = new File(
      [JSON.stringify({
        kind: 'kronn.plugins',
        encrypted: true,
        includes_values: true,
        plugin_labels: ['Fastly production'],
      })],
      'fastly.kronn-plugins.json',
      { type: 'application/json' },
    );
    fireEvent.change(document.querySelector('input[type="file"]')!, {
      target: { files: [bundle] },
    });
    await screen.findByText('Fastly production');

    const importButton = screen.getByRole('button', { name: 'Importer le bundle' });
    expect(importButton).toBeDisabled();
    fireEvent.change(document.querySelector('input[type="password"]')!, {
      target: { value: 'long-passphrase' },
    });
    fireEvent.click(importButton);

    await waitFor(() => expect(mocks.importBundle).toHaveBeenCalledWith({
      content: expect.stringContaining('"kind":"kronn.plugins"'),
      passphrase: 'long-passphrase',
    }));
    expect(screen.getByRole('checkbox', {
      name: /Global — tous les projets/,
    })).toBeChecked();
    expect(mocks.updateConfig).not.toHaveBeenCalled();
  });

  it('applies the default global scope only after explicit confirmation', async () => {
    mocks.importBundle.mockResolvedValue({
      bundle_id: 'bundle-1',
      already_imported: false,
      imported_config_ids: ['imported-1'],
      imported_configs: [{
        config_id: 'imported-1',
        server_id: 'api-fastly',
        label: 'Fastly production',
        server_name: 'Fastly',
      }],
      skipped_plugins: 0,
      includes_values: false,
      warnings: [],
      conflicts: [],
    });
    mocks.updateConfig.mockResolvedValue(config);
    mocks.setConfigProjects.mockResolvedValue(undefined);
    renderModal('import');
    const bundle = new File(
      [JSON.stringify({
        kind: 'kronn.plugins',
        encrypted: false,
        includes_values: false,
        plugin_labels: ['Fastly production'],
      })],
      'fastly.kronn-plugins.json',
      { type: 'application/json' },
    );
    fireEvent.change(document.querySelector('input[type="file"]')!, {
      target: { files: [bundle] },
    });
    fireEvent.click(await screen.findByRole('button', { name: 'Importer le bundle' }));
    const finish = await screen.findByRole('button', {
      name: /Appliquer la portée et terminer/,
    });

    expect(mocks.updateConfig).not.toHaveBeenCalled();
    fireEvent.click(finish);

    await waitFor(() => expect(mocks.updateConfig).toHaveBeenCalledWith(
      'imported-1',
      { is_global: true },
    ));
    expect(mocks.setConfigProjects).toHaveBeenCalledWith('imported-1', {
      project_ids: [],
    });
  });

  it('can replace the default global scope with selected projects', async () => {
    mocks.importBundle.mockResolvedValue({
      bundle_id: 'bundle-2',
      already_imported: false,
      imported_config_ids: ['imported-2'],
      imported_configs: [{
        config_id: 'imported-2',
        server_id: 'api-fastly',
        label: 'Fastly staging',
        server_name: 'Fastly',
      }],
      skipped_plugins: 0,
      includes_values: false,
      warnings: [],
      conflicts: [],
    });
    mocks.updateConfig.mockResolvedValue(config);
    mocks.setConfigProjects.mockResolvedValue(undefined);
    renderModal('import');
    const bundle = new File(
      [JSON.stringify({ kind: 'kronn.plugins', encrypted: false, plugin_labels: ['Fastly staging'] })],
      'fastly.kronn-plugins.json',
      { type: 'application/json' },
    );
    fireEvent.change(document.querySelector('input[type="file"]')!, {
      target: { files: [bundle] },
    });
    fireEvent.click(await screen.findByRole('button', { name: 'Importer le bundle' }));
    const global = await screen.findByRole('checkbox', { name: /Global — tous les projets/ });
    fireEvent.click(global);
    fireEvent.click(screen.getByRole('checkbox', { name: 'Website' }));
    fireEvent.click(screen.getByRole('button', { name: /Appliquer la portée et terminer/ }));

    await waitFor(() => expect(mocks.updateConfig).toHaveBeenCalledWith(
      'imported-2',
      { is_global: false },
    ));
    expect(mocks.setConfigProjects).toHaveBeenCalledWith('imported-2', {
      project_ids: ['project-1'],
    });
  });
});
