import { useState, useEffect } from 'react';
import { config as configApi, contacts as contactsApi } from '../../lib/api';
import { gravatarUrl } from '../../lib/gravatar';
import type { NetworkInfo } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import { UserCircle, Copy } from 'lucide-react';
import '../../pages/SettingsPage.css';

function GravatarPreview({ email }: { email: string }) {
  if (!email || !email.includes('@')) return null;
  return (
    <img src={gravatarUrl(email, 64)} alt="avatar" className="set-gravatar-img" />
  );
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

  // Load server config + global context once
  useEffect(() => {
    configApi.getServerConfig().then(cfg => {
      if (cfg) {
        setServerDomain(cfg.domain ?? '');
        setPseudo(cfg.pseudo ?? '');
        setAvatarEmail(cfg.avatar_email ?? '');
        setBio(cfg.bio ?? '');
      }
    }).catch(() => {});
    configApi.getGlobalContext().then(gc => {
      setGlobalContext(gc ?? '');
    }).catch(() => {});
    configApi.getGlobalContextMode().then(m => {
      setGlobalContextMode(m ?? 'always');
    }).catch(() => {});
  }, []);

  // Load network info (Tailscale detection, advertised host)
  useEffect(() => {
    contactsApi.networkInfo().then(setNetworkInfo).catch(() => {});
  }, [pseudo, serverDomain]);

  return (
    <div id="settings-identity" className="set-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <UserCircle size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.identity')}</span>
        </div>
        <p className="set-hint">
          {t('settings.identityHint')}
        </p>
        <div className="flex-row gap-8" style={{ alignItems: 'flex-start' }}>
          <div className="flex-1">
            <div className="mb-6">
              <span className="label">{t('settings.pseudo')}</span>
              <input
                type="text"
                value={pseudo}
                placeholder="Ex: JohnDoe42"
                onChange={e => {
                  setPseudo(e.target.value);
                  configApi.setServerConfig({ pseudo: e.target.value });
                }}
                className="set-input"
              />
            </div>
            <div>
              <span className="label">{t('settings.avatarEmail')}</span>
              <input
                type="email"
                value={avatarEmail}
                placeholder="email@example.com"
                onChange={e => {
                  setAvatarEmail(e.target.value);
                  configApi.setServerConfig({ avatar_email: e.target.value });
                }}
                className="set-input"
              />
              <div className="set-hint-xs">
                {t('settings.avatarHint')}{' '}
                <a href="https://gravatar.com" target="_blank" rel="noopener noreferrer" style={{ color: 'rgba(var(--kr-accent-rgb), 0.5)' }}>gravatar.com</a>
              </div>
            </div>
            <div style={{ marginTop: 12 }}>
              <span className="label">{t('settings.bio')}</span>
              <textarea
                value={bio}
                placeholder={t('settings.bioPlaceholder')}
                onChange={e => {
                  setBio(e.target.value);
                  configApi.setServerConfig({ bio: e.target.value });
                }}
                className="set-input"
                rows={2}
                style={{ resize: 'vertical' }}
              />
              <div className="set-hint-xs">
                {t('settings.bioHint')}
              </div>
            </div>
            <div style={{ marginTop: 16 }}>
              <div className="flex-row gap-4 mb-3" style={{ alignItems: 'baseline', flexWrap: 'wrap' }}>
                <span className="label mb-0">{t('settings.globalContext')}</span>
                <select
                  className="set-input cursor-pointer"
                  style={{ width: 'auto', flex: 'none' }}
                  value={globalContextMode}
                  onChange={e => {
                    setGlobalContextMode(e.target.value);
                    configApi.saveGlobalContextMode(e.target.value);
                  }}
                  aria-label={t('settings.globalContext')}
                >
                  <option value="always">{t('settings.gcModeAlways')}</option>
                  <option value="no_project">{t('settings.gcModeNoProject')}</option>
                  <option value="never">{t('settings.gcModeNever')}</option>
                </select>
              </div>
              <textarea
                value={globalContext}
                placeholder={t('settings.globalContextPlaceholder')}
                onChange={e => {
                  setGlobalContext(e.target.value);
                  setGlobalContextDirty(true);
                }}
                onBlur={() => {
                  if (globalContextDirty) {
                    configApi.saveGlobalContext(globalContext).then(() => {
                      toast(t('settings.globalContextSaved'), 'success');
                      setGlobalContextDirty(false);
                    }).catch(() => {});
                  }
                }}
                className="set-input"
                rows={6}
                style={{ resize: 'vertical', fontFamily: 'monospace', fontSize: 12 }}
              />
              <div className="set-hint-xs">
                {t('settings.globalContextHint')}
              </div>
            </div>
          </div>
          <div className="flex-col gap-2 mt-4" style={{ alignItems: 'center' }}>
            {avatarEmail ? (
              <GravatarPreview email={avatarEmail} />
            ) : pseudo ? (
              <div className="set-avatar-circle" data-variant="accent">
                {pseudo.slice(0, 2).toUpperCase()}
              </div>
            ) : (
              <div className="set-avatar-circle" data-variant="empty">?</div>
            )}
            <span className="text-xs text-muted">
              {pseudo || 'User'}
            </span>
          </div>
        </div>

        {/* Invite code for multi-user */}
        {pseudo && (
          <div className="set-invite-box">
            <div className="flex-row gap-3 text-sm font-semibold mb-3" style={{ color: 'rgba(var(--kr-accent-rgb), 0.6)' }}>
              {t('contacts.inviteCode')}
              {networkInfo?.tailscale_ip && networkInfo.advertised_host === networkInfo.tailscale_ip && (
                <span className="set-tailscale-badge">
                  Tailscale {networkInfo.tailscale_ip}
                </span>
              )}
            </div>
            <div className="flex-row gap-4">
              <code className="set-invite-code">
                kronn:{pseudo}@{networkInfo?.advertised_host ?? window.location.hostname}:{networkInfo?.port ?? (window.location.port || '3140')}
              </code>
              <button
                onClick={() => {
                  const host = networkInfo?.advertised_host ?? window.location.hostname;
                  const port = networkInfo?.port ?? (window.location.port || '3140');
                  const code = `kronn:${pseudo}@${host}:${port}`;
                  navigator.clipboard.writeText(code);
                  toast(t('disc.copy'), 'success');
                }}
                className="set-icon-btn"
                style={{ padding: '4px 8px', fontSize: 10 }}
                title={t('disc.copy')}
                aria-label={t('disc.copy')}
              >
                <Copy size={10} />
              </button>
            </div>
            <div className="text-xs text-faint mt-2">
              {networkInfo?.tailscale_ip && networkInfo.advertised_host === networkInfo.tailscale_ip
                ? t('contacts.tailscaleHint')
                : t('contacts.inviteHint')
              }
            </div>
          </div>
        )}

        {/* Connection guide */}
        <div className="set-guide-box">
          <div className="text-sm font-semibold text-tertiary mb-4">
            {t('contacts.guideTitle')}
          </div>
          <div className="text-sm text-muted" style={{ lineHeight: 1.6 }}>
            <div className="mb-3">
              <span style={{ color: 'rgba(var(--kr-accent-rgb), 0.6)', fontWeight: 600 }}>1.</span> {t('contacts.guideStep1')}{' '}
              <a href="https://tailscale.com" target="_blank" rel="noopener noreferrer" style={{ color: 'rgba(var(--kr-success-rgb), 0.7)', textDecoration: 'none' }}>tailscale.com</a>
            </div>
            <div className="mb-3">
              <span style={{ color: 'rgba(var(--kr-accent-rgb), 0.6)', fontWeight: 600 }}>2.</span> {t('contacts.guideStep2')}
            </div>
            <div className="mb-3">
              <span style={{ color: 'rgba(var(--kr-accent-rgb), 0.6)', fontWeight: 600 }}>3.</span> {t('contacts.guideStep3')}
            </div>
            <div>
              <span style={{ color: 'rgba(var(--kr-accent-rgb), 0.6)', fontWeight: 600 }}>4.</span> {t('contacts.guideStep4')}
            </div>
          </div>
          {/* Detected IPs */}
          {networkInfo && networkInfo.detected_ips.length > 0 && (
            <div className="set-guide-inner">
              <div className="text-xs font-semibold text-muted mb-3">
                {t('contacts.detectedIps')}
              </div>
              {networkInfo.detected_ips.map((d, i) => {
                const isActive = d.ip === networkInfo.advertised_host;
                return (
                <button
                  key={i}
                  onClick={() => {
                    if (isActive) return;
                    configApi.setServerConfig({ domain: d.ip });
                    setServerDomain(d.ip);
                    contactsApi.networkInfo().then(setNetworkInfo).catch(() => {});
                    toast(t('contacts.ipSelected'), 'success');
                  }}
                  className="set-ip-btn"
                  data-active={isActive}
                  title={isActive ? '' : t('contacts.clickToUse')}
                >
                  <span className="set-ip-kind" data-kind={d.kind}>
                    {d.kind.toUpperCase()}
                  </span>
                  <code className="text-secondary mono">{d.ip}</code>
                  <span className="text-ghost flex-1">{d.label}</span>
                  {isActive && (
                    <span className="text-accent font-semibold" style={{ fontSize: 8 }}>{'\u2713'} {t('contacts.usedInCode')}</span>
                  )}
                </button>
                );
              })}
            </div>
          )}

          <div className="set-guide-inner" style={{ fontSize: 10, color: 'var(--kr-text-dim)', lineHeight: 1.5 }}>
            <span className="font-semibold text-muted">{t('contacts.guideNetworkTitle')}</span><br />
            {t('contacts.guideNetwork')}
          </div>
        </div>
      </div>
    </div>
  );
}
