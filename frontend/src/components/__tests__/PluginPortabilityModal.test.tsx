import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import type { McpConfigDisplay, McpDefinition, PluginBundlePreview, Project } from '../../types/generated';

const mocks = vi.hoisted(() => ({
  previewBundle: vi.fn(),
  exportBundle: vi.fn(),
  importBundle: vi.fn(),
  previewImportBundle: vi.fn(),
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
      previewImportBundle: mocks.previewImportBundle,
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
  interfaces: ['api', 'mcp', 'cli'],
  effective_kind: 'cli',
  effective_preferred_interface: 'api',
  credential_source: 'cli_token',
  last_probes: [],
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
  path: '/repos/website',
} as Project;

const registry: McpDefinition[] = [{
  id: 'mcp-fastly',
  name: 'Fastly',
  description: 'Fastly MCP',
  transport: { Stdio: { command: 'fastly-mcp', args: [] } },
  env_keys: [],
  tags: [],
  token_url: null,
  token_help: null,
  publisher: 'Fastly',
  official: true,
}];

const renderModal = (mode: 'export' | 'import') => render(
  <I18nProvider>
    <PluginPortabilityModal
      mode={mode}
      configs={[config]}
      registry={registry}
      projects={[project]}
      onClose={vi.fn()}
      onImported={vi.fn()}
    />
  </I18nProvider>,
);

const plainImportPreview = (over: Record<string, unknown> = {}) => ({
  bundle_id: 'bundle-1',
  already_imported: false,
  includes_values: false,
  legacy: false,
  plugins: [{
    source_config_id: 'source-1',
    label: 'Fastly production',
    server_name: 'Fastly',
    usual_args: null,
    proposed_args: null,
    args_differ: false,
    importable: true,
    issue: null,
  }],
  ...over,
});

describe('PluginPortabilityModal', () => {
  beforeEach(() => {
    mocks.previewImportBundle.mockReset();
    mocks.previewImportBundle.mockResolvedValue(plainImportPreview());
  });

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
    expect(importButton).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Examiner le lot' }));
    await waitFor(() => expect(importButton).not.toBeDisabled());
    expect(mocks.previewImportBundle).toHaveBeenCalledWith(expect.objectContaining({ passphrase: 'long-passphrase' }));
    fireEvent.click(importButton);

    await waitFor(() => expect(mocks.importBundle).toHaveBeenCalledWith({
      content: expect.stringContaining('"kind":"kronn.plugins"'),
      passphrase: 'long-passphrase',
      accept_args_for: [],
    }));
    expect(screen.getByRole('button', {
      name: 'Tous les projets',
    })).toHaveClass('mcp-project-toggle-on');
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

    // `configs` (unchanged in this test) never carries `imported-1`, so
    // `includeGeneral` falls back to `false` — the safe "don't guess a
    // broader visibility than what's on screen" default (KT-831).
    await waitFor(() => expect(mocks.updateConfig).toHaveBeenCalledWith(
      'imported-1',
      { is_global: true, include_general: false, host_sync: 'None' },
    ));
    expect(mocks.setConfigProjects).toHaveBeenCalledWith('imported-1', {
      project_ids: [],
    });
  });

  it('closing the import before "Appliquer la portée" still applies the displayed default scope (no orphan config, KT-352)', async () => {
    mocks.importBundle.mockResolvedValue({
      bundle_id: 'bundle-3',
      already_imported: false,
      imported_config_ids: ['imported-3'],
      imported_configs: [{
        config_id: 'imported-3',
        server_id: 'api-fastly',
        label: 'Fastly preprod',
        server_name: 'Fastly',
      }],
      skipped_plugins: 0,
      includes_values: false,
      warnings: [],
      conflicts: [],
    });
    mocks.updateConfig.mockResolvedValue(config);
    mocks.setConfigProjects.mockResolvedValue(undefined);
    const onClose = vi.fn();
    render(
      <I18nProvider>
        <PluginPortabilityModal mode="import" configs={[config]} registry={registry} projects={[project]} onClose={onClose} onImported={vi.fn()} />
      </I18nProvider>,
    );
    const bundle = new File(
      [JSON.stringify({ kind: 'kronn.plugins', encrypted: false, plugin_labels: ['Fastly preprod'] })],
      'fastly.kronn-plugins.json',
      { type: 'application/json' },
    );
    fireEvent.change(document.querySelector('input[type="file"]')!, {
      target: { files: [bundle] },
    });
    await screen.findByRole('button', { name: 'Importer le bundle' });
    fireEvent.click(screen.getByRole('button', { name: 'Importer le bundle' }));
    await screen.findByRole('button', { name: /Appliquer la portée et terminer/ });

    // Close via the header X — the operator never clicked "Appliquer".
    fireEvent.click(screen.getByLabelText('Fermer'));

    await waitFor(() => expect(mocks.updateConfig).toHaveBeenCalledWith(
      'imported-3',
      { is_global: true, include_general: false, host_sync: 'None' },
    ));
    expect(mocks.setConfigProjects).toHaveBeenCalledWith('imported-3', {
      project_ids: [],
    });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
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
    const global = await screen.findByRole('button', { name: 'Tous les projets' });
    fireEvent.click(global);
    fireEvent.click(screen.getByRole('button', { name: 'Website' }));
    fireEvent.click(screen.getByRole('button', { name: /Appliquer la portée et terminer/ }));

    await waitFor(() => expect(mocks.updateConfig).toHaveBeenCalledWith(
      'imported-2',
      { is_global: false, include_general: false, host_sync: 'None' },
    ));
    expect(mocks.setConfigProjects).toHaveBeenCalledWith('imported-2', {
      project_ids: ['project-1'],
    });
  });

  it('lets an imported MCP opt into local CLIs and persists that choice', async () => {
    mocks.importBundle.mockResolvedValue({
      bundle_id: 'bundle-mcp',
      already_imported: false,
      imported_config_ids: ['imported-mcp'],
      imported_configs: [{
        config_id: 'imported-mcp',
        server_id: 'mcp-fastly',
        label: 'Fastly MCP',
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
      [JSON.stringify({ kind: 'kronn.plugins', encrypted: false, plugin_labels: ['Fastly MCP'] })],
      'fastly-mcp.kronn-plugins.json',
      { type: 'application/json' },
    );
    fireEvent.change(document.querySelector('input[type="file"]')!, {
      target: { files: [bundle] },
    });
    fireEvent.click(await screen.findByRole('button', { name: 'Importer le bundle' }));
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Aussi disponible dans mes CLIs locaux' }));
    expect(screen.getByText(/~\/\.claude\.json/)).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: /Appliquer la portée et terminer/ }));

    await waitFor(() => expect(mocks.updateConfig).toHaveBeenCalledWith(
      'imported-mcp',
      { is_global: true, include_general: false, host_sync: 'GlobalOnly' },
    ));
  });

  it('keeps the import open when close cannot persist the displayed scope', async () => {
    mocks.importBundle.mockResolvedValue({
      bundle_id: 'bundle-failure',
      already_imported: false,
      imported_config_ids: ['imported-failure'],
      imported_configs: [{
        config_id: 'imported-failure',
        server_id: 'api-fastly',
        label: 'Fastly failure',
        server_name: 'Fastly',
      }],
      skipped_plugins: 0,
      includes_values: false,
      warnings: [],
      conflicts: [],
    });
    mocks.updateConfig.mockRejectedValueOnce(new Error('scope write failed'));
    const onClose = vi.fn();
    render(
      <I18nProvider>
        <PluginPortabilityModal mode="import" configs={[config]} registry={registry} projects={[project]} onClose={onClose} onImported={vi.fn()} />
      </I18nProvider>,
    );
    const bundle = new File(
      [JSON.stringify({ kind: 'kronn.plugins', encrypted: false, plugin_labels: ['Fastly failure'] })],
      'fastly-failure.kronn-plugins.json',
      { type: 'application/json' },
    );
    fireEvent.change(document.querySelector('input[type="file"]')!, {
      target: { files: [bundle] },
    });
    fireEvent.click(await screen.findByRole('button', { name: 'Importer le bundle' }));
    await screen.findByRole('button', { name: /Appliquer la portée et terminer/ });
    fireEvent.click(screen.getByLabelText('Fermer'));

    expect(await screen.findByText('scope write failed')).toBeVisible();
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe('PluginPortabilityModal import review (KT-1010, KT-833)', () => {
  const argsPreview = (over: Record<string, unknown> = {}) => plainImportPreview({
    plugins: [
      {
        source_config_id: 'source-a', label: 'GitHub A', server_name: 'GitHub',
        usual_args: ['-y', '@modelcontextprotocol/server-github'],
        proposed_args: ['--token', '••••', 'evil-package'], args_differ: true, importable: true, issue: null,
      },
      {
        source_config_id: 'source-b', label: 'GitHub B', server_name: 'GitHub',
        usual_args: ['-y', '@modelcontextprotocol/server-github'],
        proposed_args: ['-y', 'other'], args_differ: true, importable: true, issue: null,
      },
      {
        source_config_id: 'source-c', label: 'Fastly', server_name: 'Fastly',
        usual_args: null, proposed_args: null, args_differ: false, importable: false, issue: 'exists',
      },
    ],
    ...over,
  });

  const importReport = (over: Record<string, unknown> = {}) => ({
    bundle_id: 'bundle-1', already_imported: false, imported_config_ids: [], imported_configs: [],
    skipped_plugins: 0, includes_values: false, warnings: [], conflicts: [], ...over,
  });

  const dropFile = (body: unknown, name = 'plugins.json') => {
    fireEvent.change(document.querySelector('input[type="file"]')!, {
      target: { files: [new File([typeof body === 'string' ? body : JSON.stringify(body)], name, { type: 'application/json' })] },
    });
  };

  const clearBundle = { kind: 'kronn.plugins', encrypted: false, includes_values: false, plugin_labels: ['GitHub A', 'GitHub B'] };

  beforeEach(() => {
    mocks.previewImportBundle.mockReset();
    mocks.importBundle.mockReset();
    mocks.importBundle.mockResolvedValue(importReport());
  });

  it('shows the usual and the proposed command per plugin, with an unchecked consent box each', async () => {
    mocks.previewImportBundle.mockResolvedValue(argsPreview());
    renderModal('import');
    dropFile(clearBundle);
    const review = await screen.findByTestId('mcp-import-review');
    expect(review).toHaveTextContent('-y @modelcontextprotocol/server-github');
    expect(review).toHaveTextContent('--token •••• evil-package');
    expect(review).not.toHaveTextContent('s3cr3t');
    const boxes = screen.getAllByRole('checkbox', { name: /j.accepte ces arguments/i });
    expect(boxes).toHaveLength(2);
    boxes.forEach(box => expect(box).not.toBeChecked());
    expect(review).toHaveTextContent('une configuration avec le même plugin');
  });

  it('sends consent only for the plugins whose box is checked', async () => {
    mocks.previewImportBundle.mockResolvedValue(argsPreview());
    renderModal('import');
    dropFile(clearBundle);
    await screen.findByTestId('mcp-import-review');
    fireEvent.click(screen.getByRole('checkbox', { name: 'GitHub B : j\'accepte ces arguments' }));
    fireEvent.click(screen.getByRole('button', { name: 'Importer le bundle' }));
    await waitFor(() => expect(mocks.importBundle).toHaveBeenCalledWith(
      expect.objectContaining({ accept_args_for: ['source-b'] }),
    ));
  });

  it('sends no consent when nothing is checked', async () => {
    mocks.previewImportBundle.mockResolvedValue(argsPreview());
    renderModal('import');
    dropFile(clearBundle);
    await screen.findByTestId('mcp-import-review');
    fireEvent.click(screen.getByRole('button', { name: 'Importer le bundle' }));
    await waitFor(() => expect(mocks.importBundle).toHaveBeenCalledWith(
      expect.objectContaining({ accept_args_for: [] }),
    ));
  });

  it('cannot import before the review has loaded', async () => {
    mocks.previewImportBundle.mockReturnValue(new Promise(() => {}));
    renderModal('import');
    dropFile(clearBundle);
    await screen.findByText('GitHub A');
    expect(document.querySelector('.mcp-btn-action-primary')).toBeDisabled();
  });

  it('says so when the bundle was already imported, and lets a re-import add consent', async () => {
    mocks.previewImportBundle.mockResolvedValue(argsPreview({ already_imported: true }));
    renderModal('import');
    dropFile(clearBundle);
    const review = await screen.findByTestId('mcp-import-review');
    expect(review).toHaveTextContent('déjà été importé');
    fireEvent.click(screen.getByRole('checkbox', { name: 'GitHub A : j\'accepte ces arguments' }));
    fireEvent.click(screen.getByRole('button', { name: 'Importer le bundle' }));
    await waitFor(() => expect(mocks.importBundle).toHaveBeenCalledWith(
      expect.objectContaining({ accept_args_for: ['source-a'] }),
    ));
  });

  it('recognises an old single-plugin JSON and reviews it like any bundle', async () => {
    mocks.previewImportBundle.mockResolvedValue(plainImportPreview({ legacy: true }));
    renderModal('import');
    const old = { name: 'Legacy API', base_url: 'https://api.legacy.test', fields: [], endpoints: [], auth: 'None' };
    dropFile(old, 'legacy.kronn-plugin.json');
    expect(await screen.findByText('Legacy API')).toBeInTheDocument();
    const review = await screen.findByTestId('mcp-import-review');
    expect(review).toHaveTextContent('Export d\'un seul plugin reconnu');
    expect(mocks.previewImportBundle).toHaveBeenCalledWith(
      expect.objectContaining({ content: expect.stringContaining('"base_url"') }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Importer le bundle' }));
    await waitFor(() => expect(mocks.importBundle).toHaveBeenCalledTimes(1));
  });

  it('refuses a file that is neither a bundle nor an old plugin export', async () => {
    renderModal('import');
    dropFile({ hello: 'world' });
    expect(await screen.findByText('Ce fichier n’est pas un bundle de plugins Kronn.')).toBeInTheDocument();
    expect(mocks.previewImportBundle).not.toHaveBeenCalled();
  });

  it('shows the server error when an old plugin JSON is malformed', async () => {
    mocks.previewImportBundle.mockRejectedValue(new Error('Imported plugin: `base_url` is required'));
    renderModal('import');
    dropFile({ name: 'Broken', base_url: '' });
    expect(await screen.findByText(/base_url/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Importer le bundle' })).toBeDisabled();
  });
});
