// KT-2020 — every plugin kind names its badge with keys that exist in the
// four locales (a CLI card used to fall back to "MCP" and show the raw key
// `mcp.kind.cliTooltip` as its tooltip).
import { describe, expect, it } from 'vitest';
import { PLUGIN_KIND_META } from '../pluginKindMeta';
import { dictionaries } from '../../../lib/i18n/testing';
import type { PluginKind } from '../../../lib/pluginKind';

const KINDS: PluginKind[] = ['mcp', 'api', 'hybrid', 'cli'];

describe('PLUGIN_KIND_META (KT-2020)', () => {
  it('has one entry per plugin kind, and nothing else', () => {
    expect(Object.keys(PLUGIN_KIND_META).sort()).toEqual([...KINDS].sort());
  });

  it.each(Object.entries(dictionaries))('translates every label and tooltip key in %s', (_locale, dict) => {
    const missing: string[] = [];
    for (const kind of KINDS) {
      const { label, tooltip, rowLabel, rowTooltip } = PLUGIN_KIND_META[kind];
      for (const key of [label, tooltip, rowLabel, rowTooltip]) {
        if (!(dict as Record<string, string>)[key]?.trim()) missing.push(`${kind}: ${key}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it('gives a CLI plugin its own label, tooltip and the terminal icon', () => {
    const cli = PLUGIN_KIND_META.cli;
    expect(dictionaries.en[cli.label as keyof typeof dictionaries.en]).toBe('CLI');
    expect(cli.Icon).not.toBe(PLUGIN_KIND_META.mcp.Icon);
    expect(cli.Icon.displayName).toMatch(/terminal/i);
  });
});
