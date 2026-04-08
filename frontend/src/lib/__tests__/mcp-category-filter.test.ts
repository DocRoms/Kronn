import { describe, it, expect } from 'vitest';

// Test the category mapping + filtering logic (mirrors McpPage.tsx)
const categoryMap: Record<string, string> = {
  git: 'Git & Code', code: 'Git & Code',
  database: 'Databases', sql: 'Databases', cache: 'Databases',
  cloud: 'Cloud', containers: 'Cloud', devops: 'Cloud',
  search: 'Search', web: 'Search', http: 'Search', browser: 'Search',
  monitoring: 'Monitoring', analytics: 'Monitoring', errors: 'Monitoring',
  communication: 'Communication', chat: 'Communication', email: 'Communication',
  'project-management': 'Project Mgmt', issues: 'Project Mgmt',
};

function getCategory(tags: string[]): string {
  for (const tag of tags) {
    if (categoryMap[tag]) return categoryMap[tag];
  }
  return 'Other';
}

interface MockMcp { id: string; name: string; tags: string[] }

function filterMcps(mcps: MockMcp[], selectedCategory: string | null, searchText: string): MockMcp[] {
  return mcps.filter(m => {
    if (selectedCategory && getCategory(m.tags) !== selectedCategory) return false;
    if (searchText && !m.name.toLowerCase().includes(searchText.toLowerCase()) && !m.tags.some(t => t.toLowerCase().includes(searchText.toLowerCase()))) return false;
    return true;
  });
}

describe('MCP category filtering', () => {
  const mcps: MockMcp[] = [
    { id: 'mcp-github', name: 'GitHub', tags: ['git', 'code'] },
    { id: 'mcp-postgres', name: 'PostgreSQL', tags: ['database', 'sql'] },
    { id: 'mcp-redis', name: 'Redis', tags: ['cache', 'database'] },
    { id: 'mcp-slack', name: 'Slack', tags: ['communication', 'chat'] },
    { id: 'mcp-sentry', name: 'Sentry', tags: ['monitoring', 'errors'] },
    { id: 'mcp-brave-search', name: 'Brave Search', tags: ['search', 'web'] },
  ];

  it('shows all MCPs when no filter', () => {
    expect(filterMcps(mcps, null, '')).toHaveLength(6);
  });

  it('filters by category only', () => {
    const result = filterMcps(mcps, 'Databases', '');
    expect(result).toHaveLength(2);
    expect(result.map(m => m.id)).toContain('mcp-postgres');
    expect(result.map(m => m.id)).toContain('mcp-redis');
  });

  it('filters by search text only', () => {
    const result = filterMcps(mcps, null, 'slack');
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('mcp-slack');
  });

  it('filters by category + search text', () => {
    const result = filterMcps(mcps, 'Databases', 'redis');
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('mcp-redis');
  });

  it('category filter does NOT match on translated category name', () => {
    // This was the original bug: using translated category name as search text
    const result = filterMcps(mcps, null, 'Databases');
    // "Databases" is NOT a tag, so no MCPs should match
    expect(result).toHaveLength(0);
  });

  it('search matches on tags', () => {
    const result = filterMcps(mcps, null, 'chat');
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('mcp-slack');
  });

  it('getCategory returns correct category', () => {
    expect(getCategory(['git', 'code'])).toBe('Git & Code');
    expect(getCategory(['database', 'sql'])).toBe('Databases');
    expect(getCategory(['communication'])).toBe('Communication');
    expect(getCategory(['unknown'])).toBe('Other');
  });
});
