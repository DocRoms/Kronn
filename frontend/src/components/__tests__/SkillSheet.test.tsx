import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import type { Project, Skill } from '../../types/generated';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) => (args.length > 0 ? `${key}:${args.join('|')}` : key),
  }),
}));

import { SkillCard, SkillSheet } from '../SkillSheet';

const RAW_HTML = '<script>window.leak = true</script><img src=x onerror="window.leak = true"><b>bold html</b>';
const CONTENT = `---\nname: review\ndescription: Checks pull requests\n---\n\n# Review carefully\n\nRead **every** line.\n\n${RAW_HTML}\n`;

const skill = (over: Partial<Skill> = {}): Skill => ({
  id: 'review', name: 'Review', description: 'Checks pull requests', icon: '🔍', category: 'Domain',
  content: CONTENT, is_builtin: false, token_estimate: 120, ...over,
});
const projects = [
  { id: 'alpha', name: 'Alpha', default_skill_ids: ['review'] },
  { id: 'beta', name: 'Beta', default_skill_ids: ['review', 'rust'] },
  { id: 'gamma', name: 'Gamma', default_skill_ids: ['rust'] },
] as unknown as Project[];

const noop = () => {};
const renderSheet = (over: Partial<Parameters<typeof SkillSheet>[0]> = {}) => render(
  <SkillSheet skill={skill()} projects={projects} pinned={false} onTogglePinned={noop} {...over} />,
);

afterEach(() => {
  cleanup();
  delete (window as unknown as Record<string, unknown>).leak;
});

describe('SkillSheet', () => {
  it('names the skill, describes it, gives its category and the projects that use it', () => {
    renderSheet();
    expect(screen.getByRole('heading', { level: 2, name: 'Review' })).toBeInTheDocument();
    expect(screen.getByTestId('skill-description')).toHaveTextContent('Checks pull requests');
    expect(screen.getByTestId('skill-category')).toHaveTextContent('skills.domain');
    const used = screen.getByTestId('skill-projects');
    expect(within(used).getAllByRole('listitem').map(item => item.textContent)).toEqual(['Alpha', 'Beta']);
    expect(within(used).queryByText('Gamma')).toBeNull();
  });

  it('says so when no project uses the skill', () => {
    renderSheet({ skill: skill({ id: 'orphan' }) });
    expect(within(screen.getByTestId('skill-projects')).queryAllByRole('listitem')).toHaveLength(0);
    expect(screen.getByText('automation.skill.projectsNone')).toBeInTheDocument();
  });

  it('renders the SKILL.md as Markdown, without any raw HTML', () => {
    const { container } = renderSheet();
    const content = screen.getByTestId('skill-content');
    expect(within(content).getByRole('heading', { level: 1, name: 'Review carefully' })).toBeInTheDocument();
    expect(within(content).getByText('every').tagName).toBe('STRONG');
    // The front matter stays visible, apart from the body.
    expect(within(content).getByText(/description: Checks pull requests/)).toBeInTheDocument();
    expect(container.querySelector('script, img, b, [onerror]')).toBeNull();
    expect((window as unknown as Record<string, unknown>).leak).toBeUndefined();
  });

  it('switches between Rendered and Source, Rendered being the one shown first', () => {
    renderSheet();
    const rendered = screen.getByRole('button', { name: 'projects.repositoryResources.content.rendered' });
    const source = screen.getByRole('button', { name: 'projects.repositoryResources.content.source' });
    expect(rendered).toHaveAttribute('aria-pressed', 'true');
    expect(source).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByTestId('content-rendered')).toBeInTheDocument();
    expect(screen.queryByTestId('content-source')).toBeNull();

    fireEvent.click(source);
    expect(source).toHaveAttribute('aria-pressed', 'true');
    expect(rendered).toHaveAttribute('aria-pressed', 'false');
    // The source is the file as it is: text, front matter and HTML included.
    const text = screen.getByTestId('content-source');
    expect(text.textContent).toBe(CONTENT);
    expect(text.querySelector('script, img, b')).toBeNull();
    expect(screen.queryByTestId('content-rendered')).toBeNull();

    fireEvent.click(rendered);
    expect(screen.getByTestId('content-rendered')).toBeInTheDocument();
  });

  it('shows an empty SKILL.md as empty', () => {
    renderSheet({ skill: skill({ content: '  \n' }) });
    expect(screen.getByText('projects.repositoryResources.content.empty')).toBeInTheDocument();
    expect(screen.queryByTestId('content-rendered')).toBeNull();
  });

  it('is a sheet to read: nothing to launch, no variable, no editor', () => {
    renderSheet();
    const sheet = screen.getByTestId('skill-sheet');
    expect(within(sheet).queryByRole('textbox')).toBeNull();
    expect(within(sheet).queryByRole('combobox')).toBeNull();
    expect(within(sheet).queryByRole('button', { name: /qp\.launch|qe\.run|qa\.run|wf\.launch/ })).toBeNull();
    expect(screen.getByText('automation.skill.editHint')).toBeInTheDocument();
  });

  it('leads to the existing Settings screen, where a skill is edited', () => {
    const onOpenSettings = vi.fn();
    renderSheet({ onOpenSettings });
    fireEvent.click(screen.getByRole('button', { name: 'automation.skill.openSettings' }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it('offers no Settings link when the page has nowhere to send it', () => {
    renderSheet();
    expect(screen.queryByRole('button', { name: 'automation.skill.openSettings' })).toBeNull();
  });

  it('deletes a skill the user wrote, in two steps and saying who depends on it', async () => {
    const onDelete = vi.fn().mockResolvedValue(undefined);
    renderSheet({ onDelete });
    const trash = screen.getByTestId('skill-delete-review');
    fireEvent.click(trash);
    expect(onDelete).not.toHaveBeenCalled();
    expect(await screen.findByTestId('skill-delete-review-impact')).toHaveTextContent('automation.skill.deleteImpact:2');
    await act(async () => { fireEvent.click(screen.getByTestId('skill-delete-review')); });
    expect(onDelete).toHaveBeenCalledTimes(1);
  });

  it('cannot delete a built-in skill, which is read-only', () => {
    renderSheet({ skill: skill({ is_builtin: true }) });
    expect(screen.queryByTestId('skill-delete-review')).toBeNull();
    expect(screen.getByText('automation.skill.builtinHint')).toBeInTheDocument();
    expect(screen.getByText('config.originKronn')).toBeInTheDocument();
  });

  it('links the upstream project of an external skill, and only over http(s)', () => {
    const { rerender } = renderSheet({ skill: skill({ is_builtin: true, external: true, source_url: 'https://example.com/skill' }) });
    expect(screen.getByRole('link', { name: /skills\.source/ })).toHaveAttribute('href', 'https://example.com/skill');
    expect(screen.getByRole('link', { name: /skills\.source/ })).toHaveAttribute('rel', 'noopener noreferrer');
    rerender(<SkillSheet skill={skill({ external: true, source_url: 'javascript:alert(1)' })} projects={projects} pinned={false} onTogglePinned={noop} />);
    expect(screen.queryByRole('link')).toBeNull();
  });

  it('stars the skill like any other row', () => {
    const onTogglePinned = vi.fn();
    renderSheet({ onTogglePinned });
    fireEvent.click(screen.getByRole('button', { name: 'wf.pin · Review' }));
    expect(onTogglePinned).toHaveBeenCalledTimes(1);
    cleanup();
    renderSheet({ pinned: true });
    expect(screen.getByRole('button', { name: 'wf.unpin · Review' })).toHaveAttribute('aria-pressed', 'true');
  });
});

describe('SkillCard', () => {
  it('opens the sheet from the list of the main column', () => {
    const onOpen = vi.fn();
    render(<SkillCard skill={skill()} pinned={false} onTogglePinned={noop} projectCount={2} onOpen={onOpen} />);
    expect(screen.getByText('Checks pull requests')).toBeInTheDocument();
    expect(screen.getByText('automation.skill.projectCount:2')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'automation.openResource:Review' }));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });
});
