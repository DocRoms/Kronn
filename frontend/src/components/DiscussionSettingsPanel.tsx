import { useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import {
  Check,
  ChevronRight,
  Cpu,
  FileText,
  Search,
  Server,
  Share2,
  Settings,
  UserCircle,
  Users2,
  Wrench,
  X,
  Zap,
} from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import {
  AGENT_LABELS,
  agentSupportsIntrospection,
  isHiddenPath,
} from '../lib/constants';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type { ToastFn } from '../hooks/useToast';
import type {
  AgentProfile,
  AgentType,
  Contact,
  Directive,
  Discussion,
  McpConfigDisplay,
  McpIncompatibility,
  Project,
  Skill,
} from '../types/generated';
import { ProfileTooltip } from './ProfileTooltip';
import './DiscussionToolPanel.css';
import './DiscussionSettingsPanel.css';

interface Props {
  discussion: Discussion;
  projects: Project[];
  availableSkills: Skill[];
  availableProfiles: AgentProfile[];
  availableDirectives: Directive[];
  mcpConfigs: McpConfigDisplay[];
  mcpIncompatibilities: McpIncompatibility[];
  contacts: Contact[];
  onClose: () => void;
  onDiscussionUpdated: () => void;
  onShare: (contactIds: string[]) => void | Promise<void>;
  toast: ToastFn;
}

type ConfigSection = 'profiles' | 'skills' | 'directives' | null;

export function DiscussionSettingsPanel({
  discussion,
  projects,
  availableSkills,
  availableProfiles,
  availableDirectives,
  mcpConfigs,
  mcpIncompatibilities,
  contacts,
  onClose,
  onDiscussionUpdated,
  onShare,
  toast,
}: Props) {
  const { t } = useT();
  const [expandedSection, setExpandedSection] = useState<ConfigSection>(null);
  const [mcpSearch, setMcpSearch] = useState('');

  const discussionMcps = useMemo(() => {
    const projectId = discussion.project_id;
    return projectId
      ? mcpConfigs.filter(config => config.is_global || config.project_ids.includes(projectId))
      : mcpConfigs.filter(config => config.include_general);
  }, [discussion.project_id, mcpConfigs]);

  const filteredMcps = useMemo(() => {
    const query = mcpSearch.trim().toLowerCase();
    if (!query) return discussionMcps;
    return discussionMcps.filter(config => (
      config.label.toLowerCase().includes(query)
      || config.server_name.toLowerCase().includes(query)
    ));
  }, [discussionMcps, mcpSearch]);

  const isApiOnly = (['Vibe'] as AgentType[]).includes(discussion.agent);
  const profileCount = discussion.profile_ids?.length ?? 0;
  const skillCount = discussion.skill_ids?.length ?? 0;
  const directiveCount = discussion.directive_ids?.length ?? 0;

  const updateDiscussion = async (patch: Parameters<typeof discussionsApi.update>[1]) => {
    try {
      await discussionsApi.update(discussion.id, patch);
      onDiscussionUpdated();
    } catch (cause) {
      toast(userError(cause), 'error');
    }
  };

  return (
    <aside className="disc-tool-panel disc-settings-panel" aria-label={t('disc.settingsPanel')}>
      <header className="disc-tool-panel-header">
        <div className="disc-tool-panel-title">
          <Settings size={15} />
          <span>{t('disc.settingsPanel')}</span>
        </div>
        <button
          type="button"
          className="disc-tool-panel-icon"
          onClick={onClose}
          aria-label={t('common.close')}
        >
          <X size={16} />
        </button>
      </header>

      <div className="disc-tool-panel-body disc-settings-body">
        <section className="disc-settings-overview" aria-label={t('disc.settingsOverview')}>
          <div className="disc-settings-agent">
            <Cpu size={13} />
            <span>{t('disc.agent')}</span>
            <strong>{AGENT_LABELS[discussion.agent] ?? discussion.agent}</strong>
          </div>
          <div className="disc-settings-stats">
            <span title={t('disc.introspectionPillTooltip', discussion.introspection_call_count ?? 0, (discussion.introspection_call_count ?? 0) > 1 ? 's' : '')}>
              <Wrench size={11} /> {discussion.introspection_call_count ?? 0}
            </span>
            <span title={t('profiles.select')}><UserCircle size={11} /> {profileCount}</span>
            <span title={t('skills.selectSkills')}><Zap size={11} /> {skillCount}</span>
            <span title={t('directives.title')}><FileText size={11} /> {directiveCount}</span>
            <span title={t('disc.mcps')}><Server size={11} /> {discussionMcps.length}</span>
          </div>
        </section>

        {contacts.length > 0 && (
          <section className="disc-settings-section">
            <div className="disc-settings-section-head">
              <h3>{t('disc.sharing')}</h3>
              {discussion.shared_id && (
                <span className="disc-settings-connected">
                  <Users2 size={10} /> {t('contacts.wsConnected')}
                </span>
              )}
            </div>
            <div className="disc-settings-contact-list">
              {contacts.map(contact => {
                const alreadyShared = discussion.shared_with?.includes(contact.id) ?? false;
                return (
                  <button
                    key={contact.id}
                    type="button"
                    className="disc-settings-contact"
                    data-shared={alreadyShared}
                    disabled={alreadyShared}
                    onClick={() => {
                      if (!alreadyShared) void onShare([contact.id]);
                    }}
                  >
                    {alreadyShared ? <Check size={12} /> : <Share2 size={12} />}
                    <span>{contact.pseudo}</span>
                    <small>{alreadyShared ? t('disc.alreadyShared') : t('disc.shareWith')}</small>
                  </button>
                );
              })}
            </div>
          </section>
        )}

        <section className="disc-settings-section">
          <h3>{t('disc.settingsContext')}</h3>
          <label className="disc-settings-field">
            <span>{t('disc.project')}</span>
            <select
              className="disc-popover-select"
              value={discussion.project_id ?? ''}
              onChange={event => {
                void updateDiscussion({ project_id: event.target.value || '' });
              }}
            >
              <option value="">{t('disc.general')}</option>
              {projects.filter(project => !isHiddenPath(project.path)).map(project => (
                <option key={project.id} value={project.id}>{project.name}</option>
              ))}
            </select>
          </label>

          {availableProfiles.length > 0 && (
            <ConfigAccordion
              icon={<UserCircle size={12} />}
              title={t('profiles.select')}
              count={profileCount}
              expanded={expandedSection === 'profiles'}
              onToggle={() => setExpandedSection(current => current === 'profiles' ? null : 'profiles')}
            >
              {availableProfiles.map(profile => {
                const active = (discussion.profile_ids ?? []).includes(profile.id);
                return (
                  <ProfileTooltip key={profile.id} profile={profile}>
                    <button
                      type="button"
                      className="disc-toggle-pill"
                      data-active={active}
                      data-color="purple"
                      style={{
                        borderColor: active ? (profile.color || 'rgba(var(--kr-purple-rgb), 0.4)') : undefined,
                        background: active ? `${profile.color}15` : undefined,
                        color: active ? (profile.color || 'var(--kr-purple-soft)') : undefined,
                      }}
                      onClick={() => {
                        const current = discussion.profile_ids ?? [];
                        void updateDiscussion({
                          profile_ids: active
                            ? current.filter(id => id !== profile.id)
                            : [...current, profile.id],
                        });
                      }}
                    >
                      {active && <Check size={8} />}
                      {profile.avatar} {profile.persona_name || profile.name}
                    </button>
                  </ProfileTooltip>
                );
              })}
            </ConfigAccordion>
          )}

          {availableSkills.length > 0 && (
            <ConfigAccordion
              icon={<Zap size={12} />}
              title={t('skills.selectSkills')}
              count={skillCount}
              expanded={expandedSection === 'skills'}
              onToggle={() => setExpandedSection(current => current === 'skills' ? null : 'skills')}
            >
              {availableSkills.map(skill => {
                const active = (discussion.skill_ids ?? []).includes(skill.id);
                return (
                  <button
                    key={skill.id}
                    type="button"
                    className="disc-toggle-pill"
                    data-active={active}
                    data-color="accent"
                    onClick={() => {
                      const current = discussion.skill_ids ?? [];
                      void updateDiscussion({
                        skill_ids: active
                          ? current.filter(id => id !== skill.id)
                          : [...current, skill.id],
                      });
                    }}
                  >
                    {active && <Check size={8} />}
                    {skill.icon} {skill.name}
                  </button>
                );
              })}
            </ConfigAccordion>
          )}

          {availableDirectives.length > 0 && (
            <ConfigAccordion
              icon={<FileText size={12} />}
              title={t('directives.title')}
              count={directiveCount}
              expanded={expandedSection === 'directives'}
              onToggle={() => setExpandedSection(current => current === 'directives' ? null : 'directives')}
            >
              {availableDirectives.map(directive => {
                const active = (discussion.directive_ids ?? []).includes(directive.id);
                return (
                  <button
                    key={directive.id}
                    type="button"
                    className="disc-toggle-pill"
                    data-active={active}
                    data-color="warning"
                    onClick={() => {
                      const current = discussion.directive_ids ?? [];
                      void updateDiscussion({
                        directive_ids: active
                          ? current.filter(id => id !== directive.id)
                          : [...current, directive.id],
                      });
                    }}
                  >
                    {active && <Check size={8} />}
                    {directive.icon} {directive.name}
                  </button>
                );
              })}
            </ConfigAccordion>
          )}

          <div className="disc-settings-field">
            <span>{t('disc.modelTier')}</span>
            <div className="disc-settings-options">
              {(['economy', 'default', 'reasoning'] as const).map(tier => (
                <button
                  key={tier}
                  type="button"
                  className="disc-toggle-pill"
                  data-active={(discussion.tier ?? 'default') === tier}
                  data-tier={tier}
                  onClick={() => void updateDiscussion({ tier })}
                >
                  {tier === 'economy' ? '⚡' : tier === 'reasoning' ? '🧠' : '⚙️'} {t(`disc.tier.${tier}`)}
                </button>
              ))}
            </div>
          </div>

          <div className="disc-settings-field">
            <span>{t('disc.summaryStrategyLabel')}</span>
            <div className="disc-settings-options">
              {(['Auto', 'OnDemand', 'Off'] as const).map(strategy => (
                <button
                  key={strategy}
                  type="button"
                  className="disc-toggle-pill"
                  data-active={(discussion.summary_strategy ?? 'Auto') === strategy}
                  title={t(`disc.summaryStrategy.${strategy}.hint`)}
                  onClick={() => void updateDiscussion({ summary_strategy: strategy })}
                >
                  {t(`disc.summaryStrategy.${strategy}.label`)}
                </button>
              ))}
            </div>
            {!agentSupportsIntrospection(discussion.agent) && (
              <div className="disc-popover-note" role="note" data-testid="introspection-unsupported-note">
                ⚠️ {t('disc.introspectionUnsupportedNote', AGENT_LABELS[discussion.agent] ?? discussion.agent)}
              </div>
            )}
          </div>
        </section>

        <section className="disc-settings-section">
          <div className="disc-settings-section-head">
            <h3>{t('disc.mcps')}</h3>
            <span>{discussionMcps.length}</span>
          </div>
          {discussionMcps.length > 6 && (
            <label className="disc-settings-search">
              <Search size={12} />
              <input
                value={mcpSearch}
                onChange={event => setMcpSearch(event.target.value)}
                placeholder={t('disc.mcpSearch')}
              />
            </label>
          )}
          {isApiOnly && <div className="disc-mcp-api-notice">⚡ Mode API — MCPs indisponibles</div>}
          <div className="disc-settings-mcp-list">
            {filteredMcps.length === 0 ? (
              <div className="disc-mcp-empty">{t('disc.noMcps')}</div>
            ) : filteredMcps.map(config => {
              const incompatibility = mcpIncompatibilities.find(item => (
                item.server_id === config.server_id && item.agent === discussion.agent
              ));
              return (
                <div
                  key={config.id}
                  className="disc-settings-mcp"
                  data-unavailable={Boolean(incompatibility) || isApiOnly}
                  title={incompatibility?.reason}
                >
                  <Server size={12} />
                  <span>{config.label}</span>
                  {incompatibility && <em>{t('disc.mcpIncompatible')}</em>}
                  <code>{config.server_name}</code>
                </div>
              );
            })}
          </div>
        </section>
      </div>
    </aside>
  );
}

function ConfigAccordion({
  icon,
  title,
  count,
  expanded,
  onToggle,
  children,
}: {
  icon: ReactNode;
  title: string;
  count: number;
  expanded: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <div className="disc-settings-accordion">
      <button type="button" onClick={onToggle} aria-expanded={expanded}>
        <ChevronRight size={10} className="disc-chevron" data-expanded={expanded} />
        {icon}
        <span>{title}</span>
        {count > 0 && <strong>{count}</strong>}
      </button>
      {expanded && <div className="disc-settings-chips">{children}</div>}
    </div>
  );
}
