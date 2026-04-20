import { describe, it, expect } from 'vitest';

// Inline the renderTemplate function to test it (mirrors WorkflowsPage implementation)
function renderTemplate(template: string, vars: Record<string, string>): string {
  let rendered = template;
  // 1. Process conditional sections: {{#var}}content{{/var}} — removed if var is empty
  rendered = rendered.replace(/\{\{#(\w+)\}\}([\s\S]*?)\{\{\/\1\}\}/g, (_, name, content) => {
    return vars[name]?.trim() ? content : '';
  });
  // 2. Replace remaining {{var}} placeholders
  rendered = rendered.replace(/\{\{(\w+)\}\}/g, (_, name) => vars[name] ?? '');
  // 3. Clean up double spaces/commas from removed sections
  rendered = rendered.replace(/  +/g, ' ').replace(/, ,/g, ',').trim();
  return rendered;
}

describe('Quick Prompt renderTemplate', () => {
  it('replaces simple variables', () => {
    expect(renderTemplate('Analyse {{ticket}}', { ticket: 'PROJ-123' }))
      .toBe('Analyse PROJ-123');
  });

  it('leaves empty variables as empty string', () => {
    expect(renderTemplate('Analyse {{ticket}}', {}))
      .toBe('Analyse');
  });

  it('handles multiple variables', () => {
    expect(renderTemplate('{{action}} ticket {{ticket}} on {{repo}}', {
      action: 'Review', ticket: 'PROJ-456', repo: 'acme-frontend',
    })).toBe('Review ticket PROJ-456 on acme-frontend');
  });

  it('renders conditional section when variable is filled', () => {
    expect(renderTemplate('Analyse {{#jira}}le ticket {{jira}} {{/jira}}maintenant', { jira: 'PROJ-1' }))
      .toBe('Analyse le ticket PROJ-1 maintenant');
  });

  it('removes conditional section when variable is empty', () => {
    expect(renderTemplate('Analyse {{#jira}}le ticket {{jira}}, {{/jira}}maintenant', {}))
      .toBe('Analyse maintenant');
  });

  it('handles multiple conditional sections', () => {
    const tpl = '{{#jira}}Ticket: {{jira}}. {{/jira}}{{#pr}}PR: #{{pr}}. {{/pr}}Go.';
    expect(renderTemplate(tpl, { jira: 'PROJ-1', pr: '42' }))
      .toBe('Ticket: PROJ-1. PR: #42. Go.');
    expect(renderTemplate(tpl, { jira: 'PROJ-1' }))
      .toBe('Ticket: PROJ-1. Go.');
    expect(renderTemplate(tpl, {}))
      .toBe('Go.');
  });

  it('handles whitespace-only variables as empty', () => {
    expect(renderTemplate('{{#name}}Hello {{name}}{{/name}}', { name: '   ' }))
      .toBe('');
  });

  it('cleans up double spaces', () => {
    expect(renderTemplate('A {{#b}}B{{/b}} C', {}))
      .toBe('A C');
  });
});

describe('Quick Prompt dynamic title', () => {
  it('builds title with first non-empty variable', () => {
    const variables = [
      { name: 'ticket', label: 'Ticket', placeholder: '' },
      { name: 'repo', label: 'Repo', placeholder: '' },
    ];
    const launchVars: Record<string, string> = { ticket: 'PROJ-123', repo: 'front' };
    const baseName = 'Analyse ticket';
    const firstVal = variables.map(v => launchVars[v.name]).find(v => v?.trim());
    const title = firstVal ? `${baseName} — ${firstVal}` : baseName;
    expect(title).toBe('Analyse ticket — PROJ-123');
  });

  it('uses base name when no variables filled', () => {
    const variables = [{ name: 'ticket', label: 'Ticket', placeholder: '' }];
    const launchVars: Record<string, string> = {};
    const baseName = 'Analyse ticket';
    const firstVal = variables.map(v => launchVars[v.name]).find(v => v?.trim());
    const title = firstVal ? `${baseName} — ${firstVal}` : baseName;
    expect(title).toBe('Analyse ticket');
  });
});
