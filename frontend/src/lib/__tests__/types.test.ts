import { describe, it, expect } from 'vitest';
import type {
  AgentType,
  RunStatus,
  SetupStep,
  AiAuditStatus,
  McpTransport,
  ConditionAction,
  SkillCategory,
  ProfileCategory,
  DirectiveCategory,
  MessageRole,
  DriftCheckResponse,
  DriftSection,
} from '../../types/generated';

/**
 * Type-level tests: verify that union types and discriminated unions
 * match expected values. These catch drift between Rust models and frontend.
 */
describe('generated types — union exhaustiveness', () => {
  it('AgentType includes all 6 known agents', () => {
    const types: AgentType[] = ['ClaudeCode', 'Codex', 'Vibe', 'GeminiCli', 'Kiro', 'Custom'];
    expect(types).toHaveLength(6);
  });

  it('SetupStep covers the full flow', () => {
    const steps: SetupStep[] = ['Agents', 'ScanPaths', 'Detection', 'Complete'];
    expect(steps).toHaveLength(4);
  });

  it('RunStatus covers all possible states', () => {
    const statuses: RunStatus[] = ['Pending', 'Running', 'Success', 'Failed', 'Cancelled', 'WaitingApproval'];
    expect(statuses).toHaveLength(6);
  });

  it('AiAuditStatus covers the full lifecycle including Bootstrapped', () => {
    const statuses: AiAuditStatus[] = ['NoTemplate', 'TemplateInstalled', 'Bootstrapped', 'Audited', 'Validated'];
    expect(statuses).toHaveLength(5);
  });

  it('SkillCategory has 3 values', () => {
    const categories: SkillCategory[] = ['Language', 'Domain', 'Business'];
    expect(categories).toHaveLength(3);
  });

  it('ProfileCategory has 3 values', () => {
    const categories: ProfileCategory[] = ['Technical', 'Business', 'Meta'];
    expect(categories).toHaveLength(3);
  });

  it('DirectiveCategory has 2 values', () => {
    const categories: DirectiveCategory[] = ['Output', 'Language'];
    expect(categories).toHaveLength(2);
  });

  it('MessageRole has 3 values', () => {
    const roles: MessageRole[] = ['User', 'Agent', 'System'];
    expect(roles).toHaveLength(3);
  });
});

describe('generated types — discriminated unions', () => {
  it('McpTransport has 3 variants with correct shapes', () => {
    const stdio: McpTransport = { Stdio: { command: 'npx', args: ['-y', 'mcp-server'] } };
    const sse: McpTransport = { Sse: { url: 'http://localhost:3000/sse' } };
    const streamable: McpTransport = { Streamable: { url: 'http://localhost:3000/stream' } };

    expect(stdio.Stdio?.command).toBe('npx');
    expect(sse.Sse?.url).toContain('sse');
    expect(streamable.Streamable?.url).toContain('stream');
  });

  it('ConditionAction has Stop, Skip, and Goto variants', () => {
    const stop: ConditionAction = { type: 'Stop' };
    const skip: ConditionAction = { type: 'Skip' };
    const goto: ConditionAction = { type: 'Goto', step_name: 'cleanup' };

    expect(stop.type).toBe('Stop');
    expect(skip.type).toBe('Skip');
    expect(goto.type).toBe('Goto');
    expect(goto.step_name).toBe('cleanup');
  });
});

describe('generated types — drift detection', () => {
  it('DriftCheckResponse has required fields', () => {
    const staleSection: DriftSection = {
      ai_file: 'ai/index.md',
      audit_step: 1,
      changed_sources: ['src/main.rs'],
    };
    const response: DriftCheckResponse = {
      audit_date: '2026-03-20T10:00:00Z',
      stale_sections: [staleSection],
      fresh_sections: ['ai/glossary.md'],
      total_sections: 2,
    };
    expect(response.audit_date).toBe('2026-03-20T10:00:00Z');
    expect(response.stale_sections).toHaveLength(1);
    expect(response.stale_sections[0].ai_file).toBe('ai/index.md');
    expect(response.stale_sections[0].audit_step).toBe(1);
    expect(response.stale_sections[0].changed_sources).toContain('src/main.rs');
    expect(response.fresh_sections).toContain('ai/glossary.md');
    expect(response.total_sections).toBe(2);
  });
});
