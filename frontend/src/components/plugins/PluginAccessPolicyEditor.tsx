import { useState } from 'react';
import { Lock, Plus, Save, Trash2 } from 'lucide-react';
import type {
  AgentType, ApiAccessPolicy, ApiAccessRule, ApiAccessSubject, ApiEndpoint, ApiEndpointAccess,
} from '../../types/generated';
import { AGENT_LABELS, ALL_AGENT_TYPES } from '../../lib/constants';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { ruleForKind, sameEndpoint, type Kind } from './accessPolicy';

const KINDS: Kind[] = ['all', 'agents', 'local_only', 'blocked'];
const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'];

function RuleEditor({ t, rule, onChange, testId }: {
  t: (key: string, ...args: (string | number)[]) => string;
  rule: ApiAccessRule;
  onChange: (rule: ApiAccessRule) => void;
  testId: string;
}) {
  const subjects: ApiAccessSubject[] = rule.kind === 'agents' ? rule.agents : [];
  const toggle = (agent: AgentType) => {
    const has = subjects.some(s => s.agent === agent);
    onChange({ kind: 'agents', agents: has ? subjects.filter(s => s.agent !== agent) : [...subjects, { agent }] });
  };
  const setModel = (agent: AgentType, model: string) => onChange({
    kind: 'agents',
    agents: subjects.map(s => s.agent === agent ? { agent, ...(model.trim() ? { model } : {}) } : s),
  });
  return (
    <div className="mcp-access-rule">
      <select
        aria-label={t('mcp.access.who')}
        data-testid={testId}
        value={rule.kind}
        onChange={e => onChange(ruleForKind(e.target.value as Kind, rule))}
      >
        {KINDS.map(kind => <option key={kind} value={kind}>{t(`mcp.access.kind.${kind}`)}</option>)}
      </select>
      {rule.kind === 'agents' && (
        <div className="mcp-access-agents">
          {ALL_AGENT_TYPES.map(agent => {
            const subject = subjects.find(s => s.agent === agent);
            return (
              <label key={agent} className="mcp-access-agent">
                <input type="checkbox" checked={!!subject} onChange={() => toggle(agent)} />
                <span>{AGENT_LABELS[agent] ?? agent}</span>
                {subject && (
                  <input
                    type="text"
                    className="mcp-access-model"
                    placeholder={t('mcp.access.anyModel')}
                    aria-label={t('mcp.access.model', AGENT_LABELS[agent] ?? agent)}
                    value={subject.model ?? ''}
                    onChange={e => setModel(agent, e.target.value)}
                  />
                )}
              </label>
            );
          })}
        </div>
      )}
    </div>
  );
}

/**
 * Which agents may call this plugin and which of its endpoints (KT-1026).
 * No policy keeps the plugin open as before; a policy makes it strict:
 * only the endpoints listed here can be called. The parent keys it on the
 * stored policy, so a save resets the draft.
 */
export function PluginAccessPolicyEditor({ t, serverId, endpoints, policy, onSave, isHybrid = false }: {
  t: (key: string, ...args: (string | number)[]) => string;
  serverId: string;
  /** The plugin also has an MCP connection, which does not go through the broker. */
  isHybrid?: boolean;
  endpoints: ApiEndpoint[];
  policy: ApiAccessPolicy | null;
  onSave: (serverId: string, policy: ApiAccessPolicy | null) => Promise<boolean>;
}) {
  const [draft, setDraft] = useState<ApiAccessPolicy | null>(policy);
  const [newMethod, setNewMethod] = useState('GET');
  const [newPath, setNewPath] = useState('');

  const save = useAsyncGuard(async () => { await onSave(serverId, draft); });
  const dirty = JSON.stringify(draft) !== JSON.stringify(policy);

  const ruleOf = (endpoint: { method: string; path: string }): ApiEndpointAccess | undefined =>
    draft?.endpoints.find(e => sameEndpoint(e, endpoint));
  const setEndpointRule = (endpoint: { method: string; path: string }, rule: ApiAccessRule | null) => {
    if (!draft) return;
    const others = draft.endpoints.filter(e => !sameEndpoint(e, endpoint));
    setDraft({
      ...draft,
      endpoints: rule ? [...others, { method: endpoint.method.toUpperCase(), path: endpoint.path, access: rule }] : others,
    });
  };
  const extras = draft?.endpoints.filter(rule => !endpoints.some(e => sameEndpoint(e, rule))) ?? [];
  const addExtra = () => {
    if (!draft || !newPath.trim()) return;
    setEndpointRule({ method: newMethod, path: newPath.trim() }, { kind: 'all' });
    setNewPath('');
  };

  return (
    <section className="mcp-detail-section mcp-access-policy" data-testid="mcp-access-policy">
      <h3 className="mcp-detail-section-title"><Lock size={12} />{t('mcp.access.title')}</h3>
      <label className="mcp-access-toggle">
        <input
          type="checkbox"
          data-testid="mcp-access-enable"
          checked={!!draft}
          onChange={e => setDraft(e.target.checked ? { access: { kind: 'all' }, endpoints: [] } : null)}
        />
        <span>{t('mcp.access.enable')}</span>
      </label>
      <p className="mcp-meta">{draft ? t('mcp.access.strictHint') : t('mcp.access.openHint')}</p>
      {isHybrid && <p className="mcp-meta mcp-access-hybrid" role="note" data-testid="mcp-access-hybrid">{t('mcp.access.hybridNote')}</p>}
      {draft && <>
        <div className="mcp-access-row">
          <strong>{t('mcp.access.plugin')}</strong>
          <RuleEditor t={t} testId="mcp-access-plugin" rule={draft.access} onChange={access => setDraft({ ...draft, access })} />
        </div>
        <h4 className="mcp-access-subtitle">{t('mcp.access.endpoints')}</h4>
        {[...endpoints, ...extras].map(endpoint => {
          const own = ruleOf(endpoint);
          const isExtra = !endpoints.some(e => sameEndpoint(e, endpoint));
          return (
            <div className="mcp-access-row" key={`${endpoint.method} ${endpoint.path}`}>
              <code>{endpoint.method.toUpperCase()} {endpoint.path}</code>
              <select
                aria-label={t('mcp.access.endpointMode', `${endpoint.method} ${endpoint.path}`)}
                value={own ? 'own' : 'inherit'}
                onChange={e => setEndpointRule(endpoint, e.target.value === 'own' ? { ...draft.access } : null)}
                disabled={isExtra}
              >
                <option value="inherit">{t('mcp.access.inherit')}</option>
                <option value="own">{t('mcp.access.override')}</option>
              </select>
              {own && <RuleEditor t={t} testId={`mcp-access-endpoint-${endpoint.method}-${endpoint.path}`} rule={own.access} onChange={rule => setEndpointRule(endpoint, rule)} />}
              {isExtra && (
                <button type="button" className="mcp-icon-btn" aria-label={t('mcp.access.removeEndpoint')} onClick={() => setEndpointRule(endpoint, null)}>
                  <Trash2 size={12} />
                </button>
              )}
            </div>
          );
        })}
        <div className="mcp-access-row mcp-access-add">
          <select aria-label={t('mcp.access.method')} value={newMethod} onChange={e => setNewMethod(e.target.value)}>
            {METHODS.map(m => <option key={m} value={m}>{m}</option>)}
          </select>
          <input
            type="text"
            placeholder="/path/{id}"
            aria-label={t('mcp.access.path')}
            value={newPath}
            onChange={e => setNewPath(e.target.value)}
          />
          <button type="button" className="mcp-btn-action" onClick={addExtra} disabled={!newPath.trim()}>
            <Plus size={12} />{t('mcp.access.addEndpoint')}
          </button>
        </div>
      </>}
      <div className="mcp-advanced-actions">
        <button type="button" className="mcp-btn-action mcp-btn-action-primary" data-testid="mcp-access-save" disabled={!dirty} onClick={() => void save()}>
          <Save size={12} />{t('mcp.save')}
        </button>
      </div>
    </section>
  );
}
