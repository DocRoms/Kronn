import { useState, useEffect } from 'react';
import { profiles as profilesApi } from '../../lib/api';
import type { AgentProfile } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import { Plus, Trash2, Check, X } from 'lucide-react';
import '../../pages/SettingsPage.css';

interface ProfilesSectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ProfilesSection({ toast, t }: ProfilesSectionProps) {
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [showCreateProfile, setShowCreateProfile] = useState(false);
  const [newProfileName, setNewProfileName] = useState('');
  const [newProfilePersonaName, setNewProfilePersonaName] = useState('');
  const [newProfileRole, setNewProfileRole] = useState('');
  const [newProfileAvatar, setNewProfileAvatar] = useState('\uD83E\uDD16');
  const [newProfileColor, setNewProfileColor] = useState('#a78bfa');
  const [newProfileCategory, setNewProfileCategory] = useState<'Technical' | 'Business' | 'Meta'>('Technical');
  const [newProfilePersona, setNewProfilePersona] = useState('');
  const [expandedProfileDesc, setExpandedProfileDesc] = useState<string | null>(null);
  const [editingPersonaId, setEditingPersonaId] = useState<string | null>(null);
  const [editingPersonaValue, setEditingPersonaValue] = useState('');

  useEffect(() => {
    profilesApi.list().then(setAvailableProfiles).catch(e => console.warn('Failed to load profiles:', e));
  }, []);

  return (
    <div>
      <div>

        <div className="flex-wrap mb-8" style={{ gap: 12, maxHeight: 400, overflowY: 'auto', overflowX: 'hidden' }}>
          {availableProfiles.map(profile => (
            <div key={profile.id} className="set-profile-card" style={{ borderLeft: `3px solid ${profile.color}` }}>
              {/* Header: avatar + identity */}
              <div className="flex-row gap-6 mb-5" style={{ alignItems: 'flex-start' }}>
                <div
                  className="set-profile-avatar"
                  style={{ background: `${profile.color}18`, border: `1px solid ${profile.color}30` }}
                >
                  {profile.avatar}
                </div>
                <div className="flex-1 min-w-0">
                  <div className="font-bold text-md text-primary flex-row gap-2" style={{ lineHeight: 1.2 }}>
                    {editingPersonaId === profile.id ? (
                      <input
                        autoFocus
                        className="set-persona-input"
                        style={{ border: `1px solid ${profile.color}60`, color: profile.color }}
                        value={editingPersonaValue}
                        onChange={e => setEditingPersonaValue(e.target.value)}
                        onBlur={async () => {
                          if (editingPersonaValue !== profile.persona_name) {
                            try {
                              const updated = await profilesApi.updatePersonaName(profile.id, editingPersonaValue);
                              setAvailableProfiles(prev => prev.map(p => p.id === profile.id ? updated : p));
                            } catch (err) { console.warn('Settings action failed:', err); }
                          }
                          setEditingPersonaId(null);
                        }}
                        onKeyDown={e => { if (e.key === 'Enter') (e.target as HTMLInputElement).blur(); if (e.key === 'Escape') setEditingPersonaId(null); }}
                      />
                    ) : (
                      <span
                        style={{ color: profile.color, cursor: 'pointer' }}
                        title={t('profiles.clickToEditName')}
                        onClick={() => { setEditingPersonaId(profile.id); setEditingPersonaValue(profile.persona_name); }}
                      >
                        {profile.persona_name || '\u2014'}
                      </span>
                    )}
                    <span className="text-ghost">{'\u00B7'}</span>
                    {profile.name}
                  </div>
                  <div className="flex-row gap-3 text-sm text-muted mt-2">
                    {profile.role}
                    {profile.token_estimate > 0 && (
                      <span className="set-token-cost-badge" style={{ padding: '0px 5px' }} title={t('config.tokenCostHint')}>
                        ~{profile.token_estimate} tok
                      </span>
                    )}
                  </div>
                </div>
              </div>
              {/* Description: expandable persona_prompt */}
              {profile.persona_prompt && (
                <div className="mb-5">
                  <div
                    className="text-xs text-muted"
                    style={{
                      lineHeight: 1.4,
                      ...(expandedProfileDesc !== profile.id ? {
                        overflow: 'hidden', display: '-webkit-box',
                        WebkitLineClamp: 2, WebkitBoxOrient: 'vertical' as const,
                      } : {}),
                    }}
                  >
                    {expandedProfileDesc === profile.id ? profile.persona_prompt : profile.persona_prompt.slice(0, 150)}
                  </div>
                  {profile.persona_prompt.length > 100 && (
                    <button
                      className="set-see-more-btn"
                      style={{ color: profile.color }}
                      onClick={() => setExpandedProfileDesc(expandedProfileDesc === profile.id ? null : profile.id)}
                    >
                      {expandedProfileDesc === profile.id ? t('common.seeLess') : t('common.seeMore')}
                    </button>
                  )}
                </div>
              )}
              {/* Badges + actions */}
              <div className="flex-row gap-2 flex-wrap">
                <span className="set-cat-badge" data-cat={profile.category}>
                  {t(`profiles.${profile.category.toLowerCase()}`)}
                </span>
                {profile.is_builtin ? (
                  <span className="set-builtin-badge">{t('profiles.builtin')}</span>
                ) : (
                  <span className="set-custom-badge">{t('profiles.custom')}</span>
                )}
                {profile.default_engine && (
                  <span className="set-builtin-badge">{profile.default_engine}</span>
                )}
                <div className="flex-1" />
                {!profile.is_builtin && (
                  <button
                    className="set-icon-btn text-error"
                    style={{ padding: '2px 6px', borderColor: 'rgba(255,77,106,0.2)' }}
                    onClick={async () => {
                      if (!confirm(t('profiles.deleteConfirm'))) return;
                      try {
                        await profilesApi.delete(profile.id);
                        setAvailableProfiles(prev => prev.filter(p => p.id !== profile.id));
                        toast(t('common.delete'), 'success');
                      } catch (err) { console.warn('Settings action failed:', err); }
                    }}
                  >
                    <Trash2 size={10} />
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>

        {!showCreateProfile ? (
          <button
            className="set-action-btn"
            onClick={() => setShowCreateProfile(true)}
          >
            <Plus size={12} /> {t('profiles.createCustom')}
          </button>
        ) : (
          <div className="set-create-form">
            <div className="set-grid-2">
              <div>
                <label className="set-form-label">{t('profiles.name')}</label>
                <input className="set-input" value={newProfileName} onChange={e => setNewProfileName(e.target.value)} placeholder="Architect, QA Lead..." />
              </div>
              <div>
                <label className="set-form-label">{t('profiles.personaName')}</label>
                <input className="set-input" value={newProfilePersonaName} onChange={e => setNewProfilePersonaName(e.target.value)} placeholder="Leo, Mia, Sam..." />
              </div>
            </div>
            <div className="set-grid-2">
              <div>
                <label className="set-form-label">{t('profiles.role')}</label>
                <input className="set-input" value={newProfileRole} onChange={e => setNewProfileRole(e.target.value)} placeholder="Software Architect, QA Engineer..." />
              </div>
              <div>
                <label className="set-form-label">{t('profiles.category')}</label>
                <select
                  className="set-input cursor-pointer"
                  value={newProfileCategory}
                  onChange={e => setNewProfileCategory(e.target.value as 'Technical' | 'Business' | 'Meta')}
                >
                  <option value="Technical">{t('profiles.technical')}</option>
                  <option value="Business">{t('profiles.business')}</option>
                  <option value="Meta">{t('profiles.meta')}</option>
                </select>
              </div>
            </div>
            <div className="set-grid-avatar">
              <div>
                <label className="set-form-label">{t('profiles.avatar')}</label>
                <input className="set-input set-avatar-input" value={newProfileAvatar} onChange={e => setNewProfileAvatar(e.target.value)} placeholder="\uD83E\uDD16" />
              </div>
              <div />
              <div>
                <label className="set-form-label">{t('profiles.color')}</label>
                <input className="set-input set-color-input" type="color" value={newProfileColor} onChange={e => setNewProfileColor(e.target.value)} />
              </div>
            </div>
            <div className="mb-5">
              <label className="set-form-label">{t('profiles.persona')}</label>
              <textarea
                className="set-textarea"
                value={newProfilePersona}
                onChange={e => setNewProfilePersona(e.target.value)}
                placeholder="You are an expert in... Always prioritize..."
              />
            </div>
            <div className="flex-row gap-4">
              <button
                className="set-action-btn"
                style={{ opacity: newProfileName && newProfilePersona ? 1 : 0.4 }}
                disabled={!newProfileName || !newProfilePersona}
                onClick={async () => {
                  try {
                    const created = await profilesApi.create({
                      name: newProfileName,
                      persona_name: newProfilePersonaName,
                      role: newProfileRole,
                      avatar: newProfileAvatar,
                      color: newProfileColor,
                      category: newProfileCategory,
                      persona_prompt: newProfilePersona,
                    });
                    setAvailableProfiles(prev => [...prev, created]);
                    setShowCreateProfile(false);
                    setNewProfileName(''); setNewProfilePersonaName(''); setNewProfileRole(''); setNewProfileAvatar('\uD83E\uDD16'); setNewProfileColor('#a78bfa'); setNewProfilePersona('');
                    toast(t('profiles.createCustom'), 'success');
                  } catch (err) { console.warn('Settings action failed:', err); }
                }}
              >
                <Check size={12} /> {t('profiles.createCustom')}
              </button>
              <button
                className="set-icon-btn"
                onClick={() => { setShowCreateProfile(false); setNewProfileName(''); setNewProfilePersonaName(''); setNewProfileRole(''); setNewProfileAvatar('\uD83E\uDD16'); setNewProfileColor('#a78bfa'); setNewProfilePersona(''); }}
              >
                <X size={12} />
              </button>
            </div>
          </div>
        )}

      </div>
    </div>
  );
}
