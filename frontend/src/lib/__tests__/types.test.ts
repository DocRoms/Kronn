import { describe, it, expect } from 'vitest';
import type {
  WorkflowRun,
  WorkflowTrigger,
  AgentType,
  RunStatus,
  SetupStep,
  AiAuditStatus,
  McpTransport,
  WorkflowAction,
  ConditionAction,
} from '../../types/generated';

/**
 * Type-level tests: these verify that the generated types are structurally sound.
 * They don't test runtime behavior — they test that the type definitions compile correctly.
 */
describe('generated types', () => {
  describe('WorkflowRun.trigger_context', () => {
    it('accepts WorkflowTrigger values (not any)', () => {
      const manualRun: WorkflowRun = {
        id: '1',
        workflow_id: 'wf-1',
        status: 'Running',
        trigger_context: { type: 'Manual' },
        step_results: [],
        tokens_used: 0,
        workspace_path: null,
        started_at: '2024-01-01T00:00:00Z',
        finished_at: null,
      };
      expect(manualRun.trigger_context).toEqual({ type: 'Manual' });
    });

    it('accepts Cron trigger context', () => {
      const cronTrigger: WorkflowTrigger = { type: 'Cron', schedule: '0 * * * *' };
      const run: WorkflowRun = {
        id: '2',
        workflow_id: 'wf-2',
        status: 'Success',
        trigger_context: cronTrigger,
        step_results: [],
        tokens_used: 100,
        workspace_path: null,
        started_at: '2024-01-01T00:00:00Z',
        finished_at: '2024-01-01T00:05:00Z',
      };
      expect(run.trigger_context).toEqual({ type: 'Cron', schedule: '0 * * * *' });
    });

    it('accepts null trigger_context', () => {
      const run: WorkflowRun = {
        id: '3',
        workflow_id: 'wf-3',
        status: 'Pending',
        trigger_context: null,
        step_results: [],
        tokens_used: 0,
        workspace_path: null,
        started_at: '2024-01-01T00:00:00Z',
        finished_at: null,
      };
      expect(run.trigger_context).toBeNull();
    });
  });

  describe('union types are exhaustive', () => {
    it('AgentType includes all known agents', () => {
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

    it('AiAuditStatus covers the full lifecycle', () => {
      const statuses: AiAuditStatus[] = ['NoTemplate', 'TemplateInstalled', 'Audited', 'Validated'];
      expect(statuses).toHaveLength(4);
    });
  });

  describe('discriminated union structures', () => {
    it('McpTransport variants have correct shapes', () => {
      const stdio: McpTransport = { Stdio: { command: 'npx', args: ['-y', 'mcp-server'] } };
      const sse: McpTransport = { Sse: { url: 'http://localhost:3000/sse' } };
      const streamable: McpTransport = { Streamable: { url: 'http://localhost:3000/stream' } };

      expect(stdio.Stdio?.command).toBe('npx');
      expect(sse.Sse?.url).toContain('sse');
      expect(streamable.Streamable?.url).toContain('stream');
    });

    it('WorkflowAction variants have correct shapes', () => {
      const pr: WorkflowAction = {
        type: 'CreatePr',
        title_template: 'Fix: {{issue_title}}',
        body_template: 'Closes #{{issue_number}}',
        branch_template: 'fix/{{issue_number}}',
      };
      const comment: WorkflowAction = {
        type: 'CommentIssue',
        body_template: 'Fixed in PR',
      };
      expect(pr.type).toBe('CreatePr');
      expect(comment.type).toBe('CommentIssue');
    });

    it('ConditionAction variants have correct shapes', () => {
      const stop: ConditionAction = { type: 'Stop' };
      const skip: ConditionAction = { type: 'Skip' };
      const goto: ConditionAction = { type: 'Goto', step_name: 'cleanup' };

      expect(stop.type).toBe('Stop');
      expect(skip.type).toBe('Skip');
      expect(goto.type).toBe('Goto');
    });
  });
});
