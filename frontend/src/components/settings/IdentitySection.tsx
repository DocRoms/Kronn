import { useEffect, useState } from 'react';
import { AtSign, ChevronDown, Copy, FileText, Network, ShieldCheck, UserCircle } from 'lucide-react';
import { config as configApi, contacts as contactsApi, type NetworkExposure } from '../../lib/api';
import { gravatarUrl } from '../../lib/gravatar';
import { userError } from '../../lib/userError';
import { invokeTauri, isTauriRuntime } from '../../lib/tauri';
import type { NetworkInfo } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import { ContextHelp } from '../ContextHelp';
import { Dropdown } from '../Dropdown';
import '../../pages/SettingsPage.css';

function GravatarPreview({ email, alt }: { email: string; alt: string }) {
  if (!email || !email.includes('@')) return null;
  return <img src={gravatarUrl(email, 96)} alt={alt} className="set-gravatar-img" />;
}

interface IdentitySectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function IdentitySection({ toast, t }: IdentitySectionProps) {
  const [pseudo, setPseudo] = useState('');
  const [avatarEmail, setAvatarEmail] = useState('');
  const [bio, setBio] = useState('');
  const [globalContext, setGlobalContext] = useState('');
  const [globalContextDirty, setGlobalContextDirty] = useState(false);
  const [globalContextMode, setGlobalContextMode] = useState('always');
  const [serverDomain, setServerDomain] = useState('');
  const [networkInfo, setNetworkInfo] = useState<NetworkInfo | null>(null);
  const [exposure, setExposure] = useState<NetworkExposure | null>(null);
  const [showConnectionGuide, setShowConnectionGuide] = useState(false);
  const isTauri = isTauriRuntime();

  useEffect(() => {
    configApi.getServerConfig().then(config => {
      if (!config) return;
      setServerDomain(config.domain ?? '');
      setPseudo(config.pseudo ?? '');
      setAvatarEmail(config.avatar_email ?? '');
      setBio(config.bio ?? '');
    }).catch(() => {});
    configApi.getGlobalContext().then(context => {
      setGlobalContext(context ?? '');
    }).catch(() => {});
    configApi.getGlobalContextMode().then(mode => {
      setGlobalContextMode(mode ?? 'always');
    }).catch(() => {});
  }, []);

  useEffect(() => {
    contactsApi.networkInfo().then(setNetworkInfo).catch(() => {});
  }, [pseudo, serverDomain]);

  useEffect(() => {
    configApi.getNetworkExposure().then(setExposure).catch(() => {});
  }, []);

  const restartApp = async () => {
    try {
      await invokeTauri('restart_app');
    } catch { /* Web mode or restart unavailable. */ }
  };

  const reportSaveError = (error: unknown) => {
    toast(t('common.actionFailed', userError(error)), 'error');
  };
  const inviteHost = networkInfo?.advertised_host ?? window.location.hostname;
  const invitePort = networkInfo?.port ?? (window.location.port || '3140');
  const inviteCode = `kronn:${pseudo}@${inviteHost}:${invitePort}`;

  return (
    <div id="settings-identity" className="set-card set-identity-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <UserCircle size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.identity')}</span>
        </div>
        <p className="set-hint">{t('settings.identityHint')}</p>

        <div className="set-identity-stack">
          <section className="set-identity-panel" data-kind="profile">
            <div className="set-identity-panel-head">
              <div className="set-identity-panel-icon"><UserCircle size={16} /></div>
              <div>
                <h3>{t('settings.identityProfileTitle')}</h3>
                <p>{t('settings.identityProfileHint')}</p>
              </div>
            </div>

            <div className="set-identity-profile-grid">
              <aside className="set-identity-preview" aria-label={t('settings.identityPreview')}>
                {avatarEmail.includes('@') ? (
                  <GravatarPreview email={avatarEmail} alt={t('settings.identityAvatarAlt')} />
                ) : pseudo ? (
                  <div className="set-avatar-circle" data-variant="accent">
                    {pseudo.slice(0, 2).toUpperCase()}
                  </div>
                ) : (
                  <div className="set-avatar-circle" data-variant="empty">?</div>
                )}
                <div className="set-identity-preview-copy">
                  <strong>{pseudo || t('settings.identityAnonymous')}</strong>
                  <p>{bio || t('settings.identityBioFallback')}</p>
                </div>
                <span className="set-identity-visibility">
                  <ShieldCheck size={11} />
                  {t('settings.identityVisibleInDiscussions')}
                </span>
              </aside>

              <div className="set-identity-fields">
                <div className="set-identity-fields-grid">
                  <div>
                    <label className="set-form-label" htmlFor="identity-pseudo">{t('settings.pseudo')}</label>
                    <input
                      id="identity-pseudo"
                      type="text"
                      value={pseudo}
                      placeholder={t('settings.pseudoPlaceholder')}
                      onChange={event => {
                        setPseudo(event.target.value);
                        configApi.setServerConfig({ pseudo: event.target.value }).catch(reportSaveError);
                      }}
                      className="set-input"
                    />
                  </div>
                  <div>
                    <label className="set-form-label" htmlFor="identity-avatar-email">{t('settings.avatarEmail')}</label>
                    <input
                      id="identity-avatar-email"
                      type="email"
                      value={avatarEmail}
                      placeholder={t('settings.avatarEmailPlaceholder')}
                      onChange={event => {
                        setAvatarEmail(event.target.value);
                        configApi.setServerConfig({ avatar_email: event.target.value }).catch(reportSaveError);
                      }}
                      className="set-input"
                    />
                    <div className="set-hint-xs">
                      {t('settings.avatarHint')}{' '}
                      <a href="https://gravatar.com" target="_blank" rel="noopener noreferrer">gravatar.com</a>
                    </div>
                  </div>
                </div>
                <div>
                  <div className="set-identity-label-row">
                    <label className="set-form-label" htmlFor="identity-bio">{t('settings.bio')}</label>
                    <ContextHelp title={t('settings.bioInfoTitle')} align="end">
                      <p>{t('settings.bioInfoInjection')}</p>
                      <p>{t('settings.bioInfoCli')}</p>
                    </ContextHelp>
                  </div>
                  <textarea
                    id="identity-bio"
                    value={bio}
                    placeholder={t('settings.bioPlaceholder')}
                    onChange={event => {
                      setBio(event.target.value);
                      configApi.setServerConfig({ bio: event.target.value }).catch(reportSaveError);
                    }}
                    className="set-input set-identity-bio"
                    rows={3}
                  />
                  <div className="set-hint-xs">{t('settings.bioHint')}</div>
                </div>
              </div>
            </div>
          </section>

          <section className="set-identity-panel" data-kind="context">
            <div className="set-identity-panel-head set-identity-context-head">
              <div className="set-identity-panel-icon"><FileText size={16} /></div>
              <div className="flex-1">
                <div className="set-identity-title-row">
                  <h3>{t('settings.identityContextTitle')}</h3>
                  <ContextHelp title={t('settings.globalContextInfoTitle')}>
                    <p>{t('settings.globalContextInfoInjection')}</p>
                    <p>{t('settings.globalContextInfoCli')}</p>
                  </ContextHelp>
                </div>
                <p>{t('settings.identityContextHint')}</p>
              </div>
              <div className="set-identity-context-mode">
                <span>{t('settings.identityContextMode')}</span>
                <Dropdown<'always' | 'no_project' | 'never'>
                  value={globalContextMode as 'always' | 'no_project' | 'never'}
                  options={[
                    { value: 'always', label: t('settings.gcModeAlways') },
                    { value: 'no_project', label: t('settings.gcModeNoProject') },
                    { value: 'never', label: t('settings.gcModeNever') },
                  ]}
                  onChange={value => {
                    setGlobalContextMode(value);
                    configApi.saveGlobalContextMode(value).catch(reportSaveError);
                  }}
                  ariaLabel={t('settings.globalContext')}
                  testId="settings-global-context-mode"
                />
              </div>
            </div>
            <textarea
              value={globalContext}
              placeholder={t('settings.globalContextPlaceholder')}
              onChange={event => {
                setGlobalContext(event.target.value);
                setGlobalContextDirty(true);
              }}
              onBlur={() => {
                if (!globalContextDirty) return;
                configApi.saveGlobalContext(globalContext).then(() => {
                  toast(t('settings.globalContextSaved'), 'success');
                  setGlobalContextDirty(false);
                }).catch(error => {
                  console.warn('Failed to save global context:', error);
                  toast(t('settings.globalContextSaveFailed', userError(error)), 'error');
                });
              }}
              className="set-input set-identity-context-input"
              rows={7}
            />
            <div className="set-identity-context-foot">
              <span>{t('settings.globalContextHint')}</span>
              <span className="set-identity-save-state" data-dirty={globalContextDirty} role="status">
                {globalContextDirty ? t('settings.globalContextPending') : t('settings.globalContextAutoSave')}
              </span>
            </div>
          </section>

          <section className="set-identity-panel" data-kind="network">
            <div className="set-identity-panel-head">
              <div className="set-identity-panel-icon"><Network size={16} /></div>
              <div className="flex-1">
                <h3>{t('settings.identityNetworkTitle')}</h3>
                <p>{t('settings.identityNetworkHint')}</p>
              </div>
              <span className="set-identity-network-state" data-exposed={!!exposure?.exposed}>
                {exposure?.exposed ? t('settings.networkReachable') : t('settings.networkLocalOnly')}
              </span>
            </div>

            <button
              type="button"
              role="switch"
              data-testid="expose-network-toggle"
              aria-checked={!!exposure?.exposed}
              className="set-identity-network-toggle"
              disabled={exposure === null}
              onClick={async () => {
                try {
                  const updated = await configApi.setNetworkExposure(!exposure?.exposed);
                  setExposure(updated);
                } catch (error) {
                  reportSaveError(error);
                }
              }}
            >
              <span>
                <strong>{t('settings.exposeNetwork')}</strong>
                <small>{t('settings.exposeNetworkHint')}</small>
              </span>
              <span className="set-toggle-track" data-on={!!exposure?.exposed} aria-hidden="true">
                <span className="set-toggle-thumb" style={{ left: exposure?.exposed ? 15 : 1 }} />
              </span>
            </button>

            {exposure?.exposed && (
              <div className="set-expose-warn" role="note">{t('settings.exposeSecurityNote')}</div>
            )}
            {exposure?.restart_required && (
              <div className="set-expose-restart" role="alert">
                <span>{t('settings.exposeRestartRequired')}</span>
                {isTauri ? (
                  <button type="button" className="btn btn-ghost" onClick={restartApp}>
                    {t('settings.exposeRestartBtn')}
                  </button>
                ) : (
                  <code className="set-ollama-cmd">./kronn restart</code>
                )}
              </div>
            )}

            {pseudo ? (
              <div className="set-invite-box">
                <div className="set-invite-head">
                  <div>
                    <strong>{t('contacts.inviteCode')}</strong>
                    <p>{t('contacts.inviteHint')}</p>
                  </div>
                  {networkInfo?.tailscale_ip && networkInfo.advertised_host === networkInfo.tailscale_ip && (
                    <span className="set-tailscale-badge">Tailscale {networkInfo.tailscale_ip}</span>
                  )}
                </div>
                <div className="set-invite-code-row">
                  <AtSign size={14} aria-hidden="true" />
                  <code className="set-invite-code">{inviteCode}</code>
                  <button
                    onClick={() => {
                      navigator.clipboard.writeText(inviteCode);
                      toast(t('disc.copy'), 'success');
                    }}
                    className="set-icon-btn"
                    title={t('disc.copy')}
                    aria-label={t('disc.copy')}
                  >
                    <Copy size={12} />
                  </button>
                </div>
                {networkInfo?.tailscale_ip && networkInfo.advertised_host === networkInfo.tailscale_ip && (
                  <div className="set-hint-xs">{t('contacts.tailscaleHint')}</div>
                )}
              </div>
            ) : (
              <div className="set-identity-invite-empty">{t('settings.identityInviteNeedsPseudo')}</div>
            )}

            <button
              type="button"
              className="set-identity-guide-toggle"
              aria-expanded={showConnectionGuide}
              onClick={() => setShowConnectionGuide(open => !open)}
            >
              <span>
                <strong>{t('contacts.guideTitle')}</strong>
                <small>{t('settings.identityGuideHint')}</small>
              </span>
              <ChevronDown size={15} data-expanded={showConnectionGuide} />
            </button>

            {showConnectionGuide && (
              <div className="set-guide-box">
                <ol className="set-identity-guide-steps">
                  <li>
                    <span>1</span>
                    <div>{t('contacts.guideStep1')}{' '}
                      <a href="https://tailscale.com" target="_blank" rel="noopener noreferrer">tailscale.com</a>
                    </div>
                  </li>
                  <li><span>2</span><div>{t('contacts.guideStep2')}</div></li>
                  <li><span>3</span><div>{t('contacts.guideStep3')}</div></li>
                  <li><span>4</span><div>{t('contacts.guideStep4')}</div></li>
                </ol>

                {networkInfo && networkInfo.detected_ips.length > 0 && (
                  <div className="set-guide-inner">
                    <div className="text-xs font-semibold text-muted mb-3">{t('contacts.detectedIps')}</div>
                    {networkInfo.detected_ips.map((detected, index) => {
                      const isActive = detected.ip === networkInfo.advertised_host;
                      return (
                        <button
                          key={index}
                          onClick={() => {
                            if (isActive) return;
                            configApi.setServerConfig({ domain: detected.ip }).catch(reportSaveError);
                            setServerDomain(detected.ip);
                            contactsApi.networkInfo().then(setNetworkInfo).catch(() => {});
                            toast(t('contacts.ipSelected'), 'success');
                          }}
                          className="set-ip-btn"
                          data-active={isActive}
                          title={isActive ? '' : t('contacts.clickToUse')}
                        >
                          <span className="set-ip-kind" data-kind={detected.kind}>{detected.kind.toUpperCase()}</span>
                          <code className="text-secondary mono">{detected.ip}</code>
                          <span className="text-ghost flex-1">{detected.label}</span>
                          {isActive && (
                            <span className="text-accent font-semibold set-ip-active">
                              {'✓'} {t('contacts.usedInCode')}
                            </span>
                          )}
                        </button>
                      );
                    })}
                  </div>
                )}

                <div className="set-guide-inner set-identity-network-help">
                  <strong>{t('contacts.guideNetworkTitle')}</strong>
                  <p>{t('contacts.guideNetwork')}</p>
                </div>
              </div>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
