// KT-1706 — what the catalogue says about the pickers of a card is ONE muted
// line inside the « Models and fallbacks » block, in plain words (never the raw
// reason code), and a picker keeps its full label in a tooltip.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { SearchableSelect } from '../../SearchableSelect';
import { AgentFallbackControl } from '../AgentFallbackControl';
import { CatalogHint } from '../CatalogHint';
import { I18nProvider } from '../../../lib/I18nContext';

afterEach(cleanup);

const t = (key: string) => key;
const base = { t, loading: false, failed: false, empty: false, stale: false, onReload: () => {} };

describe('CatalogHint', () => {
  it('says nothing when the catalogue has nothing to report', () => {
    const { container } = render(<CatalogHint {...base} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('says what to do for a stale catalogue instead of printing the reason code', () => {
    render(<CatalogHint {...base} stale reason="provider_error" />);
    const hint = screen.getByTestId('catalog-hint');
    expect(hint).toHaveTextContent('modelCatalog.staleHint.provider_error');
    expect(hint).not.toHaveTextContent(/—\s*provider_error/);
  });

  it('falls back to a generic sentence for a reason it does not know', () => {
    render(<CatalogHint {...base} stale reason="something_new" />);
    expect(screen.getByTestId('catalog-hint')).toHaveTextContent('modelCatalog.staleHint.unknown');
  });

  it('keeps an empty catalogue and a stale one on the same single line', () => {
    render(<CatalogHint {...base} empty stale reason="auth_required" />);
    const hints = screen.getAllByTestId('catalog-hint');
    expect(hints).toHaveLength(1);
    expect(hints[0]).toHaveTextContent('modelCatalog.empty');
    expect(hints[0]).toHaveTextContent('modelCatalog.staleHint.auth_required');
  });

  it('offers the reload next to a load failure', () => {
    const onReload = vi.fn();
    render(<CatalogHint {...base} failed onReload={onReload} />);
    fireEvent.click(screen.getByRole('button', { name: 'modelCatalog.reload' }));
    expect(onReload).toHaveBeenCalledTimes(1);
  });
});

describe('the hint sits inside the block', () => {
  it('is drawn under the title of the block, before the first row', () => {
    render(
      <I18nProvider>
        <AgentFallbackControl
          agentType="ClaudeCode"
          modes={[{ mode: 'standard', model: 'sonnet' }]}
          hint={<p data-testid="the-hint">hint</p>}
          options={[]}
          chains={[]}
          onChange={async () => {}}
        />
      </I18nProvider>,
    );
    const block = screen.getByTestId('agent-fallback-ClaudeCode');
    const hint = screen.getByTestId('the-hint');
    expect(block).toContainElement(hint);
    const row = screen.getByTestId('agent-fallback-ClaudeCode-standard');
    expect(hint.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe('model picker tooltip', () => {
  const props = { options: [{ value: 'gemini-2.5-flash', label: 'gemini-2.5-flash' }], onChange: () => {}, label: 'Model', emptyLabel: 'none' };

  it('carries the full label of the chosen model', () => {
    render(<SearchableSelect {...props} value="gemini-2.5-flash" placeholder="auto" />);
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveAttribute('title', 'gemini-2.5-flash');
  });

  it('carries the full placeholder (the auto-picked model) when nothing is chosen', () => {
    render(<SearchableSelect {...props} value="" placeholder="gemini-2.5-flash (automatic)" />);
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveAttribute('title', 'gemini-2.5-flash (automatic)');
  });
});
