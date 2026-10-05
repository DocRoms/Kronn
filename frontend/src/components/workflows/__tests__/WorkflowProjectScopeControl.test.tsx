import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key, locale: 'en', setLocale: () => {} }),
}));

import { WorkflowProjectScopeControl } from '../WorkflowProjectScopeControl';

const projects = [
  { id: 'home', name: 'Front Euronews' },
  { id: 'apollo', name: 'Front Apollo' },
  { id: 'api', name: 'API' },
];

describe('WorkflowProjectScopeControl (KT-851)', () => {
  it('switches between this project only, all projects and chosen projects', () => {
    const onChange = vi.fn();
    render(<WorkflowProjectScopeControl value={null} onChange={onChange} projects={projects} homeProjectId="home" />);
    expect(screen.getByLabelText('wiz.projectScope.home')).toBeChecked();
    fireEvent.click(screen.getByLabelText('wiz.projectScope.all'));
    expect(onChange).toHaveBeenLastCalledWith({ type: 'All' });
    fireEvent.click(screen.getByLabelText('wiz.projectScope.chosen'));
    expect(onChange).toHaveBeenLastCalledWith({ type: 'Projects', project_ids: [] });
    expect(screen.getByText('wiz.projectScope.hint')).toBeInTheDocument();
  });

  it('lists the other projects, the home one being always served', () => {
    const onChange = vi.fn();
    render(
      <WorkflowProjectScopeControl
        value={{ type: 'Projects', project_ids: ['api'] }}
        onChange={onChange}
        projects={projects}
        homeProjectId="home"
      />,
    );
    expect(screen.queryByLabelText('Front Euronews')).not.toBeInTheDocument();
    expect(screen.getByLabelText('API')).toBeChecked();
    fireEvent.click(screen.getByLabelText('Front Apollo'));
    expect(onChange).toHaveBeenLastCalledWith({ type: 'Projects', project_ids: ['api', 'apollo'] });
    fireEvent.click(screen.getByLabelText('API'));
    expect(onChange).toHaveBeenLastCalledWith({ type: 'Projects', project_ids: [] });
    fireEvent.click(screen.getByLabelText('wiz.projectScope.home'));
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it('explains a global workflow asks for its project', () => {
    render(<WorkflowProjectScopeControl value={{ type: 'All' }} onChange={() => {}} projects={projects} homeProjectId="" />);
    expect(screen.getByLabelText('wiz.projectScope.all')).toBeChecked();
    expect(screen.getByText('wiz.projectScope.hintGlobal')).toBeInTheDocument();
  });
});
