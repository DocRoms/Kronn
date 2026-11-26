import { readFileSync } from 'node:fs';
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { LivePageAction } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  discussionActions: { get: vi.fn(), cancel: vi.fn(), launch: vi.fn() },
  pages: { getAction: vi.fn(), cancelAction: vi.fn(), launchAction: vi.fn() },
  workflows: { get: vi.fn() },
  agents: { detect: vi.fn() },
  config: { getAgentAccess: () => Promise.resolve({}) },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));
vi.mock('../RunStatusCard', () => ({ RunStatusCard: () => null }));

import { LivePageActionCard } from '../LivePageActionCard';
import { resolveActionSkin, extractActionSkin } from '../../lib/live-page-action-skin';

const proposed: LivePageAction = {
  id: 'page-action:page-1:go', live_page_id: 'page-1', live_page_revision_id: 'rev-1', action_ref: 'go',
  kind: 'quick_exec', target_id: 'qe-1', target_name: 'Draw sprite', project_id: 'p', project_name: 'Studio',
  state: 'proposed', values: [], shared_run_id: null, result_discussion_id: null, deep_link: null,
  diagnostic: null, launched_at: null, finished_at: null, created_at: '2026-09-01T08:00:00Z',
  updated_at: '2026-09-01T08:00:00Z', stale_source: false, binding_key: null,
};
const noop = vi.fn();
const html = `<script type="application/json" kronn-action-skin>{"surface":"#1a1330","text":"#f3ecff",
"accent":"#ffcc33","accent-text":"#1a1330","radius":0,"border-width":3,"font":"pixel",
"label":"HACKED","title":"Free text"}</script>`;

describe('LivePageActionCard skin', () => {
  it('wraps the card in the page skin, with Kronn labels and Launch untouched', () => {
    const vars = resolveActionSkin(extractActionSkin(html), 'dark');
    const { container } = render(<LivePageActionCard action={proposed} skin={vars} onChanged={noop} onOpenDiscussion={noop} />);
    const wrapper = container.querySelector('.live-page-action-skin') as HTMLElement;
    expect(wrapper.dataset.skinned).toBe('true');
    expect(wrapper.style.getPropertyValue('--kac-accent')).toBe('rgb(255, 204, 51)');
    expect(wrapper.style.getPropertyValue('--kac-radius')).toBe('0px');
    expect(wrapper.querySelector('.discussion-action-card')).not.toBeNull();
    // Kronn's text is the only text: the skin cannot replace a label.
    expect(wrapper.textContent).not.toMatch(/HACKED|Free text/);
    expect(screen.getByText('disc.action.proposal')).toBeVisible();
    expect(screen.getByRole('button', { name: /disc\.action\.launch/ })).toBeEnabled();
    expect(wrapper.getAttribute('style')).not.toMatch(/url\(/);
  });

  it('adds no styling at all without a skin', () => {
    for (const skin of [undefined, null, {}]) {
      const { container, unmount } = render(<LivePageActionCard action={proposed} skin={skin} onChanged={noop} onOpenDiscussion={noop} />);
      const wrapper = container.querySelector('.live-page-action-skin') as HTMLElement;
      expect(wrapper.hasAttribute('data-skinned')).toBe(false);
      expect(wrapper.getAttribute('style')).toBeNull();
      unmount();
    }
  });

  it('keeps the skin sheet scoped to the Page wrapper and never hides anything', () => {
    const css = readFileSync('src/components/LivePageActionSkin.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    const selectors = css.split('{').slice(0, -1).map(chunk => chunk.split('}').pop()!.trim()).flatMap(s => s.split(','));
    expect(selectors.length).toBeGreaterThan(10);
    for (const selector of selectors) expect(selector.trim()).toMatch(/^\.live-page-action-skin /);
    expect(css).not.toMatch(/display\s*:\s*none|visibility|opacity|pointer-events|position\s*:\s*absolute/);
  });
});
