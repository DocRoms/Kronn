// KT-1878 — Settings > Design system shows every token of the design scales
// with its name, its value in the current theme and its use. A token of a
// scale that tokens.css declares but the page does not show fails here.
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DesignSystemSection, DESIGN_SYSTEM_SECTION_ID } from '../DesignSystemSection';
import { DESIGN_TOKEN_GROUPS, DESIGN_TOKENS } from '../../../styles/designTokens';
import { loadThemes } from '../../../styles/themeAudit';
import { dictionaries } from '../../../lib/i18n/testing';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const t = (key: string) => key;
const root = document.documentElement;

afterEach(() => {
  cleanup();
  root.removeAttribute('style');
  root.removeAttribute('data-theme');
});

const rowOf = (name: string) => document.querySelector(`tr[data-token="${name}"]`) as HTMLElement | null;

describe('DesignSystemSection', () => {
  it('is the design-system anchor of Settings', () => {
    render(<DesignSystemSection t={t} />);
    expect(DESIGN_SYSTEM_SECTION_ID).toBe('design-system');
    expect(document.getElementById('design-system')).toBe(screen.getByTestId('design-system-section'));
  });

  it('shows every scale tokens.css declares, in every theme, with its name and use', () => {
    render(<DesignSystemSection t={t} />);
    for (const [theme, tokens] of loadThemes(SRC)) {
      const declared = [...tokens.keys()].filter(name => DESIGN_TOKEN_GROUPS.some(g => g.match.test(name)));
      const missing = declared.filter(name => !rowOf(name));
      expect(missing, `${theme}: tokens of the scales the page does not show`).toEqual([]);
    }
    for (const token of DESIGN_TOKENS) {
      const row = rowOf(token.name)!;
      expect(row, token.name).toBeTruthy();
      expect(within(row).getByText(token.name)).toBeInTheDocument();
      expect(within(row).getByText(token.use)).toBeInTheDocument();
    }
  });

  it('groups the tokens by scale, each under its title, in order', () => {
    render(<DesignSystemSection t={t} />);
    const titles = screen.getAllByRole('heading', { level: 3 }).map(h => h.textContent);
    expect(titles).toEqual(DESIGN_TOKEN_GROUPS.map(g => g.title));
    for (const group of DESIGN_TOKEN_GROUPS) {
      const section = document.querySelector(`section[data-scale="${group.scale}"]`) as HTMLElement;
      const rows = [...section.querySelectorAll('tbody tr')].map(r => r.getAttribute('data-token'));
      expect(rows).toEqual(group.tokens.map(token => token.name));
    }
  });

  it('shows a text size in rem and the px it computes to at the current root size', async () => {
    root.style.setProperty('--kr-type-sm', '0.75rem');
    root.style.setProperty('--kr-type-2xl', '1.25rem');
    render(<DesignSystemSection t={t} />);
    expect(screen.getByTestId('ds-value---kr-type-sm')).toHaveTextContent('0.75rem · 12px');
    expect(screen.getByTestId('ds-value---kr-type-2xl')).toHaveTextContent('1.25rem · 20px');

    // The reader's larger text size: the same token, a larger px.
    root.style.fontSize = '20px';
    await waitFor(() => expect(screen.getByTestId('ds-value---kr-type-sm')).toHaveTextContent('0.75rem · 15px'));
  });

  it('follows the theme: the value shown is the one the document computes now', async () => {
    root.style.setProperty('--kr-status-success', '#73c991');
    render(<DesignSystemSection t={t} />);
    expect(screen.getByTestId('ds-value---kr-status-success')).toHaveTextContent('#73c991');

    root.style.setProperty('--kr-status-success', '#15803d');
    root.setAttribute('data-theme', 'light');
    await waitFor(() => expect(screen.getByTestId('ds-value---kr-status-success')).toHaveTextContent('#15803d'));
  });

  it('labels every scale and every use in the four locales', () => {
    const keys = [
      'designSystem.link', 'designSystem.title', 'designSystem.hint',
      'designSystem.column.token', 'designSystem.column.value', 'designSystem.column.use', 'designSystem.column.preview',
      ...DESIGN_TOKEN_GROUPS.map(g => g.title),
      ...DESIGN_TOKENS.map(token => token.use),
    ];
    for (const [locale, dict] of Object.entries(dictionaries)) {
      const missing = keys.filter(key => !(key in dict) || !dict[key].trim());
      expect(missing, `${locale} labels`).toEqual([]);
    }
  });
});
