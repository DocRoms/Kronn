import { useEffect, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { ShieldCheck, BookOpen, ChevronRight } from 'lucide-react';
import { config as configApi } from '../../lib/api';
import type { ToastFn } from '../../hooks/useToast';
import { Dropdown } from '../Dropdown';
import '../../pages/SettingsPage.css';

type AhMode = 'off' | 'warn' | 'enforce';
const AH_MODES: readonly AhMode[] = ['off', 'warn', 'enforce'] as const;
const isAhMode = (x: unknown): x is AhMode =>
  typeof x === 'string' && (AH_MODES as readonly string[]).includes(x);

const remarkPlugins = [remarkGfm];

interface AntiHallucSectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function AntiHallucSection({ toast, t }: AntiHallucSectionProps) {
  const [mode, setMode] = useState<AhMode>('warn');
  const [specOpen, setSpecOpen] = useState(false);
  const [specContent, setSpecContent] = useState<string | null>(null);
  const [specError, setSpecError] = useState(false);
  const [specLoading, setSpecLoading] = useState(false);

  useEffect(() => {
    let alive = true;
    configApi.getAntiHallucinationMode()
      .then(m => { if (alive) setMode(isAhMode(m) ? m : 'warn'); })
      .catch(() => {});
    return () => { alive = false; };
  }, []);

  const toggleSpec = async () => {
    if (specContent) {
      setSpecOpen(o => !o);
      return;
    }
    // After a previous failure, a second click should retry (clear the
    // error and re-fetch) rather than re-opening an empty error panel.
    if (specError) setSpecError(false);
    setSpecLoading(true);
    try {
      const r = await fetch('/api/conventions/agents-md-format-v1');
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      const text = await r.text();
      setSpecContent(text);
      setSpecOpen(true);
    } catch {
      setSpecError(true);
      setSpecOpen(true);
    } finally {
      setSpecLoading(false);
    }
  };

  const onModeChange = (v: AhMode) => {
    const prev = mode;
    setMode(v);
    configApi.saveAntiHallucinationMode(v)
      .then(() => {
        toast(t('settings.antiHallucSaved'), 'success');
        if (v === 'enforce' && prev !== 'enforce') {
          toast(t('settings.ahEnforcePreviewToast'), 'info');
        }
      })
      .catch(() => {
        setMode(prev);
        toast(t('settings.antiHallucSaveError'), 'error');
      });
  };

  return (
    <div id="settings-sourcing" className="set-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <ShieldCheck size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.sourcingTitle')}</span>
        </div>
        <p className="set-hint">
          {t('settings.sourcingIntro')}
        </p>

        <div style={{ marginTop: 8, marginBottom: 16 }}>
          <button
            type="button"
            onClick={toggleSpec}
            className="set-icon-btn"
            style={{ padding: '6px 10px', fontSize: 11, display: 'inline-flex', alignItems: 'center', gap: 6 }}
            aria-expanded={specOpen}
            aria-controls="settings-sourcing-spec"
            aria-busy={specLoading}
            disabled={specLoading}
            data-testid="settings-sourcing-spec-toggle"
          >
            <ChevronRight
              size={10}
              className="set-sourcing-spec-chev"
              style={{ transform: specOpen ? 'rotate(90deg)' : 'none' }}
            />
            <BookOpen size={11} />
            {specLoading ? t('settings.sourcingSpecLoading') : t('settings.sourcingSpecToggle')}
          </button>
          {specOpen && (
            <div
              id="settings-sourcing-spec"
              role="region"
              aria-label={t('settings.sourcingSpecRegion')}
              tabIndex={0}
              data-testid="settings-sourcing-spec"
              className="set-sourcing-spec"
            >
              {specError ? (
                <div className="text-danger" style={{ fontSize: 12 }}>
                  {t('settings.sourcingSpecError')}
                </div>
              ) : specContent ? (
                <ReactMarkdown remarkPlugins={remarkPlugins}>{specContent}</ReactMarkdown>
              ) : null}
            </div>
          )}
        </div>

        <div className="flex-row gap-4 mb-3" style={{ alignItems: 'baseline', flexWrap: 'wrap' }}>
          <span className="label mb-0">{t('settings.antiHalluc')}</span>
          <div style={{ width: 240 }}>
            <Dropdown<AhMode>
              value={mode}
              options={[
                { value: 'off', label: t('settings.ahModeOff') },
                { value: 'warn', label: t('settings.ahModeWarn') },
                { value: 'enforce', label: t('settings.ahModeEnforce') },
              ]}
              onChange={onModeChange}
              ariaLabel={t('settings.antiHalluc')}
              testId="settings-anti-hallucination-mode"
            />
          </div>
        </div>

        <ul className="set-hint-sm" style={{ marginTop: 4, paddingLeft: 18, lineHeight: 1.6 }}>
          <li><strong>{t('settings.ahModeOff')}</strong> — {t('settings.ahExplainOff')}</li>
          <li><strong>{t('settings.ahModeWarn')}</strong> — {t('settings.ahExplainWarn')}</li>
          <li><strong>{t('settings.ahModeEnforce')}</strong> — {t('settings.ahExplainEnforce')}</li>
        </ul>
      </div>
    </div>
  );
}
