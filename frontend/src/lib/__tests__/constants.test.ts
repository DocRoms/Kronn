import { describe, it, expect } from 'vitest';
import { AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, agentColor, getProjectGroup, isHiddenPath, isUsable, isValidationDisc, isBriefingDisc, isBootstrapDisc, agentSupportsIntrospection, isTrackerMcp, TRACKER_MCP_NEEDLES, parseRepoUrl, buildOldestIssueRequest, inferTrackerSlugFromRepoUrl } from '../constants';

describe('constants', () => {
  describe('AGENT_COLORS', () => {
    it('has colors for all agent types', () => {
      for (const at of ALL_AGENT_TYPES) {
        expect(AGENT_COLORS[at]).toBeDefined();
        expect(AGENT_COLORS[at]).toMatch(/^#/);
      }
    });

    it('has display-name aliases for Claude and Gemini', () => {
      expect(AGENT_COLORS['Claude Code']).toBe(AGENT_COLORS['ClaudeCode']);
      expect(AGENT_COLORS['Gemini CLI']).toBe(AGENT_COLORS['GeminiCli']);
    });
  });

  describe('AGENT_LABELS', () => {
    it('has labels for all agent types', () => {
      for (const at of ALL_AGENT_TYPES) {
        expect(AGENT_LABELS[at]).toBeDefined();
        expect(typeof AGENT_LABELS[at]).toBe('string');
      }
    });

    it('maps type names to display names', () => {
      expect(AGENT_LABELS['ClaudeCode']).toBe('Claude Code');
      expect(AGENT_LABELS['GeminiCli']).toBe('Gemini CLI');
      expect(AGENT_LABELS['Codex']).toBe('Codex');
      expect(AGENT_LABELS['Vibe']).toBe('Vibe');
    });
  });

  describe('ALL_AGENT_TYPES', () => {
    it('contains the 7 real agent types — Custom is intentionally excluded', () => {
      // ALL_AGENT_TYPES lists only the concrete, installable agent types.
      // AgentType (from generated.ts) also includes "Custom" (an 8th variant)
      // which is a generic escape-hatch type, not a selectable agent in the UI.
      // Therefore ALL_AGENT_TYPES has 7 entries and Custom is excluded on purpose.
      expect(ALL_AGENT_TYPES).toHaveLength(7);
      expect(ALL_AGENT_TYPES).toContain('ClaudeCode');
      expect(ALL_AGENT_TYPES).toContain('Codex');
      expect(ALL_AGENT_TYPES).toContain('Vibe');
      expect(ALL_AGENT_TYPES).toContain('GeminiCli');
      expect(ALL_AGENT_TYPES).toContain('Kiro');
      expect(ALL_AGENT_TYPES).toContain('CopilotCli');
      expect(ALL_AGENT_TYPES).not.toContain('Custom');
    });
  });

  describe('agentColor()', () => {
    it('returns correct color for known agent', () => {
      expect(agentColor('ClaudeCode')).toBe('#D4714E');
      expect(agentColor('Codex')).toBe('#10a37f');
      expect(agentColor('GeminiCli')).toBe('#4285f4');
    });

    it('returns fallback purple for unknown agent', () => {
      expect(agentColor('UnknownAgent')).toBe('#8b5cf6');
    });

    it('returns fallback for null/undefined', () => {
      expect(agentColor(null)).toBe('#8b5cf6');
      expect(agentColor(undefined)).toBe('#8b5cf6');
    });

    it('handles display name keys', () => {
      expect(agentColor('Claude Code')).toBe('#D4714E');
      expect(agentColor('Gemini CLI')).toBe('#4285f4');
    });
  });

  describe('getProjectGroup()', () => {
    it('extracts GitHub org from SSH URL', () => {
      expect(getProjectGroup({ repo_url: 'git@github.com:acme-org/my-project.git' }))
        .toBe('acme-org');
    });

    it('extracts GitHub org from HTTPS URL', () => {
      expect(getProjectGroup({ repo_url: 'https://github.com/johndoe/awesome-app.git' }))
        .toBe('johndoe');
    });

    it('extracts GitLab org from SSH URL', () => {
      expect(getProjectGroup({ repo_url: 'git@gitlab.com:myorg/myproject.git' }))
        .toBe('myorg');
    });

    it('returns local label when no repo_url', () => {
      expect(getProjectGroup({ repo_url: null })).toBe('Local');
      expect(getProjectGroup({ repo_url: null }, 'Perso')).toBe('Perso');
    });

    it('returns other label on invalid URL', () => {
      expect(getProjectGroup({ repo_url: 'not-a-url' })).toBe('Other');
      expect(getProjectGroup({ repo_url: 'not-a-url' }, 'Local', 'Divers')).toBe('Divers');
    });

    it('returns empty string repo_url as local', () => {
      expect(getProjectGroup({ repo_url: '' })).toBe('Local');
    });
  });

  describe('isHiddenPath()', () => {
    it('detects hidden segments in path', () => {
      expect(isHiddenPath('/home/.config/app')).toBe(true);
      expect(isHiddenPath('.hidden/project')).toBe(true);
      expect(isHiddenPath('/home/user/.local/share')).toBe(true);
    });

    it('returns false for visible paths', () => {
      expect(isHiddenPath('/home/user/projects/my-app')).toBe(false);
      expect(isHiddenPath('projects/frontend')).toBe(false);
    });

    it('handles edge cases', () => {
      expect(isHiddenPath('')).toBe(false);
      expect(isHiddenPath('.')).toBe(true);
      expect(isHiddenPath('..')).toBe(true);
    });
  });

  describe('isUsable()', () => {
    it('returns true when installed and enabled', () => {
      expect(isUsable({ installed: true, runtime_available: false, enabled: true })).toBe(true);
    });

    it('returns true when runtime available and enabled', () => {
      expect(isUsable({ installed: false, runtime_available: true, enabled: true })).toBe(true);
    });

    it('returns false when disabled', () => {
      expect(isUsable({ installed: true, runtime_available: true, enabled: false })).toBe(false);
    });

    it('returns false when neither installed nor runtime available', () => {
      expect(isUsable({ installed: false, runtime_available: false, enabled: true })).toBe(false);
    });
  });

  describe('isValidationDisc()', () => {
    it('detects exact validation title', () => {
      expect(isValidationDisc('Validation audit AI')).toBe(true);
    });

    it('rejects non-matching titles', () => {
      expect(isValidationDisc('validation audit AI')).toBe(false);
      expect(isValidationDisc('Validation audit AI ')).toBe(false);
      expect(isValidationDisc('')).toBe(false);
      expect(isValidationDisc('Some other discussion')).toBe(false);
    });
  });

  describe('isBriefingDisc()', () => {
    // Pre-fix this used `startsWith('Briefing')`, which mismatched the
    // English title `Project Briefing` (en) — English users got none of
    // the briefing-specific UI (Zap icon, completion CTA, refetch effect).
    // Regression guard: must accept all three localized titles emitted by
    // the backend's `start_briefing` handler.
    it('detects all three locale-specific briefing titles', () => {
      expect(isBriefingDisc('Project Briefing')).toBe(true);     // en
      expect(isBriefingDisc('Briefing del proyecto')).toBe(true); // es
      expect(isBriefingDisc('Briefing projet')).toBe(true);       // fr
    });

    it('rejects unrelated titles', () => {
      expect(isBriefingDisc('Validation audit AI')).toBe(false);
      expect(isBriefingDisc('Bootstrap: my-app')).toBe(false);
      expect(isBriefingDisc('Refactor the API')).toBe(false);
      expect(isBriefingDisc('')).toBe(false);
    });
  });

  describe('isBootstrapDisc()', () => {
    it('detects bootstrap titles regardless of project name', () => {
      expect(isBootstrapDisc('Bootstrap: my-app')).toBe(true);
      expect(isBootstrapDisc('Bootstrap: TestProject')).toBe(true);
    });

    it('rejects user-named discs that mention "bootstrap"', () => {
      expect(isBootstrapDisc('About bootstrap testing')).toBe(false);
      expect(isBootstrapDisc('bootstrap: lowercase')).toBe(false);
      expect(isBootstrapDisc('Validation audit AI')).toBe(false);
    });
  });

  // ── Cross-agent regression (auto-extends) ──────────────────────────
  describe('cross-agent consistency', () => {
    it('ALL_AGENT_TYPES matches the generated AgentType union (minus Custom)', () => {
      // If a new agent is added to the Rust enum but not to ALL_AGENT_TYPES
      // in constants.ts, this test fails. The generated.ts union is the
      // source of truth from the backend.
      const knownFromGenerated: string[] = ['ClaudeCode', 'Codex', 'Vibe', 'GeminiCli', 'Kiro', 'CopilotCli', 'Ollama'];
      expect(ALL_AGENT_TYPES.sort()).toEqual(knownFromGenerated.sort());
    });

    it('every agent has a color, a label, and a non-empty label', () => {
      for (const at of ALL_AGENT_TYPES) {
        expect(AGENT_COLORS[at], `${at} missing color`).toBeDefined();
        expect(AGENT_LABELS[at], `${at} missing label`).toBeDefined();
        expect(AGENT_LABELS[at].length, `${at} label is empty`).toBeGreaterThan(0);
      }
    });

    it('has at least 6 agent types (grows when new agents are added)', () => {
      expect(ALL_AGENT_TYPES.length).toBeGreaterThanOrEqual(7);
    });
  });

  // The introspection predicate gates a UI warning ("the kronn-internal
  // history tools won't fire for this agent") in ChatHeader's
  // summary-strategy popover. Test pins the agents we know don't speak
  // MCP and lets the rest pass through. The backend mirror lives in
  // `backend/src/api/disc_prompts.rs` (`agent_speaks_mcp`) — keep them
  // in sync; divergence shows up here as a test that suddenly disagrees
  // with the backend's prompt-injection gate.
  describe('agentSupportsIntrospection', () => {
    it('includes Vibe + Ollama via slash-marker fallback (multi-turn)', () => {
      // Vibe + Ollama don't speak MCP, but the post-stream parser in
      // backend/src/api/discussions/slash_markers.rs picks up
      // KRONN:DISC_* markers from their reply and resolves them into
      // System messages on the next turn. The UX is multi-turn but
      // functional, so the warning popover doesn't show.
      expect(agentSupportsIntrospection('Vibe')).toBe(true);
      expect(agentSupportsIntrospection('Ollama')).toBe(true);
    });

    it('includes Codex since 0.132 (upstream sandbox fix, 2026-05-20)', () => {
      // Codex 0.121 was excluded because exec-mode sandbox cancelled
      // the MCP tool call spawn. Codex 0.132 lifts that restriction
      // (confirmed live via `tools/call disc_meta` smoke test). The
      // old TD-20260510-codex-mcp-sandbox-block is closed.
      expect(agentSupportsIntrospection('Codex')).toBe(true);
    });

    it('includes every concrete agent type', () => {
      // Every `AgentType` now has at least one introspection path
      // (MCP for the modern CLIs, slash-marker fallback for Vibe +
      // Ollama). The predicate is a `return true` placeholder kept
      // as a stable grep target for the day a future agent breaks
      // the assumption.
      expect(ALL_AGENT_TYPES.length).toBeGreaterThan(0);
      for (const t of ALL_AGENT_TYPES) {
        expect(agentSupportsIntrospection(t), `${t} should support introspection`).toBe(true);
      }
    });

    it('treats Custom as supporting (user owns their config)', () => {
      // Custom isn't in ALL_AGENT_TYPES but is a valid AgentType — we
      // err on the side of "show the tools" rather than hide them, so
      // a user who wires their own Custom agent to read .mcp.json gets
      // the introspection bridge without us needing to know about it.
      expect(agentSupportsIntrospection('Custom')).toBe(true);
    });
  });

  describe('isTrackerMcp', () => {
    it('matches the 6 canonical tracker names case-insensitively', () => {
      // Real server names observed in the wild — must all match.
      const positives = [
        'github', 'GitHub', 'github-mcp',
        'gitlab', 'GitLab',
        'jira', 'Jira',
        'atlassian', 'Atlassian',
        'linear', 'Linear',
        'youtrack', 'YouTrack',
        '@modelcontextprotocol/server-github',
        'mcp-server-jira',
      ];
      for (const name of positives) {
        expect(isTrackerMcp(name), `${name} should match a tracker`).toBe(true);
      }
    });

    it('does not false-match unrelated MCP servers', () => {
      const negatives = [
        'chartbeat', 'fastly', 'aws-cloudwatch',
        'docker', 'memory', 'context7',
        'kronn-internal', 'playwright', 'sequential-thinking',
        'resend',
      ];
      for (const name of negatives) {
        expect(isTrackerMcp(name), `${name} should NOT match a tracker`).toBe(false);
      }
    });

    it('keeps the canonical needle list in sync with the backend', () => {
      // Backend source of truth: api/audit/helpers.rs:325-333. If the
      // backend list grows (e.g. + bitbucket / azure-devops), update
      // both sides simultaneously so the ProjectCard tracker-hint
      // banner stays accurate.
      expect([...TRACKER_MCP_NEEDLES].sort()).toEqual(
        ['atlassian', 'github', 'gitlab', 'jira', 'linear', 'youtrack'],
      );
    });
  });

  describe('parseRepoUrl', () => {
    it('parses SSH GitHub URLs (.git suffix)', () => {
      expect(parseRepoUrl('git@github.com:DocRoms/DOCROMS_WEB.git'))
        .toEqual({ owner: 'DocRoms', repo: 'DOCROMS_WEB' });
    });

    it('parses HTTPS GitHub URLs (.git suffix)', () => {
      expect(parseRepoUrl('https://github.com/DocRoms/RustCrawler.git'))
        .toEqual({ owner: 'DocRoms', repo: 'RustCrawler' });
    });

    it('parses HTTPS GitHub URLs without .git suffix', () => {
      expect(parseRepoUrl('https://github.com/DocRoms/RustCrawler'))
        .toEqual({ owner: 'DocRoms', repo: 'RustCrawler' });
    });

    it('parses HTTPS GitHub URLs with trailing slash', () => {
      expect(parseRepoUrl('https://github.com/DocRoms/RustCrawler/'))
        .toEqual({ owner: 'DocRoms', repo: 'RustCrawler' });
    });

    it('handles org accounts (multi-word owner)', () => {
      expect(parseRepoUrl('git@github.com:Euronews-tech/front_euronews.git'))
        .toEqual({ owner: 'Euronews-tech', repo: 'front_euronews' });
    });

    it('parses GitLab URLs in both SSH and HTTPS shapes', () => {
      expect(parseRepoUrl('git@gitlab.com:acme/billing.git'))
        .toEqual({ owner: 'acme', repo: 'billing' });
      expect(parseRepoUrl('https://gitlab.com/acme/billing'))
        .toEqual({ owner: 'acme', repo: 'billing' });
    });

    it('parses Codeberg URLs', () => {
      expect(parseRepoUrl('https://codeberg.org/forgejo/forgejo.git'))
        .toEqual({ owner: 'forgejo', repo: 'forgejo' });
    });

    it('returns null for non-GitHub/GitLab hosts', () => {
      expect(parseRepoUrl('https://bitbucket.org/foo/bar')).toBeNull();
      expect(parseRepoUrl('https://git.example.com/foo/bar')).toBeNull();
    });

    it('returns null for null/empty/malformed inputs', () => {
      expect(parseRepoUrl(null)).toBeNull();
      expect(parseRepoUrl(undefined)).toBeNull();
      expect(parseRepoUrl('')).toBeNull();
      expect(parseRepoUrl('not a url')).toBeNull();
      expect(parseRepoUrl('github.com')).toBeNull();
    });
  });

  describe('buildOldestIssueRequest', () => {
    const repo = { owner: 'DocRoms', repo: 'DOCROMS_WEB' };

    it('GitHub uses REST v3 issues endpoint + path placeholders', () => {
      const r = buildOldestIssueRequest('mcp-github', repo);
      expect(r).toEqual({
        endpoint: '/repos/{owner}/{repo}/issues',
        query: { state: 'open', sort: 'created', direction: 'asc', per_page: '1' },
        path_params: { owner: 'DocRoms', repo: 'DOCROMS_WEB' },
        extract_path: '$[0]',
      });
    });

    it('GitLab uses API v4 + project_id as owner/repo (resolver percent-encodes the slash)', () => {
      const r = buildOldestIssueRequest('mcp-gitlab', repo);
      expect(r).toEqual({
        endpoint: '/api/v4/projects/{project_id}/issues',
        query: { state: 'opened', order_by: 'created_at', sort: 'asc', per_page: '1' },
        path_params: { project_id: 'DocRoms/DOCROMS_WEB' },
        extract_path: '$[0]',
      });
    });

    it('Jira / Atlassian use JQL search (no owner/repo concept)', () => {
      for (const slug of ['mcp-jira', 'mcp-atlassian']) {
        const r = buildOldestIssueRequest(slug, repo);
        expect(r?.endpoint).toBe('/rest/api/3/search/jql');
        expect(r?.query.jql).toContain('ORDER BY created ASC');
        expect(r?.path_params).toBeUndefined();
        expect(r?.extract_path).toBe('$.issues[0]');
      }
    });

    it('falls back to empty owner/repo placeholders when repo is unparseable', () => {
      const r = buildOldestIssueRequest('mcp-github', null);
      expect(r?.path_params).toEqual({ owner: '', repo: '' });
    });

    it('returns null for unknown tracker slugs', () => {
      expect(buildOldestIssueRequest('mcp-linear', repo)).toBeNull();
      expect(buildOldestIssueRequest('mcp-chartbeat', repo)).toBeNull();
      expect(buildOldestIssueRequest('', repo)).toBeNull();
    });
  });

  describe('inferTrackerSlugFromRepoUrl', () => {
    it('maps github.com URLs to mcp-github', () => {
      expect(inferTrackerSlugFromRepoUrl('git@github.com:DocRoms/DOCROMS_WEB.git')).toBe('mcp-github');
      expect(inferTrackerSlugFromRepoUrl('https://github.com/DocRoms/Kronn')).toBe('mcp-github');
    });

    it('maps gitlab.com / self-hosted gitlab URLs to mcp-gitlab', () => {
      expect(inferTrackerSlugFromRepoUrl('git@gitlab.com:acme/billing.git')).toBe('mcp-gitlab');
      expect(inferTrackerSlugFromRepoUrl('https://gitlab.acme.io/team/repo')).toBe('mcp-gitlab');
    });

    it('returns null for non-github/gitlab hosts and empty inputs', () => {
      // Regression: pre-fix, the AutoPilot deep-link picked the FIRST
      // matching tracker MCP (often a globally-wired Jira) even when the
      // project lived on GitHub. With this helper, the repo_url is the
      // tie-breaker and Jira/Atlassian are ignored when github.com is
      // detected — see WorkflowWizard.tsx deep-link block.
      expect(inferTrackerSlugFromRepoUrl(null)).toBeNull();
      expect(inferTrackerSlugFromRepoUrl(undefined)).toBeNull();
      expect(inferTrackerSlugFromRepoUrl('')).toBeNull();
      expect(inferTrackerSlugFromRepoUrl('https://bitbucket.org/foo/bar')).toBeNull();
      expect(inferTrackerSlugFromRepoUrl('https://codeberg.org/forgejo/forgejo')).toBeNull();
    });
  });
});
